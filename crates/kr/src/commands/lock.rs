use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use jiff::Timestamp;
use kr_events::{Event, EventKind, RunId};
use kr_resource::{Holder, Resource, Slot};
use serde_json::json;

use super::{human_duration, queue, resource_parser};
use crate::Ctx;

#[derive(Debug, Args)]
pub struct LockArgs {
    /// The resource to take a slot of: build, sim or light.
    #[arg(value_parser = resource_parser())]
    resource: Resource,

    /// Task name shown in `kr queue`. Default: the command line.
    #[arg(long)]
    task: Option<String>,

    /// The command to run while holding the slot.
    #[arg(last = true, required = true)]
    command: Vec<String>,
}

/// Slots held by ancestors, passed to children as `KR_HELD=build:1,sim:2`.
const HELD_VAR: &str = "KR_HELD";
const POLL: Duration = Duration::from_millis(500);

pub fn run(ctx: &Ctx, args: LockArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let pool = loaded.pool();
    let log = loaded.events();
    let project = loaded.project();
    let run_id = RunId::new();
    let res = args.resource;
    let task = args.task.clone().unwrap_or_else(|| args.command.join(" "));
    let held = parse_held(ctx.env_var(HELD_VAR).unwrap_or(""));

    if let Some(&(_, index)) = held.iter().find(|(r, _)| *r == res) {
        let owned = pool.is_held(res, index)
            && pool
                .holder(res, index)
                .is_some_and(|h| kr_resource::is_ancestor(h.pid));
        if owned {
            tracing::info!("{res} slot {index} is held by an ancestor; running without queueing");
            return run_command(&args.command, None, &held).map(exit_code);
        }
        log.append(&Event::now(
            run_id,
            project,
            EventKind::ReentryDenied,
            json!({ "resource": res, "slot": index, "task": task }),
        ))?;
    }

    let capacity = loaded.capacity(res);
    let holder = Holder {
        project: project.map(str::to_owned),
        task: task.clone(),
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
    let mut slot = loop {
        if let Some(slot) = ticket.try_acquire(capacity)? {
            break slot;
        }
        if !announced {
            announced = true;
            announce_wait(ctx, &loaded, res, capacity)?;
        }
        std::thread::sleep(POLL);
    };
    drop(ticket);
    let waited = queued.elapsed();
    log.append(&Event::now(
        run_id,
        project,
        EventKind::SlotAcquired,
        json!({ "resource": res, "slot": slot.index(), "task": task, "waited_ms": waited.as_millis() as u64 }),
    ))?;

    let started = Instant::now();
    let mut child_held: Vec<(Resource, u32)> =
        held.into_iter().filter(|(r, _)| *r != res).collect();
    child_held.push((res, slot.index()));
    let result = run_command(&args.command, Some(&mut slot), &child_held);
    let exit = result.as_ref().ok().copied();
    log.append(&Event::now(
        run_id,
        project,
        EventKind::SlotReleased,
        json!({
            "resource": res,
            "slot": slot.index(),
            "task": task,
            "held_ms": started.elapsed().as_millis() as u64,
            "exit_code": exit,
        }),
    ))?;
    drop(slot);
    result.map(exit_code)
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn announce_wait(ctx: &Ctx, loaded: &crate::Loaded, res: Resource, capacity: u32) -> Result<()> {
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

fn parse_held(value: &str) -> Vec<(Resource, u32)> {
    value
        .split(',')
        .filter_map(|item| {
            let (res, slot) = item.split_once(':')?;
            Some((res.parse().ok()?, slot.parse().ok()?))
        })
        .collect()
}

fn format_held(held: &[(Resource, u32)]) -> String {
    held.iter()
        .map(|(r, i)| format!("{r}:{i}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Runs the command in its own process group and returns its exit code
/// (128 + signal number if it was killed). SIGINT, SIGTERM and SIGHUP sent to kr
/// are forwarded to the whole group.
fn run_command(
    command: &[String],
    slot: Option<&mut Slot>,
    held: &[(Resource, u32)],
) -> Result<i32> {
    let (program, args) = command.split_first().context("missing command")?;
    let pending = Arc::new(AtomicUsize::new(0));
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register_usize(signal, Arc::clone(&pending), signal as usize)?;
    }
    let mut child = std::process::Command::new(program)
        .args(args)
        .env(HELD_VAR, format_held(held))
        .process_group(0)
        .spawn()
        .with_context(|| format!("failed to start `{}`", command.join(" ")))?;
    if let Some(slot) = slot {
        slot.set_child_pid(child.id())?;
    }
    let pgid = libc::pid_t::try_from(child.id())?;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)));
        }
        let signal = pending.swap(0, Ordering::SeqCst);
        if signal != 0 {
            // SAFETY: plain kill(2) on the child's process group.
            unsafe { libc::kill(-pgid, signal as libc::c_int) };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
