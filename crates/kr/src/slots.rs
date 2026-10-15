//! Taking and giving back resource slots, with queue events, a wait notice and the
//! load check. Every command that needs a slot goes through here.

use std::time::{Duration, Instant};

use anyhow::Result;
use jiff::Timestamp;
use kr_events::{Event, EventKind, RunId};
use kr_exec::Runner;
use kr_resource::legacy::{LegacyGuard, LegacyLock};
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
    /// The old machine-wide lock, held with heavy slots when the bridge is on.
    pub legacy: Option<LegacyGuard>,
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
    let capacity = loaded.capacity(res, runner)?;
    take_in(
        ctx,
        loaded,
        runner,
        run_id,
        &loaded.pool(),
        res,
        capacity,
        task,
    )
}

/// Queues for the one slot of the real device `udid`, like [`take`]: a test run on a
/// device is heavy work, so it also waits for the load and takes the old machine lock.
pub fn take_device(
    ctx: &Ctx,
    loaded: &Loaded,
    runner: &dyn Runner,
    run_id: RunId,
    udid: &str,
    task: &str,
) -> Result<Taken> {
    let pool = loaded.pool().device(udid);
    take_in(
        ctx,
        loaded,
        runner,
        run_id,
        &pool,
        Resource::Device,
        1,
        task,
    )
}

#[allow(clippy::too_many_arguments)]
fn take_in(
    ctx: &Ctx,
    loaded: &Loaded,
    runner: &dyn Runner,
    run_id: RunId,
    pool: &kr_resource::Pool,
    res: Resource,
    capacity: u32,
    task: &str,
) -> Result<Taken> {
    let log = loaded.events();
    let project = loaded.project();
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
        // Only commands that catch stop signals (see kr_exec) see one here.
        if let Some(signal) = kr_exec::stop_signal() {
            anyhow::bail!("stopped by signal {signal} while waiting for {res}");
        }
        if !announced {
            announced = true;
            announce_wait(ctx, loaded, pool, res, capacity)?;
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
    let heavy = matches!(res, Resource::Build | Resource::Sim | Resource::Device);
    // Wait (for critical memory pressure, or high load if `[load] block`) before taking
    // the machine-wide lock, so a busy machine does not also block other projects' queue.
    if heavy {
        wait_for_load(ctx, loaded, runner, run_id, res)?;
    }
    let legacy = if heavy {
        machine_lock(ctx, loaded, run_id, task)?
    } else {
        None
    };
    Ok(Taken {
        slot,
        task: task.to_owned(),
        started: Instant::now(),
        legacy,
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
    drop(taken.legacy);
    drop(taken.slot);
    Ok(())
}

/// What holds the machine now (ADR 0010): `build` and `sim` slots held by any project,
/// and the holder of the old machine-wide lock if the bridge is configured.
pub fn concurrency(loaded: &Loaded) -> serde_json::Value {
    let pool = loaded.pool();
    let held = |res| pool.status(res, 0).map_or(0, |s| s.holders.len());
    let legacy = &loaded.config.machine.legacy;
    let legacy_holder = legacy
        .machine_lock
        .as_ref()
        .zip(legacy.queue.as_ref())
        .and_then(|(lock, queue)| LegacyLock::new(lock, queue).owner())
        .map(|o| format!("{} (pid {}, since {})", o.task, o.pid, o.started));
    json!({
        "build": held(Resource::Build),
        "sim": held(Resource::Sim),
        "legacy_holder": legacy_holder,
    })
}

/// Takes the old machine-wide lock too, if the bridge is configured (ADR 0009).
fn machine_lock(
    ctx: &Ctx,
    loaded: &Loaded,
    run_id: RunId,
    task: &str,
) -> Result<Option<LegacyGuard>> {
    let legacy = &loaded.config.machine.legacy;
    let (Some(lock), Some(queue)) = (&legacy.machine_lock, &legacy.queue) else {
        return Ok(None);
    };
    let poll = legacy
        .poll
        .as_deref()
        .and_then(kr_config::parse_timeout)
        .unwrap_or(kr_resource::legacy::DEFAULT_POLL);
    let bridge = LegacyLock::new(lock, queue).with_poll(poll);
    let start = Instant::now();
    let mut waited_for = None;
    let guard = bridge.acquire(&format!("kr-{task}"), |owner| {
        let who = owner.map_or_else(
            || "unknown".to_owned(),
            |o| format!("{} (pid {}, since {})", o.task, o.pid, o.started),
        );
        waited_for = Some(who.clone());
        if ctx.json {
            eprintln!(
                "{}",
                json!({ "waiting": "machine_lock", "lock": lock, "holder": who })
            );
        } else {
            eprintln!(
                "waiting for the machine lock {}: held by {who}",
                lock.display()
            );
        }
    })?;
    loaded.events().append(&Event::now(
        run_id,
        loaded.project(),
        EventKind::MachineLock,
        json!({
            "lock": lock,
            "waited_ms": start.elapsed().as_millis() as u64,
            "waited_for": waited_for,
            "held_by_ancestor": guard.is_ancestors(),
            "shared_in_process": !guard.is_owned() && !guard.is_ancestors(),
        }),
    ))?;
    Ok(Some(guard))
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
    let policy = &loaded.config.machine.load;
    let per_core = policy.per_core;
    let wait =
        kr_machine::wait_for_load(runner, policy.block, per_core, LOAD_POLL, max_wait, |hw| {
            if !first {
                return;
            }
            first = false;
            let reason = hw.must_wait(policy.block, per_core).unwrap_or_default();
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
        });
    // Not knowing the load is no reason to hold work back.
    let wait = match wait {
        Ok(wait) => wait,
        Err(e) => {
            tracing::warn!("cannot read the machine load ({e:#}); going ahead");
            return Ok(());
        }
    };
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

fn announce_wait(
    ctx: &Ctx,
    loaded: &Loaded,
    pool: &kr_resource::Pool,
    res: Resource,
    capacity: u32,
) -> Result<()> {
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

/// Keeps the old machine lock's owner pointing at a running child (ADR 0009): the
/// newest one still running, else kr itself.
pub fn watch_children() {
    let active = std::sync::Mutex::new(Vec::<u32>::new());
    kr_exec::set_spawn_observer(move |event| {
        let mut active = active.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            kr_exec::Spawn::Started(pid) => active.push(pid),
            kr_exec::Spawn::Finished(pid) => active.retain(|p| *p != pid),
        }
        let owner = active.last().copied().unwrap_or_else(std::process::id);
        kr_resource::legacy::set_owner_pid(owner);
    });
}
