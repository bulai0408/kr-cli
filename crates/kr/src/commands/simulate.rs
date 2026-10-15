//! `kr simulate` (ADR 0010, level 2): the simulator phase of recorded runs, replayed by
//! kr's own scheduler (`kr_run::schedule`) in virtual time, under other settings: `sim`
//! capacity, lane order, cache hit or miss.
//!
//! First every given run is replayed as it was recorded (its lanes, item order,
//! capacity and durations) and compared with its measured time: the calibration error.
//! Then the items of the latest run are simulated under each setting, with each item's
//! duration drawn from its recorded durations in all given runs.

use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use jiff::Timestamp;
use kr_events::{Event, EventKind, RunId};
use kr_run::longest_first;
use kr_run::schedule::{self, Order};
use serde::Serialize;

use super::run::is_ipad;
use crate::{Ctx, Loaded};

#[derive(Debug, Args)]
pub struct SimulateArgs {
    /// Recorded runs (IDs or unique starts) to take durations from; the items of the
    /// latest one are simulated. Default: the latest run with simulator items (of this
    /// project, inside a project).
    runs: Vec<String>,

    /// `sim` capacity to simulate; repeat to compare. Default: the latest run's.
    #[arg(long = "sim", value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    sims: Vec<u32>,

    /// Lane order; repeat to compare. Default: `ipad-first`, what `kr run` does.
    #[arg(long = "order", value_enum)]
    orders: Vec<LaneOrder>,

    /// `hit`: the test products are cached; `miss`: build first, for the recorded build
    /// time. Repeat to compare. Default: `hit`.
    #[arg(long = "cache", value_enum)]
    caches: Vec<Cache>,

    /// Simulated runs per setting, each with durations drawn from the recordings.
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range(1..))]
    iterations: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LaneOrder {
    /// iPad lanes queue first (kr run since #18).
    IpadFirst,
    /// Lanes by device kind name (kr run before #18, as far as thread timing allowed).
    Name,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cache {
    Hit,
    Miss,
}

const SCHEMA_VERSION: u32 = 1;
/// ADR 0010: the makespan error the simulator should stay within.
const TARGET: f64 = 0.10;
/// Boot time of a lane's device when no recorded run has one.
const DEFAULT_BOOT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    calibration: Calibration,
    /// What the settings simulate: the latest run's items.
    items: Items,
    iterations: u32,
    settings: Vec<Setting>,
}

#[derive(Debug, Serialize)]
struct Calibration {
    runs: Vec<Replay>,
    /// Mean absolute error of the replays.
    mape: f64,
    target: f64,
    /// With two runs or more: whether the replays order the runs like their measured
    /// times. `None` with one run.
    ranking_matches: Option<bool>,
}

#[derive(Debug, Serialize)]
struct Replay {
    run_id: RunId,
    project: Option<String>,
    items: usize,
    capacity: usize,
    /// Device kind of each lane, in the order they got a slot.
    lanes: Vec<String>,
    /// First simulator item start to last end.
    measured_ms: u64,
    simulated_ms: u64,
    /// (simulated − measured) ÷ measured.
    error: f64,
}

#[derive(Debug, Serialize)]
struct Items {
    run_id: RunId,
    count: usize,
    /// Fewest and most recorded durations of an item. With one each, p50 = p90.
    samples_min: usize,
    samples_max: usize,
}

#[derive(Debug, Serialize)]
struct Setting {
    sim: usize,
    order: LaneOrder,
    cache: Cache,
    /// Total time: the build (cache miss) plus the simulator phase.
    p50_ms: u64,
    p90_ms: u64,
    /// max(longest item, items ÷ simulators, per kind items ÷ its simulators), with
    /// mean durations; no order beats it.
    lower_bound_ms: u64,
    /// Share of lane time spent waiting for a `sim` slot (mean).
    queue_share: f64,
    /// Set when the setting is beyond what the runs were recorded with: then the
    /// result is a direction only (ADR 0010).
    outside_calibration: Option<String>,
}

/// One recorded run's simulator phase.
struct Recorded {
    run_id: RunId,
    project: Option<String>,
    start: Timestamp,
    /// (name, kind, duration) in the order the items started.
    items: Vec<(String, String, Duration)>,
    lanes: Vec<String>,
    capacity: usize,
    devices: BTreeMap<String, BTreeSet<String>>,
    boots: Vec<Duration>,
    build: Option<Duration>,
    measured: Duration,
}

pub fn run(ctx: &Ctx, args: SimulateArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let runs = recorded_runs(&loaded, &args.runs)?;
    let ipad = |kind: &str| match &loaded.config.project {
        Some(p) if p.simulators.devices.contains_key(kind) => is_ipad(p, kind),
        _ => kind.to_lowercase().contains("ipad"),
    };
    let report = simulate(&runs, &args, &ipad)?;
    if ctx.json {
        crate::output::print_json(&report)?;
    } else {
        print!("{}", human(&report));
    }
    Ok(ExitCode::SUCCESS)
}

/// The given runs (oldest first), or the latest one with simulator items.
fn recorded_runs(loaded: &Loaded, wanted: &[String]) -> Result<Vec<Recorded>> {
    let log = loaded.events();
    let mut by_run: BTreeMap<RunId, Vec<Event>> = BTreeMap::new();
    for e in log.read_since(Timestamp::UNIX_EPOCH)? {
        by_run.entry(e.run_id).or_default().push(e);
    }
    let mut runs: Vec<Recorded> = Vec::new();
    if wanted.is_empty() {
        let project = loaded.project();
        let latest = by_run
            .iter()
            .filter(|(_, events)| project.is_none() || events[0].project.as_deref() == project)
            .filter_map(|(id, events)| recorded(*id, events))
            .max_by_key(|r| r.start);
        runs.extend(latest);
        if runs.is_empty() {
            bail!(
                "no run with simulator items in the events under {}; run `kr gate` or `kr run` first, or give a run ID",
                log.dir().display()
            );
        }
        return Ok(runs);
    }
    for want in wanted {
        let found: Vec<RunId> = by_run
            .keys()
            .filter(|id| id.to_string().starts_with(want.as_str()))
            .copied()
            .collect();
        let id = match found.as_slice() {
            [id] => *id,
            [] => bail!(
                "no run `{want}` in the events under {}; `kr simulate` without an ID takes the latest run",
                log.dir().display()
            ),
            _ => bail!("`{want}` matches {} runs; give more of the ID", found.len()),
        };
        match recorded(id, &by_run[&id]) {
            Some(r) => runs.push(r),
            None => bail!(
                "run {id} has no simulator items to simulate; give a run of `kr gate` or `kr run`"
            ),
        }
    }
    runs.sort_by_key(|r| r.start);
    Ok(runs)
}

/// The simulator phase of a run from its events; `None` without simulator items.
fn recorded(run_id: RunId, events: &[Event]) -> Option<Recorded> {
    let device = |e: &Event| e.fields["device"].as_str().map(str::to_owned);
    let mut starts: BTreeMap<String, Timestamp> = BTreeMap::new();
    let mut ends: BTreeMap<String, (Timestamp, String, Duration)> = BTreeMap::new();
    let (mut lanes, mut capacity, mut boots, mut build) = (Vec::new(), None, Vec::new(), None);
    for e in events {
        let item = e.fields["item"].as_str().map(str::to_owned);
        let sim = e.fields["resource"] == "sim";
        match e.kind {
            EventKind::ItemStart => {
                if let (Some(item), Some(_)) = (item, device(e)) {
                    starts.entry(item).or_insert(e.ts);
                }
            }
            EventKind::ItemEnd => {
                if let (Some(item), Some(device), Some(ms)) =
                    (item, device(e), e.fields["duration_ms"].as_u64())
                {
                    ends.insert(item, (e.ts, device, Duration::from_millis(ms)));
                }
            }
            EventKind::QueueStart if sim => {
                capacity = capacity.or(e.fields["capacity"].as_u64());
            }
            EventKind::SlotAcquired if sim => {
                if let Some(kind) = e.fields["task"]
                    .as_str()
                    .and_then(|t| t.strip_prefix("run "))
                    .and_then(|t| t.strip_suffix(" lane"))
                {
                    lanes.push(kind.to_owned());
                }
            }
            EventKind::SimLeased => {
                boots.extend(e.fields["boot_ms"].as_u64().map(Duration::from_millis))
            }
            EventKind::StepEnd => {
                if e.fields["step"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("build"))
                    && let Some(ms) = e.fields["duration_ms"].as_u64()
                {
                    *build.get_or_insert(Duration::ZERO) += Duration::from_millis(ms);
                }
            }
            _ => {}
        }
    }
    if ends.is_empty() {
        return None;
    }
    let mut devices: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut items: Vec<(Timestamp, String, String, Duration)> = Vec::new();
    for (name, (end, device, took)) in ends {
        let kind = device_kind(&device);
        devices.entry(kind.clone()).or_default().insert(device);
        let start = starts
            .get(&name)
            .copied()
            .unwrap_or_else(|| end - jiff::SignedDuration::try_from(took).unwrap_or_default());
        items.push((start, name, kind, took));
    }
    items.sort_by_key(|i| i.0);
    let first = items.iter().map(|i| i.0).min()?;
    let last = events
        .iter()
        .filter(|e| e.kind == EventKind::ItemEnd && device(e).is_some())
        .map(|e| e.ts)
        .max()?;
    let measured = Duration::try_from(last.duration_since(first)).unwrap_or_default();
    let device_count: usize = devices.values().map(BTreeSet::len).sum();
    Some(Recorded {
        run_id,
        project: events[0].project.clone(),
        start: events[0].ts,
        items: items.into_iter().map(|(_, n, k, d)| (n, k, d)).collect(),
        lanes,
        capacity: capacity.map_or(device_count, |c| c as usize),
        devices,
        boots,
        build,
        measured,
    })
}

/// The device kind of a pool device name: `Krill-Verify-2` → `default`,
/// `Krill-Verify-iPad-1` → `iPad` (see `ProjectConfig::device_names`).
fn device_kind(name: &str) -> String {
    let rest = name.split_once("-Verify-").map_or(name, |(_, rest)| rest);
    if rest.parse::<u32>().is_ok() {
        return kr_config::DEFAULT_DEVICE.to_owned();
    }
    match rest.rsplit_once('-') {
        Some((kind, n)) if n.parse::<u32>().is_ok() => kind.to_owned(),
        _ => rest.to_owned(),
    }
}

fn mean(values: &[Duration]) -> Option<Duration> {
    (!values.is_empty()).then(|| values.iter().sum::<Duration>() / values.len() as u32)
}

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

fn simulate(runs: &[Recorded], args: &SimulateArgs, ipad: &dyn Fn(&str) -> bool) -> Result<Report> {
    let all_boots: Vec<Duration> = runs.iter().flat_map(|r| r.boots.clone()).collect();
    let boot = mean(&all_boots).unwrap_or(DEFAULT_BOOT);

    let mut replays = Vec::new();
    for r in runs {
        let queues = schedule::queues(r.items.clone(), |i| i.1.clone());
        let lanes = if r.lanes.is_empty() {
            let devices = r
                .devices
                .iter()
                .map(|(k, d)| (k.clone(), d.len()))
                .collect();
            schedule::lanes(queues, &devices, r.capacity, Order::IpadFirst, ipad)
        } else {
            schedule::Lanes {
                order: r.lanes.clone(),
                queues,
            }
        };
        let order = lanes.order.clone();
        let boot = mean(&r.boots).unwrap_or(boot);
        let (timeline, _) = schedule::simulate(lanes, r.capacity, boot, |i| i.2);
        let simulated = timeline.makespan().unwrap_or_default();
        replays.push(Replay {
            run_id: r.run_id,
            project: r.project.clone(),
            items: r.items.len(),
            capacity: r.capacity,
            lanes: order,
            measured_ms: ms(r.measured),
            simulated_ms: ms(simulated),
            error: (simulated.as_secs_f64() - r.measured.as_secs_f64()) / r.measured.as_secs_f64(),
        });
    }
    let mape = replays.iter().map(|r| r.error.abs()).sum::<f64>() / replays.len() as f64;
    let ranking_matches = (replays.len() > 1).then(|| {
        replays.iter().enumerate().all(|(i, a)| {
            replays[i + 1..]
                .iter()
                .all(|b| a.measured_ms.cmp(&b.measured_ms) == a.simulated_ms.cmp(&b.simulated_ms))
        })
    });

    let latest = runs.last().expect("at least one run");
    let mut samples: BTreeMap<&str, Vec<Duration>> = BTreeMap::new();
    for r in runs {
        for (name, _, took) in &r.items {
            samples.entry(name).or_default().push(*took);
        }
    }
    let mut devices: BTreeMap<String, usize> = BTreeMap::new();
    for r in runs {
        for (kind, d) in &r.devices {
            let n = devices.entry(kind.clone()).or_default();
            *n = (*n).max(d.len());
        }
    }
    let recorded: BTreeSet<usize> = runs.iter().map(|r| r.capacity).collect();
    let builds: Vec<Duration> = runs.iter().filter_map(|r| r.build).collect();
    // (index, kind, mean) of the simulated items, longest first like `kr run`.
    let items: Vec<(usize, String, Duration)> = longest_first(
        latest
            .items
            .iter()
            .enumerate()
            .map(|(i, (name, kind, _))| {
                (
                    i,
                    kind.clone(),
                    mean(&samples[name.as_str()]).unwrap_or_default(),
                )
            })
            .collect(),
        |i| Some(i.2),
    );
    let item_samples: Vec<&Vec<Duration>> = latest
        .items
        .iter()
        .map(|(name, _, _)| &samples[name.as_str()])
        .collect();

    let sims: Vec<usize> = if args.sims.is_empty() {
        vec![latest.capacity]
    } else {
        args.sims.iter().map(|&n| n as usize).collect()
    };
    let orders = if args.orders.is_empty() {
        vec![LaneOrder::IpadFirst]
    } else {
        args.orders.clone()
    };
    let caches = if args.caches.is_empty() {
        vec![Cache::Hit]
    } else {
        args.caches.clone()
    };
    let mut settings = Vec::new();
    for &sim in &sims {
        for &order in &orders {
            for &cache in &caches {
                let build = match cache {
                    Cache::Hit => Duration::ZERO,
                    Cache::Miss => match mean(&builds) {
                        Some(b) => b,
                        None => bail!(
                            "--cache miss: the given runs have no build step to take the build time from; give a run that built (see `kr check` / `kr gate` after a change)"
                        ),
                    },
                };
                let mut rng = SplitMix(1);
                let mut totals = Vec::new();
                let mut queue_share = 0.0;
                for _ in 0..args.iterations {
                    let drawn: Vec<Duration> =
                        item_samples.iter().map(|s| s[rng.below(s.len())]).collect();
                    let queues = schedule::queues(items.clone(), |i| i.1.clone());
                    let order = match order {
                        LaneOrder::IpadFirst => Order::IpadFirst,
                        LaneOrder::Name => Order::Name,
                    };
                    let lanes = schedule::lanes(queues, &devices, sim, order, ipad);
                    let (timeline, _) = schedule::simulate(lanes, sim, boot, move |i| drawn[i.0]);
                    totals.push(build + timeline.makespan().unwrap_or_default());
                    queue_share += timeline.queue_share();
                }
                totals.sort();
                let pick = |p: f64| totals[((p * totals.len() as f64).ceil() as usize).max(1) - 1];
                settings.push(Setting {
                    sim,
                    order,
                    cache,
                    p50_ms: ms(pick(0.5)),
                    p90_ms: ms(pick(0.9)),
                    lower_bound_ms: ms(build + lower_bound(&items, &devices, sim)),
                    queue_share: queue_share / f64::from(args.iterations),
                    outside_calibration: (!recorded.contains(&sim)).then(|| {
                        format!(
                            "sim={sim}: the runs were recorded with sim={}; concurrency changes item durations, so this is a direction only",
                            recorded.iter().map(ToString::to_string).collect::<Vec<_>>().join(", sim=")
                        )
                    }),
                });
            }
        }
    }
    let counts: Vec<usize> = item_samples.iter().map(|s| s.len()).collect();
    Ok(Report {
        schema_version: SCHEMA_VERSION,
        calibration: Calibration {
            runs: replays,
            mape,
            target: TARGET,
            ranking_matches,
        },
        items: Items {
            run_id: latest.run_id,
            count: latest.items.len(),
            samples_min: counts.iter().copied().min().unwrap_or(0),
            samples_max: counts.iter().copied().max().unwrap_or(0),
        },
        iterations: args.iterations,
        settings,
    })
}

/// max(longest item, all items ÷ simulators, each kind's items ÷ its simulators).
fn lower_bound(
    items: &[(usize, String, Duration)],
    devices: &BTreeMap<String, usize>,
    sim: usize,
) -> Duration {
    let longest = items.iter().map(|i| i.2).max().unwrap_or_default();
    let total: Duration = items.iter().map(|i| i.2).sum();
    let all = sim.min(devices.values().sum()).max(1) as u32;
    let mut bound = longest.max(total / all);
    for (kind, n) in devices {
        let of_kind: Duration = items.iter().filter(|i| &i.1 == kind).map(|i| i.2).sum();
        bound = bound.max(of_kind / sim.min(*n).max(1) as u32);
    }
    bound
}

/// A small fixed-seed generator (SplitMix64): the same input gives the same report.
struct SplitMix(u64);

impl SplitMix {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) % n as u64) as usize
    }
}

fn minutes(ms: u64) -> String {
    format!("{:.1} min", ms as f64 / 60_000.0)
}

fn percent(x: f64) -> String {
    format!("{:+.1}%", x * 100.0)
}

fn human(r: &Report) -> String {
    let c = &r.calibration;
    let mut out = String::new();
    let runs = c.runs.len();
    out += &format!(
        "calibration: {runs} run{}, {} {:.1}% (target ≤ {:.0}%){}\n",
        if runs == 1 { "" } else { "s" },
        if runs == 1 { "error" } else { "MAPE" },
        c.mape * 100.0,
        c.target * 100.0,
        match c.ranking_matches {
            Some(true) => ", ranking matches the measurements",
            Some(false) => ", ranking does NOT match the measurements",
            None => "",
        }
    );
    for run in &c.runs {
        out += &format!(
            "  {} {}: measured {}, simulated {} ({}) · sim={} · lanes {}\n",
            &run.run_id.to_string()[..8],
            run.project.as_deref().unwrap_or("-"),
            minutes(run.measured_ms),
            minutes(run.simulated_ms),
            percent(run.error),
            run.capacity,
            run.lanes.join(", "),
        );
    }
    let i = &r.items;
    let samples = if i.samples_min == i.samples_max {
        format!(
            "{} sample{} each",
            i.samples_min,
            if i.samples_min == 1 { "" } else { "s" }
        )
    } else {
        format!("{}–{} samples each", i.samples_min, i.samples_max)
    };
    out += &format!(
        "simulated: {} items of run {}, {samples}, {} iterations\n",
        i.count,
        &i.run_id.to_string()[..8],
        r.iterations
    );
    for s in &r.settings {
        let order = match s.order {
            LaneOrder::IpadFirst => "ipad-first",
            LaneOrder::Name => "name",
        };
        let cache = match s.cache {
            Cache::Hit => "hit",
            Cache::Miss => "miss",
        };
        out += &format!(
            "  sim={} order={order} cache={cache}: p50 {} · p90 {} · lower bound {} · queue {:.0}%\n",
            s.sim,
            minutes(s.p50_ms),
            minutes(s.p90_ms),
            minutes(s.lower_bound_ms),
            s.queue_share * 100.0
        );
        if let Some(note) = &s.outside_calibration {
            out += &format!("    outside calibration: {note}\n");
        }
    }
    out
}
