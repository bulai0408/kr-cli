//! `kr run` and `kr gate` (spec §6, ADR 0005): build once, then run each scenario or
//! suite on leased pool simulators, check the results against the enumeration, rerun
//! failures once, and report PASS/FAIL per item.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Args;
use jiff::Timestamp;
use kr_config::{
    DEFAULT_DEVICE, DEFAULT_TIMEOUT, Destination, ProjectConfig, Suite, parse_timeout,
};
use kr_events::{Event, EventKind, RunId};
use kr_exec::{Command, Runner};
use kr_resource::Resource;
use kr_run::{Attempt, Final, Verdict, judge, longest_first};
use kr_sim::{DeviceLocks, LeaseHolder, PoolDevice, Simctl};
use kr_xcode::{
    Enumeration, XcTestRun, Xcodebuild, only_testing_id, parse_test_results, results_command,
};
use serde::Serialize;
use serde_json::json;

use super::check::{self, BuildJob, Selection, Steps, with_xcode};
use super::human_duration;
use crate::{Ctx, Loaded, slots};

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Scenarios and suites to run.
    #[arg(required = true)]
    targets: Vec<String>,
}

#[derive(Debug, Args)]
pub struct GateArgs {
    /// The gate to run. Default: `default`.
    name: Option<String>,
}

/// Where results go, relative to the repo root; the latest run of each item wins.
const RESULTS: &str = "build/kr/results";
const LEASE_POLL: Duration = Duration::from_secs(2);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub fn run(ctx: &Ctx, args: RunArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let (project, root) = loaded.require_project()?;
    let sel = check::select(project, &args.targets, None)?;
    execute(ctx, &loaded, project, &root, &sel, "run".to_owned())
}

pub fn gate(ctx: &Ctx, args: GateArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let (project, root) = loaded.require_project()?;
    let name = args.name.unwrap_or_else(|| "default".to_owned());
    if !project.gates.contains_key(&name) {
        let known: Vec<&str> = project.gates.keys().map(String::as_str).collect();
        bail!("no gate `{name}` in .kr.toml; known: {}", known.join(", "));
    }
    let sel = check::select(project, &[], Some(&name))?;
    execute(ctx, &loaded, project, &root, &sel, format!("gate {name}"))
}

/// One scenario or suite and how it ended.
#[derive(Debug, Clone, Serialize)]
struct ItemResult {
    name: String,
    kind: &'static str,
    passed: bool,
    duration_ms: u64,
    device: Option<String>,
    /// passed / failed / flaky / skipped counts, for test items.
    tests: Option<BTreeMap<&'static str, usize>>,
    flaky: Vec<String>,
    /// At most three lines on failure.
    summary: Vec<String>,
    /// Notes that do not fail the item (e.g. static and real enumeration disagree).
    notes: Vec<String>,
    xcresult: Option<PathBuf>,
    log: Option<PathBuf>,
    /// Load and memory pressure when the item started and ended.
    load: Option<serde_json::Value>,
    /// Set when the load was high (above `[load] per_core` × cores) during the item.
    high_load: Option<String>,
}

impl ItemResult {
    fn new(name: &str, kind: &'static str) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            passed: false,
            duration_ms: 0,
            device: None,
            tests: None,
            flaky: Vec::new(),
            summary: Vec::new(),
            notes: Vec::new(),
            xcresult: None,
            log: None,
            load: None,
            high_load: None,
        }
    }
}

/// A test run to place on a simulator (or on this Mac).
#[derive(Debug, Clone)]
struct WorkItem {
    name: String,
    kind: &'static str,
    /// Pool device kind; `None` runs on this Mac.
    device: Option<String>,
    job: usize,
    only: Vec<String>,
    skip: Vec<String>,
    timeout: Duration,
}

struct Shared<'a> {
    ctx: &'a Ctx,
    loaded: &'a Loaded,
    project: &'a ProjectConfig,
    root: &'a Path,
    jobs: &'a [BuildJob],
    runner: &'a dyn Runner,
    developer_dir: Option<&'a Path>,
    run_id: RunId,
}

fn execute(
    ctx: &Ctx,
    loaded: &Loaded,
    project: &ProjectConfig,
    root: &Path,
    sel: &Selection,
    label: String,
) -> Result<ExitCode> {
    let started = Instant::now();
    let runner = ctx.runner()?;
    let run_id = RunId::new();
    let developer_dir = loaded.developer_dir(ctx)?;
    let jobs = check::build_jobs(project, sel)?;
    let mut steps = Steps {
        loaded,
        run_id,
        command: "run",
        results: Vec::new(),
        cached: Vec::new(),
    };

    // Build once: only what changed, holding a build slot only while building.
    let hashes = check::job_hashes(
        project,
        root,
        &jobs,
        runner.as_ref(),
        developer_dir.as_deref(),
    );
    let mut built = true;
    if !check::stale(&jobs, &hashes, root, &mut steps).is_empty() {
        let task = format!("{label} build");
        let taken = slots::take(ctx, loaded, runner.as_ref(), run_id, Resource::Build, &task)?;
        // Someone may have built the same inputs while this run waited.
        let todo = check::stale(&jobs, &hashes, root, &mut steps);
        let result = check::build(
            project,
            root,
            &todo,
            runner.as_ref(),
            developer_dir.as_deref(),
            &mut steps,
        );
        slots::give_back(
            loaded,
            run_id,
            taken,
            Some(if matches!(result, Ok(true)) { 0 } else { 1 }),
        )?;
        built = result?;
    }
    if built {
        built = check::verify(
            project,
            root,
            &jobs,
            runner.as_ref(),
            developer_dir.as_deref(),
            loaded,
            &mut steps,
        )?;
    }
    if !built {
        report_build_failure(ctx, &label, &steps.results)?;
        return Ok(ExitCode::FAILURE);
    }

    let shared = Shared {
        ctx,
        loaded,
        project,
        root,
        jobs: &jobs,
        runner: runner.as_ref(),
        developer_dir: developer_dir.as_deref(),
        run_id,
    };
    let mut results: Vec<ItemResult> = Vec::new();
    // Build suites passed when their build did.
    for name in &sel.suites {
        if let Suite::Build { .. } = &project.suites[name] {
            let mut r = ItemResult::new(name, "suite");
            r.passed = true;
            results.push(r);
        }
    }
    for name in &sel.suites {
        if let Suite::Command { run, dir, timeout } = &project.suites[name] {
            results.push(run_command_suite(
                &shared,
                name,
                run,
                dir.as_deref(),
                timeout.as_deref(),
            )?);
        }
    }
    let items = work_items(project, sel, &jobs)?;
    results.extend(run_items(&shared, items)?);

    // Report in the order of the selection.
    let order: Vec<&String> = sel.suites.iter().chain(&sel.scenarios).collect();
    results.sort_by_key(|r| {
        order
            .iter()
            .position(|n| **n == r.name)
            .unwrap_or(usize::MAX)
    });
    let ok = results.iter().all(|r| r.passed);
    report(ctx, &label, &steps, &results, ok, started.elapsed())?;
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn work_items(
    project: &ProjectConfig,
    sel: &Selection,
    jobs: &[BuildJob],
) -> Result<Vec<WorkItem>> {
    let job_of = |dir: &Path, scheme: &str| {
        jobs.iter()
            .position(|j| j.dir == dir && j.scheme == scheme && j.action == "build-for-testing")
            .context("internal: no build for this item")
    };
    let mut items = Vec::new();
    for name in &sel.scenarios {
        let s = &project.scenarios[name];
        let scheme = project.project.scheme.as_deref().unwrap_or("");
        items.push(WorkItem {
            name: name.clone(),
            kind: "scenario",
            device: Some(
                s.device
                    .clone()
                    .unwrap_or_else(|| DEFAULT_DEVICE.to_owned()),
            ),
            job: job_of(Path::new(""), scheme)?,
            only: s.only.clone(),
            skip: s.skip.clone(),
            timeout: s
                .timeout
                .as_deref()
                .and_then(parse_timeout)
                .unwrap_or(DEFAULT_TIMEOUT),
        });
    }
    for name in &sel.suites {
        if let Suite::Test {
            dir,
            scheme,
            only,
            skip,
            destination,
            device,
            timeout,
        } = &project.suites[name]
        {
            items.push(WorkItem {
                name: name.clone(),
                kind: "suite",
                device: match destination {
                    Destination::Simulator => {
                        Some(device.clone().unwrap_or_else(|| DEFAULT_DEVICE.to_owned()))
                    }
                    Destination::Macos => None,
                },
                job: job_of(dir.as_deref().unwrap_or(Path::new("")), scheme)?,
                only: only.clone(),
                skip: skip.clone(),
                timeout: timeout
                    .as_deref()
                    .and_then(parse_timeout)
                    .unwrap_or(DEFAULT_TIMEOUT),
            });
        }
    }
    Ok(items)
}

/// A command suite, with the same high-load treatment as test items.
fn run_command_suite(
    s: &Shared<'_>,
    name: &str,
    run: &str,
    dir: Option<&Path>,
    timeout: Option<&str>,
) -> Result<ItemResult> {
    let per_core = s.loaded.config.machine.load.per_core;
    let sample = || kr_machine::read_hardware(s.runner).ok();
    let before = sample();
    let mut r = run_command_suite_once(s, name, run, dir, timeout)?;
    let after = sample();
    r.high_load = [&before, &after]
        .into_iter()
        .flatten()
        .find_map(|hw| hw.overloaded(per_core));
    if let (false, Some(reason)) = (r.passed, r.high_load.clone()) {
        let again = run_command_suite_once(s, name, run, dir, timeout)?;
        if again.passed {
            r = ItemResult {
                flaky: vec![format!(
                    "{name} (failed during high load ({reason}); passed when run again)"
                )],
                high_load: Some(reason),
                ..again
            };
        } else {
            r.notes.push(format!(
                "failed during high load ({reason}); failed again when run again"
            ));
        }
    }
    item_event(s, &r)?;
    Ok(r)
}

fn run_command_suite_once(
    s: &Shared<'_>,
    name: &str,
    run: &str,
    dir: Option<&Path>,
    timeout: Option<&str>,
) -> Result<ItemResult> {
    let started = Instant::now();
    let log = s.loaded.run_dir(s.run_id).join(format!("{name}.log"));
    let cmd = Command::new("sh")
        .args(["-c", run])
        .current_dir(s.root.join(dir.unwrap_or(Path::new(""))))
        .timeout(timeout.and_then(parse_timeout).unwrap_or(COMMAND_TIMEOUT));
    let out = s.runner.run_logged(&cmd, &log)?;
    let mut r = ItemResult::new(name, "suite");
    r.passed = out.success();
    r.duration_ms = started.elapsed().as_millis() as u64;
    r.log = Some(log);
    if !r.passed {
        r.summary = if out.timed_out {
            vec!["timed out".to_owned()]
        } else {
            kr_xcode::error_summary(&format!("{}{}", out.stdout, out.stderr))
        };
    }
    Ok(r)
}

/// Runs simulator items in lanes (one leased device each, at most `sim` capacity at
/// once), longest first; Mac items one after another.
fn run_items(s: &Shared<'_>, items: Vec<WorkItem>) -> Result<Vec<ItemResult>> {
    let history = durations(s.loaded);
    let items = longest_first(items, |i| history.get(&i.name).copied());
    let (mac, sim): (Vec<WorkItem>, Vec<WorkItem>) =
        items.into_iter().partition(|i| i.device.is_none());
    let mut results = Vec::new();
    for item in &mac {
        results.push(run_item(s, item, None)?);
    }
    if sim.is_empty() {
        return Ok(results);
    }
    let simctl = Simctl::new(s.runner, s.developer_dir.map(Path::to_path_buf));
    let pool = kr_sim::plan(s.project, &simctl.runtimes()?, &simctl.devices()?)?;
    let capacity = s.loaded.capacity(Resource::Sim, s.runner)? as usize;
    let mut by_kind: BTreeMap<String, VecDeque<WorkItem>> = BTreeMap::new();
    for item in sim {
        by_kind
            .entry(item.device.clone().unwrap_or_default())
            .or_default()
            .push_back(item);
    }
    let collected = Mutex::new(Vec::new());
    let errors = Mutex::new(Vec::new());
    // (kind, work queue, lanes); kinds without devices fail right away.
    let mut lanes_of: Vec<(String, Mutex<VecDeque<WorkItem>>, usize)> = Vec::new();
    for (kind, queue) in by_kind {
        let devices = pool
            .iter()
            .filter(|d| d.kind == kind && d.udid.is_some() && d.issue.is_none())
            .count();
        if devices == 0 {
            for item in &queue {
                let mut r = ItemResult::new(&item.name, item.kind);
                r.summary = vec![format!(
                    "no usable `{kind}` simulator in the pool; run `kr pool sync` and `kr doctor`"
                )];
                collected.lock().unwrap().push(r);
            }
            continue;
        }
        let lanes = capacity.min(devices).min(queue.len()).max(1);
        lanes_of.push((kind, Mutex::new(queue), lanes));
    }
    std::thread::scope(|scope| {
        for (kind, queue, lanes) in &lanes_of {
            for _ in 0..*lanes {
                let (pool, collected, errors, simctl) = (&pool, &collected, &errors, &simctl);
                scope.spawn(move || {
                    if let Err(e) = lane(s, simctl, pool, kind, queue, collected) {
                        errors.lock().unwrap().push(e);
                    }
                });
            }
        }
    });
    if let Some(e) = errors.into_inner().unwrap().into_iter().next() {
        return Err(e);
    }
    results.extend(collected.into_inner().unwrap());
    Ok(results)
}

fn lane(
    s: &Shared<'_>,
    simctl: &Simctl<'_>,
    pool: &[PoolDevice],
    kind: &str,
    queue: &Mutex<VecDeque<WorkItem>>,
    collected: &Mutex<Vec<ItemResult>>,
) -> Result<()> {
    if queue.lock().unwrap().is_empty() {
        return Ok(());
    }
    let task = format!("run {kind} lane");
    let taken = slots::take(s.ctx, s.loaded, s.runner, s.run_id, Resource::Sim, &task)?;
    // Other lanes may have finished the work while this one waited for its slot.
    if queue.lock().unwrap().is_empty() {
        return slots::give_back(s.loaded, s.run_id, taken, Some(0));
    }
    let holder = LeaseHolder {
        project: s.loaded.project().map(str::to_owned),
        task: task.clone(),
        pid: std::process::id(),
        started: Timestamp::now(),
    };
    let locks = DeviceLocks::new(kr_sim::locks_dir(&s.loaded.state_dir));
    let booting = Instant::now();
    let lease = match kr_sim::lease(simctl, &locks, pool, kind, &holder, LEASE_POLL, None) {
        Ok(lease) => lease,
        Err(e) => {
            slots::give_back(s.loaded, s.run_id, taken, None)?;
            return Err(e);
        }
    };
    let (name, udid) = (lease.name().to_owned(), lease.udid().to_owned());
    s.loaded.events().append(&Event::now(
        s.run_id,
        s.loaded.project(),
        EventKind::SimLeased,
        json!({ "device": name, "udid": udid, "task": task, "boot_ms": booting.elapsed().as_millis() as u64 }),
    ))?;
    let held_since = Instant::now();
    let mut outcome = Ok(());
    loop {
        let Some(item) = queue.lock().unwrap().pop_front() else {
            break;
        };
        match run_item(s, &item, Some((&name, &udid))) {
            Ok(r) => collected.lock().unwrap().push(r),
            Err(e) => {
                outcome = Err(e);
                break;
            }
        }
    }
    let released = lease.release();
    s.loaded.events().append(&Event::now(
        s.run_id,
        s.loaded.project(),
        EventKind::SimReleased,
        json!({ "device": name, "udid": udid, "held_ms": held_since.elapsed().as_millis() as u64 }),
    ))?;
    slots::give_back(
        s.loaded,
        s.run_id,
        taken,
        Some(if outcome.is_ok() { 0 } else { 1 }),
    )?;
    released?;
    outcome
}

/// Mean duration of each item's recent runs, from events of the past 14 days.
fn durations(loaded: &Loaded) -> BTreeMap<String, Duration> {
    let since = Timestamp::now() - jiff::SignedDuration::from_hours(24 * 14);
    let mut all: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for e in loaded.events().read_since(since).unwrap_or_default() {
        if e.kind == EventKind::ItemEnd
            && let (Some(name), Some(ms)) =
                (e.fields["item"].as_str(), e.fields["duration_ms"].as_u64())
        {
            all.entry(name.to_owned()).or_default().push(ms);
        }
    }
    all.into_iter()
        .map(|(k, v)| {
            let recent = &v[v.len().saturating_sub(5)..];
            (
                k,
                Duration::from_millis(recent.iter().sum::<u64>() / recent.len() as u64),
            )
        })
        .collect()
}

fn item_event(s: &Shared<'_>, r: &ItemResult) -> Result<()> {
    s.loaded.events().append(&Event::now(
        s.run_id,
        s.loaded.project(),
        EventKind::ItemEnd,
        json!({
            "item": r.name,
            "kind": r.kind,
            "passed": r.passed,
            "duration_ms": r.duration_ms,
            "device": r.device,
            "tests": r.tests,
            "flaky": r.flaky,
            "load": r.load,
            "high_load": r.high_load,
        }),
    ))
}

fn results_dir(item: &str) -> PathBuf {
    Path::new(RESULTS).join(item)
}

/// Runs one item, sampling the machine load before and after. A failure during high
/// load (`[load] per_core`) is marked as such and the whole item runs once more; if it
/// passes then, the item counts as flaky (ADR 0003).
fn run_item(s: &Shared<'_>, item: &WorkItem, device: Option<(&str, &str)>) -> Result<ItemResult> {
    let started = Instant::now();
    s.loaded.events().append(&Event::now(
        s.run_id,
        s.loaded.project(),
        EventKind::ItemStart,
        json!({ "item": item.name, "kind": item.kind, "device": device.map(|d| d.0) }),
    ))?;
    let per_core = s.loaded.config.machine.load.per_core;
    let sample = || kr_machine::read_hardware(s.runner).ok();
    let before = sample();
    let mut r = run_item_once(s, item, device, "")?;
    let after = sample();
    let high = [&before, &after]
        .into_iter()
        .flatten()
        .find_map(|hw| hw.overloaded(per_core));
    let state = |hw: &Option<kr_machine::Hardware>| {
        hw.as_ref()
            .map(|h| json!({ "load": h.load, "pressure": h.pressure }))
    };
    r.load = Some(json!({ "start": state(&before), "end": state(&after) }));
    if let (false, Some(reason)) = (r.passed, &high) {
        let note = format!("failed during high load ({reason})");
        s.loaded.events().append(&Event::now(
            s.run_id,
            s.loaded.project(),
            EventKind::Rerun,
            json!({ "item": item.name, "whole_item": true, "reason": note }),
        ))?;
        let mut again = run_item_once(s, item, device, "again")?;
        if again.passed {
            again.flaky.push(format!(
                "{} (whole item: {note}; passed when run again)",
                item.name
            ));
            again.load = r.load.take();
            r = again;
        } else {
            r.notes.push(format!("{note}; failed again when run again"));
        }
    }
    r.high_load = high;
    r.duration_ms = started.elapsed().as_millis() as u64;
    item_event(s, &r)?;
    Ok(r)
}

/// One pass of an item on a device (or this Mac): enumerate, run, read results, rerun
/// failed tests one by one, judge. `round` names a repeat ("again").
fn run_item_once(
    s: &Shared<'_>,
    item: &WorkItem,
    device: Option<(&str, &str)>,
    round: &str,
) -> Result<ItemResult> {
    let started = Instant::now();
    let mut r = ItemResult::new(&item.name, item.kind);
    r.device = device.map(|d| d.0.to_owned());
    let job = &s.jobs[item.job];
    // Paths relative to the job's directory keep command lines independent of where
    // the repo lives.
    let cwd = s.root.join(&job.dir);
    let products = Path::new(check::DERIVED_DATA).join("Build/Products");
    let xctestrun = XcTestRun::find(&cwd.join(&products))?;
    let xctestrun = products.join(xctestrun.file_name().unwrap_or_default());
    let destination = match device {
        Some((_, udid)) => format!("id={udid}"),
        None => "platform=macOS".to_owned(),
    };
    let name = if round.is_empty() {
        item.name.clone()
    } else {
        format!("{}-{round}", item.name)
    };
    let dir = results_dir(&name);
    let _ = std::fs::remove_dir_all(cwd.join(&dir));
    std::fs::create_dir_all(cwd.join(&dir))?;
    let logs = s.loaded.run_dir(s.run_id).join(&name);
    let filters: Vec<String> = item
        .only
        .iter()
        .map(|t| format!("-only-testing:{t}"))
        .chain(item.skip.iter().map(|t| format!("-skip-testing:{t}")))
        .collect();
    let base = |action: &str| {
        Xcodebuild::new(action)
            .xctestrun(&xctestrun)
            .destination(&destination)
    };

    // What will run: the real enumeration on this device is the authority.
    let enum_cmd = base("test-without-building")
        .args([
            "-enumerate-tests",
            "-test-enumeration-style",
            "flat",
            "-test-enumeration-format",
            "json",
        ])
        .args(filters.iter().cloned())
        .command();
    let enum_out = s.runner.run_logged(
        &with_xcode(enum_cmd, s.developer_dir)
            .current_dir(&cwd)
            .timeout(item.timeout),
        &logs.join("enumerate.log"),
    )?;
    let enumeration = json_tail(&enum_out.stdout)
        .and_then(|j| Enumeration::parse(j).ok())
        .unwrap_or_default();
    for f in &item.only {
        if !enumeration.hits(f) {
            let note = format!("`{f}` passed the static check but selects no test on the device");
            s.loaded.events().append(&Event::now(
                s.run_id,
                s.loaded.project(),
                EventKind::FilterMismatch,
                json!({ "item": item.name, "filter": f }),
            ))?;
            r.notes.push(note);
        }
    }

    let attempt = |name: &str, extra: Vec<String>| -> Result<(Attempt, PathBuf)> {
        let bundle = dir.join(format!("{name}.xcresult"));
        let cmd = base("test-without-building")
            .args(extra)
            .args(["-resultBundlePath".to_owned(), bundle.display().to_string()])
            .command();
        let out = s.runner.run_logged(
            &with_xcode(cmd, s.developer_dir)
                .current_dir(&cwd)
                .timeout(item.timeout),
            &logs.join(format!("{name}.log")),
        )?;
        let read = s
            .runner
            .run(&with_xcode(results_command(&bundle), s.developer_dir).current_dir(&cwd))?;
        let results = read
            .success()
            .then(|| parse_test_results(&read.stdout).ok())
            .flatten();
        Ok((
            Attempt {
                exit: out.status,
                timed_out: out.timed_out,
                results,
            },
            bundle,
        ))
    };

    let verdict = if enumeration.enabled.is_empty() {
        judge(
            &enumeration,
            &Attempt {
                exit: Some(0),
                timed_out: enum_out.timed_out,
                results: Some(Vec::new()),
            },
            &[],
        )
    } else {
        let (first, bundle) = attempt("run", filters.clone())?;
        r.xcresult = Some(cwd.join(&bundle));
        let mut reruns = Vec::new();
        for (n, test) in Verdict::to_rerun(&first).iter().enumerate() {
            let (again, _) = attempt(
                &format!("rerun-{}", n + 1),
                vec![format!("-only-testing:{}", only_testing_id(test))],
            )?;
            let passed = again.results.as_ref().is_some_and(|r| {
                r.iter()
                    .any(|c| &c.id == test && c.outcome != kr_xcode::TestOutcome::Failed)
            });
            s.loaded.events().append(&Event::now(
                s.run_id,
                s.loaded.project(),
                EventKind::Rerun,
                json!({ "item": item.name, "test": test, "passed": passed }),
            ))?;
            reruns.push(Some(again));
        }
        judge(&enumeration, &first, &reruns)
    };

    r.passed = verdict.passed();
    r.summary = if r.passed {
        Vec::new()
    } else {
        verdict.summary()
    };
    r.flaky = verdict
        .tests
        .iter()
        .filter(|t| t.outcome == Final::Flaky)
        .map(|t| t.id.clone())
        .collect();
    r.tests = Some(
        [
            ("passed", verdict.count(Final::Passed)),
            ("failed", verdict.count(Final::Failed)),
            ("flaky", verdict.count(Final::Flaky)),
            ("skipped", verdict.count(Final::Skipped)),
        ]
        .into_iter()
        .collect(),
    );
    r.log = Some(logs);
    r.duration_ms = started.elapsed().as_millis() as u64;
    Ok(r)
}

/// The JSON document at the end of xcodebuild's enumeration output.
fn json_tail(text: &str) -> Option<&str> {
    let start = text
        .find("\n{")
        .map(|i| i + 1)
        .or_else(|| text.starts_with('{').then_some(0))?;
    Some(&text[start..])
}

fn report_build_failure(ctx: &Ctx, label: &str, steps: &[check::StepResult]) -> Result<()> {
    if ctx.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({ "ok": false, "label": label, "build": steps, "items": [] })
            )?
        );
        return Ok(());
    }
    let failed = steps
        .iter()
        .find(|s| !s.ok)
        .context("a build step failed")?;
    println!("FAIL: {label} · {} failed; nothing ran", failed.step);
    for line in &failed.summary {
        println!("  {line}");
    }
    if let Some(log) = &failed.log {
        println!("  log: {}", log.display());
    }
    Ok(())
}

fn report(
    ctx: &Ctx,
    label: &str,
    steps: &Steps<'_>,
    results: &[ItemResult],
    ok: bool,
    elapsed: Duration,
) -> Result<()> {
    let passed = results.iter().filter(|r| r.passed).count();
    let build = if steps.results.iter().any(|s| s.step.starts_with("build")) {
        let ms: u64 = steps
            .results
            .iter()
            .filter(|s| s.step.starts_with("build"))
            .map(|s| s.duration_ms)
            .sum();
        format!("built in {}", human_duration(Duration::from_millis(ms)))
    } else {
        "build cached".to_owned()
    };
    if ctx.json {
        let out = json!({
            "ok": ok,
            "label": label,
            "passed": passed,
            "total": results.len(),
            "build": { "steps": steps.results, "cached": steps.cached },
            "items": results,
            "duration_ms": elapsed.as_millis() as u64,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    for r in results {
        let dur = human_duration(Duration::from_millis(r.duration_ms));
        let counts = r.tests.as_ref().map(|t| {
            let mut parts = vec![format!("{} tests", t.values().sum::<usize>())];
            for (k, label) in [("flaky", "flaky"), ("skipped", "skipped")] {
                if t[k] > 0 {
                    parts.push(format!("{} {label}", t[k]));
                }
            }
            parts.join(", ")
        });
        let detail = match (&counts, r.device.as_deref()) {
            (Some(c), Some(d)) => format!("{c}, {d}, {dur}"),
            (Some(c), None) => format!("{c}, {dur}"),
            (None, _) => dur,
        };
        if r.passed {
            println!("PASS: {} ({detail})", r.name);
            if let Some(h) = &r.high_load {
                println!("  high load: {h}");
            }
            for f in &r.flaky {
                println!("  flaky: {f}");
            }
        } else {
            println!("FAIL: {} ({detail})", r.name);
            for line in r.summary.iter().take(3) {
                println!("  {line}");
            }
            if let Some(x) = &r.xcresult {
                println!("  xcresult: {}", x.display());
            } else if let Some(log) = &r.log {
                println!("  log: {}", log.display());
            }
        }
        for note in &r.notes {
            println!("  note: {note}");
        }
    }
    println!(
        "{label}: {passed}/{} PASS · {build} · {}",
        results.len(),
        human_duration(elapsed)
    );
    Ok(())
}
