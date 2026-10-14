//! Taking and giving back resource slots, with queue events, a wait notice and the
//! load check. Every command that needs a slot goes through here.

use std::time::{Duration, Instant};

use anyhow::Result;
use jiff::Timestamp;
use kr_events::{Event, EventKind, RunId};
use kr_exec::Runner;
use kr_resource::{Holder, Resource, Slot};
use serde_json::json;

use crate::commands::{human_duration, queue};
use crate::{Ctx, Loaded};

const POLL: Duration = Duration::from_millis(500);
const LOAD_POLL: Duration = Duration::from_secs(15);

/// A slot taken for a task.
pub struct Taken {
    pub slot: Slot,
    pub task: String,
    pub started: Instant,
}

/// Queues for a slot of `res`, then (for `build` and `sim`) waits for the load to be
/// acceptable. Records queue_start, slot_acquired and load events.
pub fn take(
    ctx: &Ctx,
    loaded: &Loaded,
    runner: &dyn Runner,
    run_id: RunId,
    res: Resource,
    task: &str,
) -> Result<Taken> {
    let pool = loaded.pool();
    let log = loaded.events();
    let project = loaded.project();
    let capacity = loaded.capacity(res, runner)?;
    let holder = Holder {
        project: project.map(str::to_owned),
        task: task.to_owned(),
        pid: std::process::id(),
        child_pid: None,
        started: Timestamp::now(),
    };
    log.append(&Event::now(
        run_id,
        project,
        EventKind::QueueStart,
        json!({ "resource": res, "task": task, "capacity": capacity }),
    ))?;
    let queued = Instant::now();
    let ticket = pool.enqueue(res, &holder)?;
    let mut announced = false;
    let slot = loop {
        if let Some(slot) = ticket.try_acquire(capacity)? {
            break slot;
        }
        if !announced {
            announced = true;
            announce_wait(ctx, loaded, res, capacity)?;
        }
        std::thread::sleep(POLL);
    };
    drop(ticket);
    log.append(&Event::now(
        run_id,
        project,
        EventKind::SlotAcquired,
        json!({ "resource": res, "slot": slot.index(), "task": task, "waited_ms": queued.elapsed().as_millis() as u64 }),
    ))?;
    if matches!(res, Resource::Build | Resource::Sim) && loaded.config.machine.load.check {
        wait_for_load(ctx, loaded, runner, run_id, res)?;
    }
    Ok(Taken {
        slot,
        task: task.to_owned(),
        started: Instant::now(),
    })
}

/// Gives the slot back and records slot_released.
pub fn give_back(
    loaded: &Loaded,
    run_id: RunId,
    taken: Taken,
    exit_code: Option<i32>,
) -> Result<()> {
    loaded.events().append(&Event::now(
        run_id,
        loaded.project(),
        EventKind::SlotReleased,
        json!({
            "resource": taken.slot.resource(),
            "slot": taken.slot.index(),
            "task": taken.task,
            "held_ms": taken.started.elapsed().as_millis() as u64,
            "exit_code": exit_code,
        }),
    ))?;
    drop(taken.slot);
    Ok(())
}

/// Waits (holding the slot) while the machine is overloaded, and records it.
fn wait_for_load(
    ctx: &Ctx,
    loaded: &Loaded,
    runner: &dyn Runner,
    run_id: RunId,
    res: Resource,
) -> Result<()> {
    let log = loaded.events();
    let project = loaded.project();
    let max_wait = kr_config::parse_timeout(&loaded.config.machine.load.max_wait)
        .unwrap_or(kr_config::DEFAULT_TIMEOUT);
    let mut first = true;
    let wait = kr_machine::wait_for_load(runner, LOAD_POLL, max_wait, |hw| {
        if !first {
            return;
        }
        first = false;
        let reason = hw.overloaded().unwrap_or_default();
        let _ = log.append(&Event::now(
            run_id,
            project,
            EventKind::LoadWait,
            json!({ "resource": res, "load": hw.load, "pressure": hw.pressure, "reason": reason }),
        ));
        if ctx.json {
            eprintln!("{}", json!({ "load_wait": res, "reason": reason }));
        } else {
            eprintln!("waiting before {res} work: {reason}");
        }
    })?;
    if wait.checks > 1 {
        log.append(&Event::now(
            run_id,
            project,
            EventKind::LoadOk,
            json!({ "resource": res, "waited_ms": wait.waited.as_millis() as u64, "checks": wait.checks, "timed_out": wait.timed_out }),
        ))?;
        if wait.timed_out {
            eprintln!(
                "load still high after {}; going ahead",
                human_duration(wait.waited)
            );
        }
    }
    Ok(())
}

fn announce_wait(ctx: &Ctx, loaded: &Loaded, res: Resource, capacity: u32) -> Result<()> {
    let pool = loaded.pool();
    let status = pool.status(res, capacity)?;
    let me = std::process::id();
    let position = status
        .waiters
        .iter()
        .position(|w| w.holder.pid == me)
        .unwrap_or(0);
    let eta = queue::estimates(
        &status,
        queue::avg_hold(&loaded.events(), res),
        Timestamp::now(),
    )
    .get(position)
    .copied()
    .flatten();
    if ctx.json {
        eprintln!(
            "{}",
            json!({ "waiting": res, "ahead": position, "holders": status.holders.len(), "eta_seconds": eta.map(|d| d.as_secs()) })
        );
    } else {
        let eta = eta.map_or_else(
            || "unknown".to_owned(),
            |d| format!("~{}", human_duration(d)),
        );
        eprintln!(
            "waiting for {res}: {}/{capacity} busy, {position} ahead, eta {eta} (see `kr queue {res}`)",
            status.holders.len()
        );
    }
    Ok(())
}
