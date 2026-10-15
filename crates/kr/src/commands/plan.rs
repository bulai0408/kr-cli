//! `kr plan` (ADR 0010, level 1): what `kr run` / `kr gate` would do, without doing
//! it — which builds are cached, why each item runs, the lanes, and how long it should
//! take by history. It runs only `git ls-files` (for the input hashes) and records
//! nothing.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Args;
use jiff::Timestamp;
use kr_config::Suite;
use kr_events::EventKind;
use kr_resource::Resource;
use kr_run::longest_first;
use kr_run::schedule::{self, Order};
use serde::Serialize;

use super::check;
use super::human_duration;
use super::run::{durations, is_ipad, work_items};
use crate::{Ctx, Loaded};

#[derive(Debug, Args)]
#[command(
    after_help = "Exit codes: 0 the plan is computed (`has_work` in --json says whether there is work); with --exit-code, 0 nothing to do and 3 work to do; 1 kr failed; 2 bad arguments."
)]
pub struct PlanArgs {
    #[command(flatten)]
    selection: crate::selection::SelectionArgs,
    /// Scenarios and suites, as for `kr run`. Without any: the gate.
    targets: Vec<String>,

    /// The gate to plan, as for `kr gate`. Default: `default`.
    #[arg(long, conflicts_with = "targets")]
    gate: Option<String>,

    /// Exit 3 when there is work to do (something to build or run), 0 when there is
    /// none, like `git diff --exit-code`.
    #[arg(long)]
    exit_code: bool,
}

const SCHEMA_VERSION: u32 = 1;
/// Exit code with `--exit-code` when there is work to do (ADR 0011 supplement).
const WORK_TO_DO: u8 = 3;
const DEFAULT_BOOT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
struct Plan {
    schema_version: u32,
    /// "run" or "gate <name>", as `kr run` / `kr gate` would report.
    label: String,
    /// Something to build or run; with `--exit-code` the exit code says the same.
    has_work: bool,
    build: Vec<Build>,
    items: Vec<Item>,
    sim_capacity: usize,
    /// Device kind of each simulator lane, in the order they queue for a slot.
    lanes: Vec<String>,
    estimate: Estimate,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection: Option<crate::selection::SelectionReport>,
    decisions: Vec<kr_run::decision::Decision>,
}

#[derive(Debug, Serialize)]
struct Build {
    job: String,
    cached: bool,
    /// Why it builds; `None` when cached.
    why: Option<&'static str>,
    estimate_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct Item {
    name: String,
    /// "scenario" or "suite".
    kind: &'static str,
    /// "test", "command" or "build" (passes when its build does).
    step: &'static str,
    /// Simulator device kind; `None` on this Mac.
    device: Option<String>,
    why: String,
    /// Mean of its recent runs; `None` without history.
    estimate_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct Estimate {
    /// Builds, then command suites and Mac items, then the simulator lanes. `None`
    /// when something has no history.
    total_ms: Option<u64>,
    without_history: Vec<String>,
}

pub fn run(ctx: &Ctx, args: PlanArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let (project, root) = loaded.require_project()?;
    let (mut sel, label, why) = if args.targets.is_empty() {
        let name = args.gate.unwrap_or_else(|| "default".to_owned());
        if !project.gates.contains_key(&name) {
            let known: Vec<&str> = project.gates.keys().map(String::as_str).collect();
            bail!("no gate `{name}` in .kr.toml; known: {}", known.join(", "));
        }
        let sel = check::select(project, &[], Some(&name))?;
        (sel, format!("gate {name}"), format!("in gate `{name}`"))
    } else {
        let sel = check::select(project, &args.targets, None)?;
        (
            sel,
            "run".to_owned(),
            "named on the command line".to_owned(),
        )
    };
    crate::selection::apply(ctx, &loaded, &mut sel, &args.targets, &args.selection)?;
    let runner = ctx.runner()?;
    let developer_dir = loaded.developer_dir(ctx)?;
    let jobs = check::build_jobs(project, &sel)?;
    let hashes = check::job_hashes(
        project,
        &root,
        &jobs,
        runner.as_ref(),
        developer_dir.as_deref(),
    );
    let history = durations(&loaded);
    let (boot, built) = boot_and_builds(&loaded);
    let mut without_history = Vec::new();
    let mut total = Duration::ZERO;

    let mut build = Vec::new();
    for (job, hash) in jobs.iter().zip(&hashes) {
        let why = check::why_stale(job, &root, hash.as_deref());
        let estimate = why.and(built.get(&format!("build {}", job.label())).copied());
        match (why, estimate) {
            (Some(_), Some(e)) => total += e,
            (Some(_), None) => without_history.push(job.label()),
            (None, _) => {}
        }
        build.push(Build {
            job: job.label(),
            cached: why.is_none(),
            why,
            estimate_ms: estimate.map(ms),
        });
    }

    let mut items = Vec::new();
    for name in sel.suites.iter().chain(&sel.scenarios) {
        let step = match project.suites.get(name) {
            Some(Suite::Build { .. }) => "build",
            Some(Suite::Command { .. }) => "command",
            _ => "test",
        };
        items.push(Item {
            name: name.clone(),
            kind: if project.scenarios.contains_key(name) {
                "scenario"
            } else {
                "suite"
            },
            step,
            device: None,
            why: sel
                .selection
                .as_ref()
                .and_then(|s| s.reasons.get(name))
                .map(|r| r.join("; "))
                .unwrap_or_else(|| why.clone()),
            estimate_ms: None,
        });
    }
    let work = work_items(project, &sel, &jobs)?;
    for item in &mut items {
        if item.step == "build" {
            continue;
        }
        item.device = work
            .iter()
            .find(|w| w.name == item.name)
            .and_then(|w| w.device.clone());
        let estimate = history.get(&item.name).copied();
        item.estimate_ms = estimate.map(ms);
        match estimate {
            // Lanes are added below.
            Some(e) if item.device.is_none() => total += e,
            Some(_) => {}
            None => without_history.push(item.name.clone()),
        }
    }

    // The lanes as `kr run` would form them, every pool device assumed usable (see
    // `kr doctor`), and their time in virtual time.
    let capacity = loaded.capacity(Resource::Sim, runner.as_ref())? as usize;
    let devices: BTreeMap<String, usize> = project
        .simulators
        .devices
        .iter()
        .map(|(kind, k)| (kind.clone(), k.count as usize))
        .collect();
    let sim: Vec<(String, String, Duration)> = longest_first(
        work.into_iter()
            .filter_map(|w| {
                let device = w.device?;
                let estimate = history.get(&w.name).copied().unwrap_or_default();
                Some((w.name, device, estimate))
            })
            .collect(),
        |i| history.get(&i.0).copied(),
    );
    let lanes = schedule::lanes(
        schedule::queues(sim.clone(), |i| i.1.clone()),
        &devices,
        capacity,
        Order::IpadFirst,
        |kind| is_ipad(project, kind),
    );
    let order = lanes.order.clone();
    if !sim.is_empty() {
        let (timeline, _) = schedule::simulate(lanes, capacity, boot, |i| i.2);
        total += boot + timeline.makespan().unwrap_or_default();
    }

    let has_work = build.iter().any(|b| !b.cached) || items.iter().any(|i| i.step != "build");
    let plan = Plan {
        schema_version: SCHEMA_VERSION,
        label,
        has_work,
        build,
        items,
        sim_capacity: capacity,
        lanes: order,
        estimate: Estimate {
            total_ms: without_history.is_empty().then(|| ms(total)),
            without_history,
        },
        decisions: sel
            .selection
            .as_ref()
            .map(|s| s.decisions.clone())
            .unwrap_or_default(),
        selection: sel.selection,
    };
    if ctx.json {
        crate::output::print_json(&plan)?;
    } else {
        print!("{}", human(&plan));
    }
    Ok(if has_work && args.exit_code {
        ExitCode::from(WORK_TO_DO)
    } else {
        ExitCode::SUCCESS
    })
}

/// Mean lease boot time and mean duration of each build step, from events of the past
/// 14 days (like the item durations of `kr run`).
fn boot_and_builds(loaded: &Loaded) -> (Duration, BTreeMap<String, Duration>) {
    let since = Timestamp::now() - jiff::SignedDuration::from_hours(24 * 14);
    let mut boots = Vec::new();
    let mut builds: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for e in loaded.events().read_since(since).unwrap_or_default() {
        match e.kind {
            EventKind::SimLeased => boots.extend(e.fields["boot_ms"].as_u64()),
            EventKind::StepEnd if e.fields["ok"] == true => {
                if let (Some(step), Some(ms)) =
                    (e.fields["step"].as_str(), e.fields["duration_ms"].as_u64())
                    && step.starts_with("build ")
                {
                    builds.entry(step.to_owned()).or_default().push(ms);
                }
            }
            _ => {}
        }
    }
    let mean = |v: &[u64]| Duration::from_millis(v.iter().sum::<u64>() / v.len() as u64);
    let boot = if boots.is_empty() {
        DEFAULT_BOOT
    } else {
        mean(&boots)
    };
    (
        boot,
        builds.into_iter().map(|(k, v)| (k, mean(&v))).collect(),
    )
}

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

fn about(ms: u64) -> String {
    human_duration(Duration::from_millis(ms))
}

fn human(p: &Plan) -> String {
    let stale = p.build.iter().filter(|b| !b.cached).count();
    let build = match stale {
        0 => "build cached".to_owned(),
        1 => "builds 1 job".to_owned(),
        n => format!("builds {n} jobs"),
    };
    let runs = p.items.iter().filter(|i| i.step != "build").count();
    let lanes = if p.lanes.is_empty() {
        String::new()
    } else {
        format!(" on {} lanes ({})", p.lanes.len(), p.lanes.join(", "))
    };
    let estimate = match p.estimate.total_ms {
        Some(ms) => format!(" · about {}", about(ms)),
        None if runs > 0 || stale > 0 => format!(
            " · no estimate: no history for {}",
            p.estimate.without_history.join(", ")
        ),
        None => String::new(),
    };
    let mut out = format!(
        "{}: {} · {build} · {runs} item{}{lanes}{estimate}\n",
        if p.has_work {
            "work to do"
        } else {
            "nothing to do"
        },
        p.label,
        if runs == 1 { "" } else { "s" },
    );
    for b in &p.build {
        let state = match (b.why, b.estimate_ms) {
            (None, _) => "cached".to_owned(),
            (Some(why), Some(ms)) => format!("builds: {why}, ~{}", about(ms)),
            (Some(why), None) => format!("builds: {why}"),
        };
        out += &format!("  build {}: {state}\n", b.job);
    }
    for i in &p.items {
        let place = match (&i.device, i.step) {
            (_, "build") => "passes when its build does".to_owned(),
            (Some(d), _) => d.clone(),
            (None, _) => "this Mac".to_owned(),
        };
        let estimate = i.estimate_ms.map_or_else(
            || ", no history".to_owned(),
            |ms| format!(", ~{}", about(ms)),
        );
        let estimate = if i.step == "build" { "" } else { &estimate };
        out += &format!("  {} ({}, {place}): {}{estimate}\n", i.name, i.kind, i.why);
    }
    if let Some(s) = &p.selection {
        if s.explain {
            for (file, reason) in &s.ignored {
                out += &format!("  {file}: {reason}\n");
            }
        }
        for d in &s.decisions {
            out += &format!(
                "REVIEW: {}: {} (default: {}, pending review)\n",
                d.kind, d.question, d.default.option
            );
        }
    }
    out
}
