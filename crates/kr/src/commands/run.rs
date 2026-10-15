//! `kr run` and `kr gate` (spec §6, ADR 0005): build once, then run each scenario or
//! suite on leased pool simulators, check the results against the enumeration, rerun
//! failures once, and report PASS/FAIL per item.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Mutex, OnceLock, mpsc};
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
use kr_run::rejudge::{self, Saved};
use kr_run::schedule::{self, Action, Done, Executor, LaneId, Order, WallClock};
use kr_run::{Attempt, Final, Verdict, judge, longest_first};
use kr_sim::{DeviceLocks, Lease, LeaseHolder, PoolDevice, Simctl};
use kr_xcode::{
    Enumeration, XcTestRun, Xcodebuild, only_testing_id, parse_test_results, results_command,
};
use serde::Serialize;
use serde_json::json;

use super::check::{self, BuildJob, Selection, Steps, with_xcode};
use super::human_duration;
use super::remote;
use crate::{Ctx, Loaded, slots};

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Scenarios and suites to run.
    #[arg(required = true)]
    targets: Vec<String>,

    /// Run on this remote machine (`[remotes.<name>]` of the machine config) instead:
    /// its kr fetches the pushed branch and runs there; the result comes back here.
    #[arg(long, value_name = "NAME")]
    remote: Option<String>,
}

#[derive(Debug, Args)]
pub struct GateArgs {
    /// The gate to run. Default: `default`.
    name: Option<String>,

    /// Run on this remote machine, like `kr run --remote`.
    #[arg(long, value_name = "NAME")]
    remote: Option<String>,
}

/// Where results go, relative to the repo root; the latest run of each item wins.
const RESULTS: &str = "build/kr/results";
const LEASE_POLL: Duration = Duration::from_secs(2);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub fn run(ctx: &Ctx, args: RunArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let (project, root) = loaded.require_project()?;
    let sel = check::select(project, &args.targets, None)?;
    if let Some(host) = &args.remote {
        let kr_args = std::iter::once("run".to_owned())
            .chain(args.targets)
            .collect();
        return remote::execute(ctx, &loaded, project, &root, &sel, host, kr_args);
    }
    remote::refuse_signing_in_ssh_session(ctx, project, &sel)?;
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
    if let Some(host) = &args.remote {
        let kr_args = vec!["gate".to_owned(), name];
        return remote::execute(ctx, &loaded, project, &root, &sel, host, kr_args);
    }
    remote::refuse_signing_in_ssh_session(ctx, project, &sel)?;
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
    /// The build job of a test item, for its input hash in the events.
    #[serde(skip)]
    job: Option<usize>,
    /// Each test of a test item: outcome, time, failure signature and category.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cases: Vec<kr_run::Case>,
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
            job: None,
            cases: Vec::new(),
        }
    }
}

/// A test run to place on a simulator (or on this Mac).
#[derive(Debug, Clone)]
pub(crate) struct WorkItem {
    pub(crate) name: String,
    pub(crate) kind: &'static str,
    /// Pool device kind; `None` runs on this Mac.
    pub(crate) device: Option<String>,
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
    /// Input hash of each job, recorded again after a rebuild.
    hashes: &'a [Option<String>],
    runner: &'a dyn Runner,
    developer_dir: Option<&'a Path>,
    run_id: RunId,
    /// What the run's items ran with, without the item's own part (see [`identity_of`]).
    identity: serde_json::Value,
    /// The pool, once the simulator phase has read it.
    pool: OnceLock<Vec<PoolDevice>>,
    /// Jobs whose missing test products this run has rebuilt (once each).
    rebuilt: Mutex<BTreeSet<usize>>,
    /// Items a stop signal ended midway.
    interrupted: Mutex<Vec<String>>,
}

/// Runs, and if a stop signal (SIGINT, SIGTERM, SIGHUP) ends it, records that, reports
/// what finished and exits with 128 + the signal number.
fn execute(
    ctx: &Ctx,
    loaded: &Loaded,
    project: &ProjectConfig,
    root: &Path,
    sel: &Selection,
    label: String,
) -> Result<ExitCode> {
    kr_exec::catch_stop_signals()?;
    let run_id = RunId::new();
    let result = execute_run(ctx, loaded, project, root, sel, &label, run_id);
    match (result, kr_exec::stop_signal()) {
        // Stopped before any item ran, e.g. while building.
        (Err(e), Some(signal)) => {
            tracing::info!("stopped: {e:#}");
            let all: Vec<String> = sel.suites.iter().chain(&sel.scenarios).cloned().collect();
            let stop = Stopped {
                signal,
                interrupted: Vec::new(),
                not_started: all,
            };
            stop.record(loaded, run_id, &[])?;
            stop.report(ctx, &label, &[])?;
            Ok(stop.exit_code())
        }
        (Err(e), None) => {
            run_end(
                loaded,
                run_id,
                &label,
                &json!({ "ok": false, "error": format!("{e:#}") }),
            )?;
            Err(e)
        }
        (Ok(code), _) => Ok(code),
    }
}

/// Every run that is not aborted ends with this event, also when kr itself fails, so a
/// run that did not finish is visible.
fn run_end(loaded: &Loaded, run_id: RunId, label: &str, fields: &serde_json::Value) -> Result<()> {
    let mut fields = fields.clone();
    fields["label"] = json!(label);
    loaded.events().append(&Event::now(
        run_id,
        loaded.project(),
        EventKind::RunEnd,
        fields,
    ))
}

/// The first event of a run: what it runs and who asked for it (ADR 0010). `previous`
/// is the last run from the same worktree, to tell whether this request waited for
/// that one's result.
pub(crate) fn run_start(
    ctx: &Ctx,
    loaded: &Loaded,
    root: &Path,
    sel: &Selection,
    label: &str,
    run_id: RunId,
) -> Result<()> {
    let worktree = root.display().to_string();
    let events = loaded
        .events()
        .read_since(Timestamp::now() - jiff::SignedDuration::from_hours(24))
        .unwrap_or_default();
    let previous = events
        .iter()
        .rfind(|e| {
            e.kind == EventKind::RunStart && e.fields["requester"]["worktree"] == *worktree
        })
        .map(|start| {
            let ended = events.iter().find(|e| {
                e.run_id == start.run_id
                    && matches!(e.kind, EventKind::RunEnd | EventKind::RunAborted)
            });
            json!({
                "run_id": start.run_id,
                "ended_ms_ago": ended.map(|e| Timestamp::now().duration_since(e.ts).as_millis().max(0) as u64),
            })
        });
    loaded.events().append(&Event::now(
        run_id,
        loaded.project(),
        EventKind::RunStart,
        json!({
            "label": label,
            "targets": sel.suites.iter().chain(&sel.scenarios).collect::<Vec<_>>(),
            "requester": {
                "agent": ctx.env_var("KR_REQUESTER").or_else(|| ctx.env_var("AI_AGENT")),
                "worktree": worktree,
                // SAFETY: getppid has no preconditions.
                "parent_pid": unsafe { libc::getppid() },
            },
            "previous": previous,
        }),
    ))
}

/// What every item of the run ran with (ADR 0010): the code (git tree), Xcode, kr and
/// the effective configuration. A value kr cannot find is null.
fn identity(
    ctx: &Ctx,
    loaded: &Loaded,
    root: &Path,
    runner: &dyn Runner,
    developer_dir: Option<&Path>,
) -> serde_json::Value {
    let tree = runner
        .run(
            &Command::new("git")
                .args(["rev-parse", "HEAD^{tree}"])
                .current_dir(root),
        )
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout.trim().to_owned());
    let xcode = developer_dir.and_then(|dir| {
        kr_machine::find_xcodes(&ctx.applications_dir())
            .into_iter()
            .find(|x| x.developer_dir() == dir)
            .map(|x| json!({ "version": x.version, "build": x.build }))
    });
    let config_hash = serde_json::to_vec(&loaded.config)
        .ok()
        .map(|c| blake3::hash(&c).to_hex().to_string());
    json!({
        "tree": tree,
        "xcode": xcode,
        "kr_version": env!("CARGO_PKG_VERSION"),
        "config_hash": config_hash,
    })
}

const LOAD_SAMPLE: Duration = Duration::from_secs(15);

/// Records the machine load every [`LOAD_SAMPLE`] from its start until dropped, so
/// failures can be set against the load while they happened (ADR 0010).
struct LoadSampler {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LoadSampler {
    fn start(loaded: &Loaded, run_id: RunId) -> Self {
        let events = loaded.events();
        let project = loaded.project().map(str::to_owned);
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            loop {
                if let Some((load, pressure)) = kr_machine::sample_load() {
                    let _ = events.append(&Event::now(
                        run_id,
                        project.as_deref(),
                        EventKind::LoadSample,
                        json!({ "load": load, "pressure": pressure }),
                    ));
                }
                if stopped.recv_timeout(LOAD_SAMPLE) != Err(mpsc::RecvTimeoutError::Timeout) {
                    break;
                }
            }
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for LoadSampler {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// How a stop signal left the run.
struct Stopped {
    signal: i32,
    interrupted: Vec<String>,
    not_started: Vec<String>,
}

impl Stopped {
    fn name(&self) -> String {
        match self.signal {
            libc::SIGINT => "SIGINT".to_owned(),
            libc::SIGTERM => "SIGTERM".to_owned(),
            libc::SIGHUP => "SIGHUP".to_owned(),
            n => format!("signal {n}"),
        }
    }

    fn exit_code(&self) -> ExitCode {
        ExitCode::from(u8::try_from(128 + self.signal).unwrap_or(1))
    }

    fn record(&self, loaded: &Loaded, run_id: RunId, finished: &[ItemResult]) -> Result<()> {
        loaded.events().append(&Event::now(
            run_id,
            loaded.project(),
            EventKind::RunAborted,
            json!({
                "signal": self.name(),
                "interrupted": self.interrupted,
                "not_started": self.not_started,
                "finished": finished.iter().map(|r| &r.name).collect::<Vec<_>>(),
            }),
        ))
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "signal": self.name(),
            "interrupted": self.interrupted,
            "not_started": self.not_started,
        })
    }

    /// For the human report: `ABORTED: <label> (SIGTERM) · interrupted: … · not started: …`.
    fn line(&self, label: &str) -> String {
        let mut line = format!("ABORTED: {label} ({})", self.name());
        if !self.interrupted.is_empty() {
            line += &format!(" · interrupted: {}", self.interrupted.join(", "));
        }
        if !self.not_started.is_empty() {
            line += &format!(" · not started: {}", self.not_started.join(", "));
        }
        line
    }

    /// The report when nothing finished.
    fn report(&self, ctx: &Ctx, label: &str, results: &[ItemResult]) -> Result<()> {
        if ctx.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "ok": false,
                    "label": label,
                    "items": results,
                    "aborted": self.json(),
                }))?
            );
        } else {
            println!("{}", self.line(label));
        }
        Ok(())
    }
}

fn execute_run(
    ctx: &Ctx,
    loaded: &Loaded,
    project: &ProjectConfig,
    root: &Path,
    sel: &Selection,
    label: &str,
    run_id: RunId,
) -> Result<ExitCode> {
    let started = Instant::now();
    let runner = ctx.runner()?;
    run_start(ctx, loaded, root, sel, label, run_id)?;
    let _sampler = LoadSampler::start(loaded, run_id);
    let developer_dir = loaded.developer_dir(ctx)?;
    let identity = identity(ctx, loaded, root, runner.as_ref(), developer_dir.as_deref());
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
        run_end(
            loaded,
            run_id,
            label,
            &json!({ "ok": false, "passed": 0, "total": 0, "build_failed": true, "duration_ms": started.elapsed().as_millis() as u64 }),
        )?;
        report_build_failure(ctx, label, &steps.results)?;
        return Ok(ExitCode::FAILURE);
    }

    let shared = Shared {
        ctx,
        loaded,
        project,
        root,
        jobs: &jobs,
        hashes: &hashes,
        runner: runner.as_ref(),
        developer_dir: developer_dir.as_deref(),
        run_id,
        identity,
        pool: OnceLock::new(),
        rebuilt: Mutex::new(BTreeSet::new()),
        interrupted: Mutex::new(Vec::new()),
    };
    let mut results: Vec<ItemResult> = Vec::new();
    let mut not_started: Vec<String> = Vec::new();
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
            if kr_exec::stop_signal().is_some() {
                not_started.push(name.clone());
                continue;
            }
            match run_command_suite(&shared, name, run, dir.as_deref(), timeout.as_deref()) {
                Ok(r) => results.push(r),
                Err(_) if kr_exec::stop_signal().is_some() => {
                    shared.interrupted.lock().unwrap().push(name.clone());
                }
                Err(e) => results.push(failed_item(&shared, name, "suite", None, &e)?),
            }
        }
    }
    let items = work_items(project, sel, &jobs)?;
    let (done, left) = run_items(&shared, items)?;
    results.extend(done);
    not_started.extend(left);

    // Report in the order of the selection.
    let order: Vec<&String> = sel.suites.iter().chain(&sel.scenarios).collect();
    results.sort_by_key(|r| {
        order
            .iter()
            .position(|n| **n == r.name)
            .unwrap_or(usize::MAX)
    });
    if let Some(signal) = kr_exec::stop_signal() {
        let stop = Stopped {
            signal,
            interrupted: shared.interrupted.into_inner().unwrap(),
            not_started,
        };
        stop.record(loaded, run_id, &results)?;
        report(
            ctx,
            label,
            &steps,
            &results,
            false,
            started.elapsed(),
            Some(&stop),
        )?;
        return Ok(stop.exit_code());
    }
    let ok = results.iter().all(|r| r.passed);
    run_end(
        loaded,
        run_id,
        label,
        &json!({
            "ok": ok,
            "passed": results.iter().filter(|r| r.passed).count(),
            "total": results.len(),
            "duration_ms": started.elapsed().as_millis() as u64,
        }),
    )?;
    report(ctx, label, &steps, &results, ok, started.elapsed(), None)?;
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub(crate) fn work_items(
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
/// once), longest first; Mac items one after another. An item that errors fails on its
/// own; a lane that errors fails the items it could not run. After a stop signal no
/// item starts: returns the results and the names of the items not started.
fn run_items(s: &Shared<'_>, items: Vec<WorkItem>) -> Result<(Vec<ItemResult>, Vec<String>)> {
    let history = durations(s.loaded);
    let items = longest_first(items, |i| history.get(&i.name).copied());
    let (mac, sim): (Vec<WorkItem>, Vec<WorkItem>) =
        items.into_iter().partition(|i| i.device.is_none());
    let mut results = Vec::new();
    let mut not_started = Vec::new();
    for item in &mac {
        if kr_exec::stop_signal().is_some() {
            not_started.push(item.name.clone());
            continue;
        }
        if let Some(r) = run_item_or_fail(s, item, None)? {
            results.push(r);
        }
    }
    if sim.is_empty() {
        return Ok((results, not_started));
    }
    if kr_exec::stop_signal().is_some() {
        not_started.extend(sim.into_iter().map(|i| i.name));
        return Ok((results, not_started));
    }
    let simctl = Simctl::new(s.runner, s.developer_dir.map(Path::to_path_buf));
    let pool = kr_sim::plan(s.project, &simctl.runtimes()?, &simctl.devices()?)?;
    let pool = s.pool.get_or_init(|| pool);
    let capacity = s.loaded.capacity(Resource::Sim, s.runner)? as usize;
    let mut collected = Vec::new();
    // Kinds without devices fail right away.
    let mut devices = BTreeMap::new();
    let mut queues = schedule::queues(sim, |i| i.device.clone().unwrap_or_default());
    queues.retain(|kind, queue| {
        let count = pool
            .iter()
            .filter(|d| d.kind == *kind && d.udid.is_some() && d.issue.is_none())
            .count();
        if count == 0 {
            for item in queue.iter() {
                let mut r = ItemResult::new(&item.name, item.kind);
                r.summary = vec![format!(
                    "no usable `{kind}` simulator in the pool; run `kr pool sync` and `kr doctor`"
                )];
                collected.push(r);
            }
        }
        devices.insert(kind.clone(), count);
        count > 0
    });
    let lanes = schedule::lanes(queues, &devices, capacity, Order::IpadFirst, |kind| {
        is_ipad(s.project, kind)
    });
    let locks = DeviceLocks::new(kr_sim::locks_dir(&s.loaded.state_dir));
    let outcome = std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        let mut exec = RealLanes {
            s,
            simctl: &simctl,
            pool,
            locks: &locks,
            estimates: &history,
            scope,
            kinds: lanes.order.clone(),
            held: lanes.order.iter().map(|_| Held::default()).collect(),
            tx,
            rx,
        };
        schedule::drive(lanes, &mut exec, &WallClock::start())
    });
    collected.extend(outcome.done);
    for (kind, left) in outcome.left {
        if kr_exec::stop_signal().is_some() {
            not_started.extend(left.into_iter().map(|i| i.name));
            continue;
        }
        // Every lane of this kind failed before the queue was empty.
        let why = outcome
            .errors
            .iter()
            .find(|(k, _)| *k == kind)
            .map_or_else(|| "no lane ran it".to_owned(), |(_, e)| format!("{e:#}"));
        for item in left {
            let mut r = ItemResult::new(&item.name, item.kind);
            r.summary = vec![format!("not run: the `{kind}` lane failed: {why}")];
            item_event(s, &r)?;
            collected.push(r);
        }
    }
    for (kind, e) in &outcome.errors {
        if kr_exec::stop_signal().is_none() {
            eprintln!("warning: a `{kind}` lane failed: {e:#}");
        }
    }
    results.extend(collected);
    Ok((results, not_started))
}

/// iPad kinds' lanes queue first (ADR 0010).
pub(crate) fn is_ipad(project: &ProjectConfig, kind: &str) -> bool {
    project
        .simulators
        .devices
        .get(kind)
        .is_some_and(|k| k.device_type.contains("iPad"))
}

/// The real lanes: `sim` slots from the resource layer, leased pool simulators and
/// [`run_item_or_fail`]. Actions that wait run on their own thread and report back.
struct RealLanes<'scope, 'env> {
    s: &'env Shared<'env>,
    simctl: &'env Simctl<'env>,
    pool: &'env [PoolDevice],
    locks: &'env DeviceLocks,
    /// Expected duration of each item, from history.
    estimates: &'env BTreeMap<String, Duration>,
    scope: &'scope std::thread::Scope<'scope, 'env>,
    kinds: Vec<String>,
    held: Vec<Held<'env>>,
    tx: mpsc::Sender<Report<'env>>,
    rx: mpsc::Receiver<Report<'env>>,
}

impl<'scope, 'env> RealLanes<'scope, 'env> {
    /// Runs `f` on its own thread and reports its end; a panic fails the lane instead
    /// of leaving the scheduler waiting.
    fn spawn(&self, lane: LaneId, f: impl FnOnce() -> Report<'env> + Send + 'scope) {
        let tx = self.tx.clone();
        self.scope.spawn(move || {
            let report =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
                    Report::done(Done::Failed(
                        lane,
                        anyhow::anyhow!("internal: the lane's thread panicked"),
                    ))
                });
            let _ = tx.send(report);
        });
    }
}

/// What a lane holds between its actions.
#[derive(Default)]
struct Held<'a> {
    taken: Option<slots::Taken>,
    lease: Option<(Lease<'a>, Instant)>,
}

/// An action's end, with what the lane now holds.
struct Report<'a> {
    done: Done<ItemResult>,
    taken: Option<slots::Taken>,
    lease: Option<(Lease<'a>, Instant)>,
}

impl<'a> Report<'a> {
    fn done(done: Done<ItemResult>) -> Self {
        Self {
            done,
            taken: None,
            lease: None,
        }
    }
}

impl<'scope, 'env> Executor for RealLanes<'scope, 'env> {
    type Item = WorkItem;
    type Output = ItemResult;

    fn start(&mut self, action: Action<WorkItem>) {
        let s = self.s;
        match action {
            Action::Take(lane) => {
                let task = format!("run {} lane", self.kinds[lane]);
                self.spawn(lane, move || {
                    match slots::take(s.ctx, s.loaded, s.runner, s.run_id, Resource::Sim, &task) {
                        Ok(taken) => Report {
                            taken: Some(taken),
                            ..Report::done(Done::Taken(lane))
                        },
                        Err(e) => Report::done(Done::Failed(lane, e)),
                    }
                });
            }
            Action::Lease(lane) => {
                let taken = self.held[lane]
                    .taken
                    .take()
                    .expect("a lane leases with a slot");
                let (simctl, locks, pool) = (self.simctl, self.locks, self.pool);
                let kind = self.kinds[lane].clone();
                self.spawn(lane, move || {
                    lease(s, simctl, locks, pool, &kind, lane, taken)
                });
            }
            Action::Run(lane, item) => {
                let (lease, _) = self.held[lane].lease.as_ref().expect("a lane runs leased");
                let (name, udid) = (lease.name().to_owned(), lease.udid().to_owned());
                self.spawn(lane, move || {
                    Report::done(match run_item_or_fail(s, &item, Some((&name, &udid))) {
                        Ok(r) => Done::Ran(lane, r),
                        Err(e) => Done::Failed(lane, e),
                    })
                });
            }
            Action::Release(lane, ok) => {
                let held = std::mem::take(&mut self.held[lane]);
                self.spawn(lane, move || Report::done(release(s, lane, held, ok)));
            }
            Action::GiveBack(lane) => {
                let taken = self.held[lane]
                    .taken
                    .take()
                    .expect("a lane gives back a slot");
                let done = match slots::give_back(s.loaded, s.run_id, taken, Some(0)) {
                    Ok(()) => Done::Released(lane),
                    Err(e) => Done::Failed(lane, e),
                };
                let _ = self.tx.send(Report::done(done));
            }
        }
    }

    fn wait(&mut self) -> Done<ItemResult> {
        let report = self.rx.recv().expect("lane threads report back");
        let lane = match &report.done {
            Done::Taken(l) | Done::Leased(l) | Done::Ran(l, _) | Done::Released(l) => *l,
            Done::Failed(l, _) => *l,
        };
        if report.taken.is_some() {
            self.held[lane].taken = report.taken;
        }
        if report.lease.is_some() {
            self.held[lane].lease = report.lease;
        }
        report.done
    }

    fn stopped(&self) -> bool {
        kr_exec::stop_signal().is_some()
    }

    fn chose(&mut self, lane: LaneId, queue: &VecDeque<WorkItem>) {
        let estimate =
            |item: &WorkItem| self.estimates.get(&item.name).map(|d| d.as_millis() as u64);
        let _ = self.s.loaded.events().append(&Event::now(
            self.s.run_id,
            self.s.loaded.project(),
            EventKind::ScheduleDecision,
            json!({
                "lane": lane,
                "kind": self.kinds[lane],
                "item": queue.front().map(|i| &i.name),
                "estimate_ms": queue.front().and_then(estimate),
                "candidates": queue
                    .iter()
                    .map(|i| json!({ "item": i.name, "estimate_ms": estimate(i) }))
                    .collect::<Vec<_>>(),
                "rule": "longest_first",
            }),
        ));
    }
}

/// Leases and boots a device for a lane that holds `taken`; gives the slot back if
/// that fails.
fn lease<'a>(
    s: &Shared<'_>,
    simctl: &'a Simctl<'a>,
    locks: &DeviceLocks,
    pool: &[PoolDevice],
    kind: &str,
    lane: LaneId,
    taken: slots::Taken,
) -> Report<'a> {
    let holder = LeaseHolder {
        project: s.loaded.project().map(str::to_owned),
        task: taken.task.clone(),
        pid: std::process::id(),
        started: Timestamp::now(),
    };
    let booting = Instant::now();
    let leased = kr_sim::lease(simctl, locks, pool, kind, &holder, LEASE_POLL, None).and_then(
        |lease| {
            s.loaded.events().append(&Event::now(
                s.run_id,
                s.loaded.project(),
                EventKind::SimLeased,
                json!({ "device": lease.name(), "udid": lease.udid(), "task": taken.task, "boot_ms": booting.elapsed().as_millis() as u64 }),
            ))?;
            Ok(lease)
        },
    );
    match leased {
        Ok(lease) => Report {
            done: Done::Leased(lane),
            taken: Some(taken),
            lease: Some((lease, Instant::now())),
        },
        Err(e) => {
            let done = match slots::give_back(s.loaded, s.run_id, taken, None) {
                Ok(()) => Done::Failed(lane, e),
                Err(back) => Done::Failed(lane, back),
            };
            Report::done(done)
        }
    }
}

/// Shuts the lane's device down and gives it and the slot back.
fn release(s: &Shared<'_>, lane: LaneId, held: Held<'_>, ok: bool) -> Done<ItemResult> {
    let mut result = Ok(());
    if let Some((lease, since)) = held.lease {
        let (name, udid) = (lease.name().to_owned(), lease.udid().to_owned());
        let released = lease.release();
        result = s
            .loaded
            .events()
            .append(&Event::now(
                s.run_id,
                s.loaded.project(),
                EventKind::SimReleased,
                json!({ "device": name, "udid": udid, "held_ms": since.elapsed().as_millis() as u64 }),
            ))
            .and(released);
    }
    if let Some(taken) = held.taken {
        let back = slots::give_back(s.loaded, s.run_id, taken, Some(if ok { 0 } else { 1 }));
        result = back.and(result);
    }
    match result {
        Ok(()) => Done::Released(lane),
        Err(e) => Done::Failed(lane, e),
    }
}

/// Mean duration of each item's recent runs, from events of the past 14 days.
pub(crate) fn durations(loaded: &Loaded) -> BTreeMap<String, Duration> {
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
            "xcresult": r.xcresult,
            "cases": r.cases,
            "identity": identity_of(s, r),
        }),
    ))
}

/// The run's identity with the item's part: its build inputs and its simulator.
fn identity_of(s: &Shared<'_>, r: &ItemResult) -> serde_json::Value {
    let device = s
        .pool
        .get()
        .and_then(|pool| pool.iter().find(|d| Some(&d.name) == r.device.as_ref()));
    let mut identity = s.identity.clone();
    identity["input_hash"] = json!(r.job.and_then(|j| s.hashes[j].clone()));
    identity["runtime"] = json!(device.map(|d| &d.runtime));
    identity["device_type"] = json!(device.map(|d| &d.device_type));
    identity
}

fn results_dir(item: &str) -> PathBuf {
    Path::new(RESULTS).join(item)
}

/// Runs one item; an error fails just this item (Krill #125: one error lost every
/// verdict of a finished run). `None` when a stop signal ended it midway.
fn run_item_or_fail(
    s: &Shared<'_>,
    item: &WorkItem,
    device: Option<(&str, &str)>,
) -> Result<Option<ItemResult>> {
    match run_item(s, item, device) {
        Ok(r) => Ok(Some(r)),
        Err(_) if kr_exec::stop_signal().is_some() => {
            s.interrupted.lock().unwrap().push(item.name.clone());
            Ok(None)
        }
        Err(e) => failed_item(s, &item.name, item.kind, device.map(|d| d.0), &e).map(Some),
    }
}

/// A failed item for an error that kept it from finishing; records its end.
fn failed_item(
    s: &Shared<'_>,
    name: &str,
    kind: &'static str,
    device: Option<&str>,
    error: &anyhow::Error,
) -> Result<ItemResult> {
    let mut r = ItemResult::new(name, kind);
    r.device = device.map(str::to_owned);
    r.summary = vec![format!("{error:#}")];
    r.log = Some(s.loaded.run_dir(s.run_id).join(name));
    item_event(s, &r)?;
    Ok(r)
}

/// Test products can disappear while a run waits (Krill: a workspace deleted midway).
/// Before each item: if they are gone, rebuild them once under a `build` slot.
/// `Ok(Some(note))` after a rebuild; `Err` says why the item cannot run and what to do.
fn ensure_products(s: &Shared<'_>, item: &WorkItem) -> Result<Option<String>, String> {
    let job = &s.jobs[item.job];
    let products = job.products(s.root);
    let present = || XcTestRun::find(&products, &job.scheme).is_ok();
    if present() {
        return Ok(None);
    }
    let rebuilt_note = "test products were missing before the item started; rebuilt them";
    // One lane rebuilds; the others wait here and find the products back.
    let mut rebuilt = s.rebuilt.lock().unwrap();
    if present() {
        return Ok(Some(rebuilt_note.to_owned()));
    }
    let still_missing = format!(
        "test products are still missing after rebuilding them once ({}); something deletes them during the run: stop it, run `kr check`, then run again",
        products.display()
    );
    if !rebuilt.insert(item.job) {
        return Err(still_missing);
    }
    let event = |kind, fields| {
        s.loaded
            .events()
            .append(&Event::now(s.run_id, s.loaded.project(), kind, fields))
            .map_err(|e| format!("{e:#}"))
    };
    event(
        EventKind::ProductsMissing,
        json!({ "item": item.name, "products": products }),
    )?;
    let mut steps = Steps {
        loaded: s.loaded,
        run_id: s.run_id,
        command: "run",
        results: Vec::new(),
        cached: Vec::new(),
    };
    let built = (|| -> Result<bool> {
        let task = format!("run rebuild {}", job.scheme);
        let taken = slots::take(s.ctx, s.loaded, s.runner, s.run_id, Resource::Build, &task)?;
        let todo = [(job, s.hashes[item.job].clone())];
        let result = check::build(
            s.project,
            s.root,
            &todo,
            s.runner,
            s.developer_dir,
            &mut steps,
        );
        slots::give_back(
            s.loaded,
            s.run_id,
            taken,
            Some(if matches!(result, Ok(true)) { 0 } else { 1 }),
        )?;
        result
    })();
    let ok = matches!(built, Ok(true)) && present();
    event(
        EventKind::ProductsRebuilt,
        json!({ "item": item.name, "ok": ok }),
    )?;
    match built {
        Err(e) => Err(format!(
            "test products were missing ({}) and rebuilding them failed: {e:#}; run `kr check`, then run again",
            products.display()
        )),
        Ok(false) => {
            let failed = steps.results.iter().find(|r| !r.ok);
            let log = failed
                .and_then(|r| r.log.as_ref())
                .map(|l| format!(" (log: {})", l.display()))
                .unwrap_or_default();
            Err(format!(
                "test products were missing ({}) and rebuilding them failed{log}; fix the build with `kr check`, then run again",
                products.display()
            ))
        }
        Ok(true) if ok => Ok(Some(rebuilt_note.to_owned())),
        Ok(true) => Err(still_missing),
    }
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
        json!({
            "item": item.name,
            "kind": item.kind,
            "device": device.map(|d| d.0),
            "concurrency": slots::concurrency(s.loaded),
        }),
    ))?;
    let rebuilt = match ensure_products(s, item) {
        Ok(note) => note,
        Err(why) => {
            let mut r = ItemResult::new(&item.name, item.kind);
            r.device = device.map(|d| d.0.to_owned());
            r.job = Some(item.job);
            r.summary = vec![why];
            r.duration_ms = started.elapsed().as_millis() as u64;
            item_event(s, &r)?;
            return Ok(r);
        }
    };
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
            json!({
                "item": item.name,
                "whole_item": true,
                "reason": note,
                "attempt": 2,
                "xcresult": s.root.join(&s.jobs[item.job].dir)
                    .join(results_dir(&format!("{}-again", item.name)))
                    .join("run.xcresult"),
            }),
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
    r.notes.extend(rebuilt);
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
    r.job = Some(item.job);
    let job = &s.jobs[item.job];
    // Paths relative to the job's directory keep command lines independent of where
    // the repo lives.
    let cwd = s.root.join(&job.dir);
    let products = Path::new(check::DERIVED_DATA).join("Build/Products");
    let xctestrun = XcTestRun::find(&cwd.join(&products), &job.scheme)?;
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
    let enumerate = |filters: &[String], log: &Path| -> Result<(Enumeration, bool)> {
        let cmd = base("test-without-building")
            .args([
                "-enumerate-tests",
                "-test-enumeration-style",
                "flat",
                "-test-enumeration-format",
                "json",
            ])
            .args(filters.iter().cloned())
            .command();
        let out = s.runner.run_logged(
            &with_xcode(cmd, s.developer_dir)
                .current_dir(&cwd)
                .timeout(item.timeout),
            log,
        )?;
        // The whole log: the JSON of a big project is larger than the in-memory tail.
        let text = std::fs::read_to_string(log).unwrap_or_default();
        let enumeration = json_tail(&text)
            .and_then(|j| Enumeration::parse(j).ok())
            .unwrap_or_default();
        Ok((enumeration, out.timed_out))
    };
    let (enumeration, enum_timed_out) = enumerate(&filters, &logs.join("enumerate.log"))?;
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
                bundle: Some(cwd.join(&bundle)),
            },
            bundle,
        ))
    };

    let mut invalid: Vec<String> = Vec::new();
    let saved = if enumeration.enabled.is_empty() {
        let first = Attempt {
            exit: Some(0),
            timed_out: enum_timed_out,
            results: Some(Vec::new()),
            bundle: None,
        };
        save(&logs, &name, &enumeration, first, Vec::new())?
    } else {
        let (first, bundle) = attempt("run", filters.clone())?;
        r.xcresult = Some(cwd.join(&bundle));
        let mut reruns = Vec::new();
        for (n, test) in Verdict::to_rerun(&first).iter().enumerate() {
            let name = format!("rerun-{}", n + 1);
            // The filter follows the test's framework (Swift Testing keeps "()").
            let id = first
                .results
                .iter()
                .flatten()
                .find(|c| &c.id == test)
                .map_or_else(|| only_testing_id(test, None), |c| c.filter.clone());
            let filter = format!("-only-testing:{id}");
            // A filter that selects nothing would "rerun" zero tests (Krill #125).
            let (selected, _) = enumerate(
                std::slice::from_ref(&filter),
                &logs.join(format!("{name}-enumerate.log")),
            )?;
            if selected.enabled.is_empty() {
                invalid.push(format!(
                    "rerun filter invalid: `{filter}` selects 0 tests; {test} was not run again"
                ));
                s.loaded.events().append(&Event::now(
                    s.run_id,
                    s.loaded.project(),
                    EventKind::Rerun,
                    json!({ "item": item.name, "test": test, "passed": false, "filter": filter, "filter_invalid": true, "attempt": 2, "xcresult": null }),
                ))?;
                reruns.push(None);
                continue;
            }
            let (again, bundle) = attempt(&name, vec![filter.clone()])?;
            let passed = again.results.as_ref().is_some_and(|r| {
                r.iter()
                    .any(|c| &c.id == test && c.outcome != kr_xcode::TestOutcome::Failed)
            });
            s.loaded.events().append(&Event::now(
                s.run_id,
                s.loaded.project(),
                EventKind::Rerun,
                json!({ "item": item.name, "test": test, "passed": passed, "filter": filter, "filter_invalid": false, "attempt": 2, "xcresult": cwd.join(bundle) }),
            ))?;
            reruns.push(Some(again));
        }
        save(&logs, &name, &enumeration, first, reruns)?
    };
    let verdict = &saved.verdict;

    r.passed = verdict.passed();
    r.cases = kr_run::cases(verdict, saved.first.timed_out);
    r.summary = if r.passed {
        Vec::new()
    } else {
        invalid
            .into_iter()
            .chain(verdict.summary())
            .take(3)
            .collect()
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

/// Judges one pass and saves the verdict with what it came from as `<logs>/verdict.json`, small
/// enough to keep for every run, so `kr rejudge` can judge it again (ADR 0010).
fn save(
    logs: &Path,
    name: &str,
    enumeration: &Enumeration,
    first: Attempt,
    reruns: Vec<Option<Attempt>>,
) -> Result<Saved> {
    let saved = Saved {
        schema_version: rejudge::SCHEMA_VERSION,
        item: name.to_owned(),
        verdict: judge(enumeration, &first, &reruns),
        enumeration: enumeration.clone(),
        first,
        reruns,
    };
    std::fs::create_dir_all(logs)?;
    let file = logs.join("verdict.json");
    std::fs::write(&file, serde_json::to_vec(&saved)?)
        .with_context(|| format!("failed to write {}", file.display()))?;
    Ok(saved)
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
    stopped: Option<&Stopped>,
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
            "aborted": stopped.map(Stopped::json),
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
    if let Some(stop) = stopped {
        println!("{}", stop.line(label));
        return Ok(());
    }
    println!(
        "{label}: {passed}/{} PASS · {build} · {}",
        results.len(),
        human_duration(elapsed)
    );
    Ok(())
}
