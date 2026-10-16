//! The invocation log (ADR 0016): one line per kr call, so `kr report` can say which
//! commands an agent called and how often its arguments were wrong. Kept apart from the
//! event log, which holds the steps of runs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kr_events::{Event, EventKind, EventLog, RunId};
use serde_json::json;

use crate::Ctx;

/// Where the log of a state directory is.
pub fn log(state_dir: &Path) -> EventLog {
    EventLog::new(state_dir.join("invocations"))
}

/// Appends one call. Never fails the command: a call kr cannot log is not logged.
pub fn record(ctx: &Ctx, args: &[String], exit_code: u8, took: Duration, error: Option<String>) {
    let (state_dir, project, worktree): (Option<PathBuf>, Option<String>, Option<PathBuf>) =
        match ctx.load() {
            Ok(loaded) => (
                Some(loaded.state_dir.clone()),
                loaded.project().map(str::to_owned),
                loaded
                    .project_file
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_owned),
            ),
            Err(_) => (
                kr_config::MachineConfig::default().state_dir(&ctx.env),
                None,
                None,
            ),
        };
    let Some(state_dir) = state_dir else {
        return;
    };
    let fields = json!({
        "command": args.iter().find(|a| !a.starts_with('-')),
        "args": args,
        "exit_code": exit_code,
        "duration_ms": took.as_millis() as u64,
        "error": error.and_then(|e| e.lines().next().map(str::to_owned)),
        "requester": {
            "agent": ctx.env_var("KR_REQUESTER").or_else(|| ctx.env_var("AI_AGENT")),
            "worktree": worktree.unwrap_or_else(|| ctx.cwd.clone()),
            // SAFETY: getppid has no preconditions.
            "parent_pid": unsafe { libc::getppid() },
        },
    });
    let event = Event::now(
        RunId::new(),
        project.as_deref(),
        EventKind::Invocation,
        fields,
    );
    if let Err(e) = log(&state_dir).append(&event) {
        tracing::debug!("cannot log this call: {e:#}");
    }
}
