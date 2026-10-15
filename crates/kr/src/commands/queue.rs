use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use clap::Args;
use jiff::Timestamp;
use kr_events::{EventKind, EventLog};
use kr_resource::{Resource, Status, estimate_waits};
use serde_json::{Value, json};

use super::{human_duration, resource_parser};
use crate::Ctx;

#[derive(Debug, Args)]
pub struct QueueArgs {
    /// Resources to show (build, sim, light). Default: all.
    #[arg(value_parser = resource_parser())]
    resources: Vec<Resource>,
}

/// How many recent releases the average hold time is taken from.
const HISTORY: usize = 20;

pub fn run(ctx: &Ctx, args: QueueArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let pool = loaded.pool();
    let log = loaded.events();
    let resources = if args.resources.is_empty() {
        Resource::ALL.to_vec()
    } else {
        args.resources
    };
    let now = Timestamp::now();
    let runner = ctx.runner()?;
    let mut views = Vec::new();
    for res in resources {
        let status = pool.status(res, loaded.capacity(res, runner.as_ref())?)?;
        views.push(view(&status, avg_hold(&log, res), now));
    }

    if ctx.json {
        crate::output::print_json(&json!({ "resources": views }))?;
    } else {
        for v in &views {
            print_human(v);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Mean hold time of the last releases of `res` in the past 7 days.
pub fn avg_hold(log: &EventLog, res: Resource) -> Option<Duration> {
    let since = Timestamp::now() - jiff::SignedDuration::from_hours(24 * 7);
    let events = log.read_since(since).ok()?;
    let held: Vec<u64> = events
        .iter()
        .filter(|e| e.kind == EventKind::SlotReleased && e.fields["resource"] == res.as_str())
        .filter_map(|e| e.fields["held_ms"].as_u64())
        .collect();
    let recent = &held[held.len().saturating_sub(HISTORY)..];
    if recent.is_empty() {
        return None;
    }
    Some(Duration::from_millis(
        recent.iter().sum::<u64>() / recent.len() as u64,
    ))
}

fn since(now: Timestamp, then: Timestamp) -> Duration {
    Duration::try_from(now.duration_since(then)).unwrap_or_default()
}

/// Expected waits for the waiters of `status`, if there is history.
pub fn estimates(status: &Status, avg: Option<Duration>, now: Timestamp) -> Vec<Option<Duration>> {
    let Some(avg) = avg else {
        return vec![None; status.waiters.len()];
    };
    let held_for: Vec<Duration> = status
        .holders
        .iter()
        .map(|h| {
            h.holder
                .as_ref()
                .map_or(Duration::ZERO, |h| since(now, h.started))
        })
        .collect();
    estimate_waits(avg, status.capacity, &held_for, status.waiters.len())
        .into_iter()
        .map(Some)
        .collect()
}

fn view(status: &Status, avg: Option<Duration>, now: Timestamp) -> Value {
    let holders: Vec<Value> = status
        .holders
        .iter()
        .map(|h| {
            let mut v = json!({ "slot": h.slot, "orphan": h.orphan });
            if let Some(holder) = &h.holder {
                v["project"] = json!(holder.project);
                v["task"] = json!(holder.task);
                v["pid"] = json!(holder.pid);
                v["child_pid"] = json!(holder.child_pid);
                v["started"] = json!(holder.started);
                v["held_seconds"] = json!(since(now, holder.started).as_secs());
            }
            v
        })
        .collect();
    let waiters: Vec<Value> = status
        .waiters
        .iter()
        .zip(estimates(status, avg, now))
        .map(|(w, eta)| {
            json!({
                "project": w.holder.project,
                "task": w.holder.task,
                "pid": w.holder.pid,
                "enqueued": w.holder.started,
                "waited_seconds": since(now, w.holder.started).as_secs(),
                "eta_seconds": eta.map(|d| d.as_secs()),
            })
        })
        .collect();
    json!({
        "resource": status.resource,
        "capacity": status.capacity,
        "avg_hold_seconds": avg.map(|d| d.as_secs()),
        "holders": holders,
        "waiters": waiters,
    })
}

fn print_human(v: &Value) {
    let holders = v["holders"].as_array().map_or(&[][..], Vec::as_slice);
    let waiters = v["waiters"].as_array().map_or(&[][..], Vec::as_slice);
    let secs = |x: &Value| human_duration(Duration::from_secs(x.as_u64().unwrap_or(0)));
    let mut line = format!(
        "{:<6} {}/{} busy",
        v["resource"].as_str().unwrap_or("?"),
        holders.len(),
        v["capacity"]
    );
    if !waiters.is_empty() {
        line += &format!(", {} waiting", waiters.len());
    }
    if !v["avg_hold_seconds"].is_null() {
        line += &format!(", avg hold {}", secs(&v["avg_hold_seconds"]));
    }
    println!("{line}");
    for h in holders {
        let who = describe(h);
        let orphan = if h["orphan"] == true {
            " ORPHAN: kr is gone but its command still runs; kill it if it is stuck"
        } else {
            ""
        };
        println!(
            "  slot {}  {who} · {}{orphan}",
            h["slot"],
            secs(&h["held_seconds"])
        );
    }
    for (i, w) in waiters.iter().enumerate() {
        let eta = if w["eta_seconds"].is_null() {
            "eta unknown".to_owned()
        } else {
            format!("eta ~{}", secs(&w["eta_seconds"]))
        };
        println!(
            "  {}. {} · waited {} · {eta}",
            i + 1,
            describe(w),
            secs(&w["waited_seconds"])
        );
    }
}

fn describe(v: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(p) = v["project"].as_str() {
        parts.push(p.to_owned());
    }
    if let Some(t) = v["task"].as_str() {
        parts.push(t.to_owned());
    }
    if let Some(pid) = v["pid"].as_u64() {
        match v["child_pid"].as_u64() {
            Some(child) => parts.push(format!("pid {pid} (command {child})")),
            None => parts.push(format!("pid {pid}")),
        }
    }
    if parts.is_empty() {
        "unknown holder".to_owned()
    } else {
        parts.join(" · ")
    }
}
