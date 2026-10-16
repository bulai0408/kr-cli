use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;
use kr_events::{Event, EventKind, RunId};
use kr_resource::{Resource, Slot};
use serde_json::json;

use super::resource_parser;
use crate::{Ctx, slots};

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

pub fn run(ctx: &Ctx, args: LockArgs) -> Result<ExitCode> {
    kr_exec::catch_stop_signals()?;
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
            return run_command(&args.command, None, &held, &[]).map(exit_code);
        }
        log.append(&Event::now(
            run_id,
            project,
            EventKind::ReentryDenied,
            json!({ "resource": res, "slot": index, "task": task }),
        ))?;
    }

    let runner = ctx.runner()?;
    let mut taken = slots::take(ctx, &loaded, runner.as_ref(), run_id, res, &task)?;

    let mut child_held: Vec<(Resource, u32)> =
        held.into_iter().filter(|(r, _)| *r != res).collect();
    child_held.push((res, taken.slot.index()));
    let result = run_command(&args.command, Some(&mut taken.slot), &child_held, &[]);
    slots::give_back(&loaded, run_id, taken, result.as_ref().ok().copied())?;
    result.map(exit_code)
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

pub(crate) fn parse_held(value: &str) -> Vec<(Resource, u32)> {
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
pub(crate) fn run_command(
    command: &[String],
    slot: Option<&mut Slot>,
    held: &[(Resource, u32)],
    extra_env: &[(&str, &str)],
) -> Result<i32> {
    if let Some(signal) = kr_exec::stop_signal() {
        return Ok(128 + signal);
    }
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
        .envs(extra_env.iter().copied())
        .process_group(0)
        .spawn()
        .with_context(|| format!("failed to start `{}`", command.join(" ")))?;
    kr_exec::notify_spawn(kr_exec::Spawn::Started(child.id()));
    // Record the command's process group right away, so the slot counts as held
    // ("orphan") if kr dies. The signal handlers above keep SIGINT/TERM/HUP from
    // killing kr in between; only a SIGKILL during the microseconds between spawn and
    // this write leaves the command unrecorded.
    if let Some(slot) = slot {
        slot.set_child_pid(child.id())?;
    }
    let pgid = libc::pid_t::try_from(child.id())?;
    loop {
        if let Some(status) = child.try_wait()? {
            kr_exec::notify_spawn(kr_exec::Spawn::Finished(child.id()));
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
