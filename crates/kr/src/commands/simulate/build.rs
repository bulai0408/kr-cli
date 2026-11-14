//! `kr simulate --build N` (#65): the recorded `build` requests of all projects on this
//! machine (they share one queue), replayed under kr's slot rule: first come, first
//! served, a request starts once it is first in line and fewer than the capacity hold
//! a slot.
//!
//! The calibration replays the requests as they were taken: until the bridge was
//! removed (ADR 0009, ADR 0022), a request that took the old machine-wide lock queued
//! for that one lock together with the `sim` requests that took it. The settings replay
//! the build slot alone, as kr runs now.

use std::collections::BTreeSet;
use std::process::ExitCode;

use anyhow::{Result, bail};
use jiff::{SignedDuration, Timestamp};
use kr_events::{EventKind, RunId};
use serde::Serialize;

use super::{SimulateArgs, TARGET, minutes};
use crate::Ctx;
use crate::commands::report::parse_time;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    since: Timestamp,
    requests: Requests,
    slowdown: f64,
    calibration: Calibration,
    settings: Vec<Setting>,
    /// What the numbers leave out.
    notes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Requests {
    /// Build requests that queued, held a slot and gave it back.
    build: usize,
    /// Of those, the ones that took the old machine-wide lock themselves.
    bridged: usize,
    /// Build requests left out: no slot or no release in the events (killed, or still
    /// running).
    unfinished: usize,
}

#[derive(Debug, Serialize)]
struct Calibration {
    /// The build capacities the requests were recorded with.
    capacities: Vec<usize>,
    measured: Stats,
    replayed: Stats,
    /// (replayed − measured) ÷ measured queued time; null when nothing waited.
    error: Option<f64>,
    target: f64,
}

#[derive(Debug, Serialize)]
struct Setting {
    build: usize,
    #[serde(flatten)]
    stats: Stats,
    /// Set when the capacity is not one the requests were recorded with: then the
    /// result is a direction only (ADR 0010).
    outside_calibration: Option<String>,
}

#[derive(Debug, Serialize)]
struct Stats {
    queued_ms: u64,
    /// queued ÷ (queued + held).
    queue_share: f64,
    p50_wait_ms: u64,
    p90_wait_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Build,
    Sim,
}

/// One request for a slot, from `queue_start` to `slot_released`.
struct Request {
    arrival: Timestamp,
    kind: Kind,
    capacity: usize,
    waited_ms: u64,
    held_ms: u64,
    /// Took the old machine-wide lock itself: milliseconds from the lock to the slot.
    bridged_ms: Option<u64>,
}

/// What the FIFO replay needs of a request.
struct Job {
    arrival: Timestamp,
    capacity: usize,
    hold_ms: u64,
}

pub(super) fn slowdown(s: &str) -> Result<f64, String> {
    match s.parse::<f64>() {
        Ok(f) if f.is_finite() && f >= 1.0 => Ok(f),
        _ => Err(format!(
            "`{s}` is not a factor of 1.0 or more; 1.5 means builds side by side each run 1.5 times slower"
        )),
    }
}

pub(super) fn run(ctx: &Ctx, args: &SimulateArgs) -> Result<ExitCode> {
    let now = Timestamp::now();
    let since = match &args.since {
        Some(s) => parse_time(s, now, "--since")?,
        None => now - SignedDuration::from_hours(24),
    };
    let loaded = ctx.load()?;
    let log = loaded.events();
    let (requests, unfinished) = requests(&log.read_since(since)?);
    let builds: Vec<&Request> = requests.iter().filter(|r| r.kind == Kind::Build).collect();
    if builds.is_empty() {
        bail!(
            "no finished build request since {since} in the events under {}; give an earlier --since",
            log.dir().display()
        );
    }
    let capacities: Vec<usize> = args.builds.iter().map(|&n| n as usize).collect();
    let report = simulate(since, &requests, unfinished, &capacities, args.slowdown);
    if ctx.json {
        crate::output::print_json(&report)?;
    } else {
        print!("{}", human(&report));
    }
    Ok(ExitCode::SUCCESS)
}

/// The finished `build` and `sim` requests in arrival order, and how many build
/// requests did not finish.
fn requests(events: &[kr_events::Event]) -> (Vec<Request>, usize) {
    struct Open {
        arrival: Timestamp,
        kind: Kind,
        capacity: usize,
        lock: Option<Timestamp>,
        acquired: Option<(Timestamp, u64)>,
    }
    let mut open: Vec<((RunId, String, String), Open)> = Vec::new();
    let mut done = Vec::new();
    let mut unfinished = 0;
    let key = |e: &kr_events::Event| {
        (
            e.run_id,
            e.fields["resource"].as_str().unwrap_or_default().to_owned(),
            e.fields["task"].as_str().unwrap_or_default().to_owned(),
        )
    };
    for e in events {
        match e.kind {
            EventKind::QueueStart => {
                let kind = match e.fields["resource"].as_str() {
                    Some("build") => Kind::Build,
                    Some("sim") => Kind::Sim,
                    _ => continue,
                };
                open.push((
                    key(e),
                    Open {
                        arrival: e.ts,
                        kind,
                        capacity: e.fields["capacity"].as_u64().unwrap_or(1) as usize,
                        lock: None,
                        acquired: None,
                    },
                ));
            }
            // Riding on a lock someone else holds (a script, or another lane of the
            // same run) is no wait of its own for that lock.
            EventKind::MachineLock
                if e.fields["held_by_ancestor"] != true
                    && e.fields["shared_in_process"] != true =>
            {
                if let Some((_, o)) = open.iter_mut().find(|((run, _, _), o)| {
                    *run == e.run_id && o.lock.is_none() && o.acquired.is_none()
                }) {
                    o.lock = Some(e.ts);
                }
            }
            EventKind::SlotAcquired => {
                let k = key(e);
                if let Some((_, o)) = open.iter_mut().find(|(ok, _)| *ok == k) {
                    o.acquired = Some((e.ts, e.fields["waited_ms"].as_u64().unwrap_or(0)));
                }
            }
            EventKind::SlotReleased => {
                let k = key(e);
                let Some(i) = open.iter().position(|(ok, _)| *ok == k) else {
                    continue;
                };
                let (_, o) = open.remove(i);
                let (Some((acquired, waited_ms)), Some(held_ms)) =
                    (o.acquired, e.fields["held_ms"].as_u64())
                else {
                    unfinished += usize::from(o.kind == Kind::Build);
                    continue;
                };
                done.push(Request {
                    arrival: o.arrival,
                    kind: o.kind,
                    capacity: o.capacity,
                    waited_ms,
                    held_ms,
                    bridged_ms: o.lock.map(|l| millis(acquired.duration_since(l))),
                });
            }
            _ => {}
        }
    }
    done.sort_by_key(|r| r.arrival);
    unfinished += open.iter().filter(|(_, o)| o.kind == Kind::Build).count();
    (done, unfinished)
}

fn millis(d: SignedDuration) -> u64 {
    d.as_millis().max(0) as u64
}

/// The rule of `kr_resource::Ticket::try_acquire` (a waiter takes a slot when fewer
/// waiters are ahead of it than slots are free), in virtual time: each job, in arrival
/// order, starts once the one before it started and fewer than its capacity hold a
/// slot. Its recorded hold is work done at rate 1 alone; while two or more jobs hold a
/// slot, each advances at 1 ÷ `slowdown` (#69: two builds side by side each took 2.03
/// times as long). Returns (wait, hold) per job.
fn fifo(jobs: &[Job], slowdown: f64) -> Vec<(u64, u64)> {
    /// Times and work in microseconds from the first arrival, as the events record them.
    struct Running {
        job: usize,
        start: f64,
        left: f64,
    }
    let Some(origin) = jobs.first().map(|j| j.arrival) else {
        return Vec::new();
    };
    let at = |j: &Job| j.arrival.duration_since(origin).as_micros() as f64;
    let mut out = vec![(0, 0); jobs.len()];
    let mut running: Vec<Running> = Vec::new();
    let mut next = 0;
    let mut now = 0.0;
    loop {
        while next < jobs.len()
            && at(&jobs[next]) <= now
            && running.len() < jobs[next].capacity.max(1)
        {
            running.push(Running {
                job: next,
                start: now,
                left: jobs[next].hold_ms as f64 * 1000.0,
            });
            next += 1;
        }
        let rate = if running.len() > 1 {
            1.0 / slowdown
        } else {
            1.0
        };
        let arrival = (next < jobs.len() && running.len() < jobs[next].capacity.max(1))
            .then(|| at(&jobs[next]));
        let Some(until) = running
            .iter()
            .map(|r| now + r.left / rate)
            .chain(arrival)
            .min_by(f64::total_cmp)
        else {
            return out;
        };
        running.retain_mut(|r| {
            if now + r.left / rate <= until {
                out[r.job] = (
                    ((r.start - at(&jobs[r.job])) / 1000.0) as u64,
                    ((until - r.start) / 1000.0) as u64,
                );
                false
            } else {
                r.left -= (until - now) * rate;
                true
            }
        });
        now = until;
    }
}

fn stats(waits_and_holds: &[(u64, u64)]) -> Stats {
    let queued: u64 = waits_and_holds.iter().map(|w| w.0).sum();
    let held: u64 = waits_and_holds.iter().map(|w| w.1).sum();
    let mut waits: Vec<u64> = waits_and_holds.iter().map(|w| w.0).collect();
    waits.sort_unstable();
    let pick = |p: f64| waits[((p * waits.len() as f64).ceil() as usize).max(1) - 1];
    Stats {
        queued_ms: queued,
        queue_share: if queued + held == 0 {
            0.0
        } else {
            queued as f64 / (queued + held) as f64
        },
        p50_wait_ms: pick(0.5),
        p90_wait_ms: pick(0.9),
    }
}

fn simulate(
    since: Timestamp,
    requests: &[Request],
    unfinished: usize,
    capacities: &[usize],
    slowdown: f64,
) -> Report {
    let builds: Vec<&Request> = requests.iter().filter(|r| r.kind == Kind::Build).collect();
    let measured: Vec<(u64, u64)> = builds.iter().map(|r| (r.waited_ms, r.held_ms)).collect();

    // As recorded: the bridged requests share the one machine-wide lock, held from the
    // lock to the release; the others queue for the build slot.
    let (bridged, slot): (Vec<&Request>, Vec<&Request>) =
        requests.iter().partition(|r| r.bridged_ms.is_some());
    let mut replayed: Vec<(u64, u64)> = Vec::new();
    for (queue, legacy) in [(bridged, true), (slot, false)] {
        let queue: Vec<&Request> = queue
            .into_iter()
            .filter(|r| legacy || r.kind == Kind::Build)
            .collect();
        let jobs: Vec<Job> = queue
            .iter()
            .map(|r| Job {
                arrival: r.arrival,
                capacity: if legacy { 1 } else { r.capacity },
                hold_ms: r.held_ms + r.bridged_ms.unwrap_or(0),
            })
            .collect();
        for (r, (wait, _)) in queue.iter().zip(fifo(&jobs, 1.0)) {
            if r.kind == Kind::Build {
                replayed.push((wait, r.held_ms));
            }
        }
    }
    let measured = stats(&measured);
    let replayed = stats(&replayed);
    let recorded: BTreeSet<usize> = builds.iter().map(|r| r.capacity).collect();

    let settings = capacities
        .iter()
        .map(|&n| {
            let jobs: Vec<Job> = builds
                .iter()
                .map(|r| Job {
                    arrival: r.arrival,
                    capacity: n,
                    hold_ms: r.held_ms,
                })
                .collect();
            Setting {
                build: n,
                stats: stats(&fifo(&jobs, slowdown)),
                outside_calibration: (!recorded.contains(&n)).then(|| {
                    format!(
                        "build={n}: the requests were recorded with build={}; builds side by side take longer, so this is a direction only",
                        recorded.iter().map(ToString::to_string).collect::<Vec<_>>().join(", build=")
                    )
                }),
            }
        })
        .collect();

    let bridged = builds.iter().filter(|r| r.bridged_ms.is_some()).count();
    let mut notes = vec![
        "all projects on this machine: they share the build queue".to_owned(),
        if slowdown == 1.0 {
            "slowdown 1.0: builds side by side keep their recorded durations, so the waits with more slots are a lower bound; give --slowdown to stretch them".to_owned()
        } else {
            format!(
                "slowdown {slowdown}: while two or more builds hold a slot, each runs {slowdown} times slower than recorded"
            )
        },
        "arrivals are replayed as recorded: a request that waited less would have let its agent send the next one sooner, so a larger capacity saves less than shown".to_owned(),
    ];
    if bridged > 0 {
        notes.push(format!(
            "{bridged} of {} build requests took the old machine-wide lock, which sim requests took too: the calibration replays that lock, the settings the build slot alone; waits for lock holders outside kr are not in the events, so the replay falls short of the measured waits",
            builds.len()
        ));
    }
    Report {
        schema_version: SCHEMA_VERSION,
        since,
        requests: Requests {
            build: builds.len(),
            bridged,
            unfinished,
        },
        slowdown,
        calibration: Calibration {
            capacities: recorded.into_iter().collect(),
            error: (measured.queued_ms > 0).then(|| {
                (replayed.queued_ms as f64 - measured.queued_ms as f64) / measured.queued_ms as f64
            }),
            measured,
            replayed,
            target: TARGET,
        },
        settings,
        notes,
    }
}

fn line(s: &Stats) -> String {
    format!(
        "{} queued ({:.0}%) · p50 wait {} · p90 wait {}",
        minutes(s.queued_ms),
        s.queue_share * 100.0,
        minutes(s.p50_wait_ms),
        minutes(s.p90_wait_ms)
    )
}

fn human(r: &Report) -> String {
    let c = &r.calibration;
    let mut out = format!(
        "build requests since {}: {} ({} unfinished left out)\n",
        r.since, r.requests.build, r.requests.unfinished
    );
    let capacities = c
        .capacities
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", build=");
    out += &format!(
        "calibration (build={capacities} as recorded): error {} (target ≤ {:.0}%)\n",
        c.error.map_or_else(
            || "n/a (nothing waited)".to_owned(),
            |e| format!("{:+.1}%", e * 100.0)
        ),
        c.target * 100.0
    );
    out += &format!("  measured: {}\n", line(&c.measured));
    out += &format!("  replayed: {}\n", line(&c.replayed));
    out += &format!("settings (slowdown {:?}):\n", r.slowdown);
    for s in &r.settings {
        out += &format!("  build={}: {}\n", s.build, line(&s.stats));
        if let Some(note) = &s.outside_calibration {
            out += &format!("    outside calibration: {note}\n");
        }
    }
    out += "notes:\n";
    for note in &r.notes {
        out += &format!("  - {note}\n");
    }
    out
}
