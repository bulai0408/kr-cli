//! Xcode's compilation cache, shared by the worktrees of one project (ADR 0013).

use std::path::{Path, PathBuf};

use anyhow::Result;
use kr_config::ProjectConfig;
use kr_events::{Event, EventKind};
use kr_exec::{Command, Runner};
use serde_json::json;

use crate::Loaded;
use crate::commands::check::Steps;

/// The cache of the project, `<state>/cache/<project>/compilation`, if `.kr.toml`
/// turns it on.
pub fn dir(project: &ProjectConfig, loaded: &Loaded) -> Option<PathBuf> {
    project.project.compilation_cache.then(|| {
        loaded
            .state_dir
            .join("cache")
            .join(&project.project.name)
            .join("compilation")
    })
}

/// Build settings that compile through the cache at `dir`. Prefix mapping makes the
/// cache keys independent of where the worktree is.
fn settings(dir: &Path) -> Vec<String> {
    vec![
        "COMPILATION_CACHE_ENABLE_CACHING=YES".to_owned(),
        format!("COMPILATION_CACHE_CAS_PATH={}", dir.display()),
        "COMPILATION_CACHE_ENABLE_DIAGNOSTIC_REMARKS=YES".to_owned(),
        "SWIFT_ENABLE_PREFIX_MAPPING=YES".to_owned(),
        "SWIFT_ENABLE_PROJECT_PREFIX_MAPPING=YES".to_owned(),
        "CLANG_ENABLE_PREFIX_MAPPING=YES".to_owned(),
        "CLANG_ENABLE_PROJECT_PREFIX_MAPPING=YES".to_owned(),
    ]
}

/// The event fields of a build without the cache.
pub fn off() -> serde_json::Value {
    json!({ "compilation_cache": { "enabled": false } })
}

/// Runs one build step through the shared cache at `dir`. If it fails in a way the
/// cache may cause, builds once more with an empty cache at `clean` (removed after)
/// and lets that build decide. Both builds are recorded.
pub fn build(
    runner: &dyn Runner,
    steps: &mut Steps<'_>,
    step: &str,
    cmd: &Command,
    dir: &Path,
    clean: &Path,
) -> Result<bool> {
    let fields = |clean: bool| json!({ "compilation_cache": { "enabled": true, "clean": clean } });
    let shared = cmd.clone().args(settings(dir));
    if steps.run_logged_with(runner, step, &shared, |_| fields(false))? {
        return Ok(true);
    }
    let first = steps.results.last().expect("the step was recorded");
    let Some(reason) = suspect(&first.summary) else {
        return Ok(false);
    };
    let first_log = first.log.clone();
    let _ = std::fs::remove_dir_all(clean);
    let again = cmd.clone().args(settings(clean));
    let retry_step = format!("{step} (clean cache)");
    let ok = steps.run_logged_with(runner, &retry_step, &again, |_| fields(true));
    let _ = std::fs::remove_dir_all(clean);
    let ok = ok?;
    let retry = steps.results.pop().expect("the retry was recorded");
    steps.loaded.events().append(&Event::now(
        steps.run_id,
        steps.loaded.project(),
        EventKind::CompilationCacheRetry,
        json!({
            "step": step,
            "reason": reason,
            "first_ok": false,
            "first_log": first_log,
            "ok": ok,
            "log": retry.log,
            "cache_caused": ok,
        }),
    ))?;
    // The clean build decides the step.
    let result = steps.results.last_mut().expect("the step was recorded");
    result.ok = ok;
    result.duration_ms += retry.duration_ms;
    if ok {
        result.summary = Vec::new();
        result.log = retry.log;
        result.note = Some(format!(
            "passed with a clean compilation cache after failing with the shared one ({reason}); if this happens again, delete {}",
            dir.display()
        ));
    } else {
        result.summary = retry.summary;
        result.log = retry.log;
        result.note = Some(
            "failed again with a clean compilation cache, so the cache is not the cause".to_owned(),
        );
    }
    Ok(ok)
}

/// Why a failed build may be the cache's fault, or `None` if it is not. A compiler
/// error at a source line is not: failed compilations are not cached, so the error
/// came from compiling the source. Anything else may be (an error naming the cache,
/// a link or module error from a bad cached output).
fn suspect(summary: &[String]) -> Option<String> {
    let names_cache = |l: &String| {
        let l = l.to_lowercase();
        l.contains("llvmcas") || l.contains("compilation cach") || l.contains("cached output")
    };
    if let Some(line) = summary.iter().find(|l| names_cache(l)) {
        return Some(format!("the error names the cache: {line}"));
    }
    let at_source_line = |l: &String| {
        l.split_once(": error:")
            .is_some_and(|(loc, _)| loc.matches(':').count() >= 2 && !loc.contains(' '))
    };
    if summary.iter().any(at_source_line) {
        return None;
    }
    Some("the build failed without a compiler error at a source line".to_owned())
}
