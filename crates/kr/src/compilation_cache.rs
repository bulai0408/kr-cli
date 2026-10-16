//! Xcode's compilation cache, shared by the worktrees of one project (ADR 0013).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
/// cache keys independent of where the worktree `root` is. The project mapping does not
/// cover the directory a package target compiles in (the one of the `.xcodeproj`), so
/// the whole worktree is mapped as well.
fn settings(root: &Path, dir: &Path) -> Vec<String> {
    vec![
        "COMPILATION_CACHE_ENABLE_CACHING=YES".to_owned(),
        format!("COMPILATION_CACHE_CAS_PATH={}", dir.display()),
        "COMPILATION_CACHE_ENABLE_DIAGNOSTIC_REMARKS=YES".to_owned(),
        "SWIFT_ENABLE_PREFIX_MAPPING=YES".to_owned(),
        "SWIFT_ENABLE_PROJECT_PREFIX_MAPPING=YES".to_owned(),
        "CLANG_ENABLE_PREFIX_MAPPING=YES".to_owned(),
        "CLANG_ENABLE_PROJECT_PREFIX_MAPPING=YES".to_owned(),
        format!("SWIFT_OTHER_PREFIX_MAPPINGS={}=/^worktree", root.display()),
        format!("CLANG_OTHER_PREFIX_MAPPINGS={}=/^worktree", root.display()),
    ]
}

/// The event fields of a build without the cache.
pub fn off() -> serde_json::Value {
    json!({ "compilation_cache": { "enabled": false } })
}

/// Runs one build step of the worktree `root` through the shared cache at `dir`. If it fails in a way the
/// cache may cause, builds once more with an empty cache at `clean` (removed after)
/// and lets that build decide. Both builds are recorded.
pub fn build(
    runner: &dyn Runner,
    steps: &mut Steps<'_>,
    step: &str,
    cmd: &Command,
    root: &Path,
    dir: &Path,
    clean: &Path,
) -> Result<bool> {
    let logs = root
        .join(crate::commands::check::DERIVED_DATA)
        .join("Logs/Build");
    let fields = |clean: bool, started: SystemTime| {
        let (hits, misses) = remarks(&logs, started).unzip();
        json!({ "compilation_cache": {
            "enabled": true, "clean": clean, "hits": hits, "misses": misses,
        } })
    };
    let shared = cmd.clone().args(settings(root, dir));
    let started = SystemTime::now();
    if steps.run_logged_with(runner, step, &shared, |_| fields(false, started))? {
        return Ok(true);
    }
    let first = steps.results.last().expect("the step was recorded");
    let Some(reason) = suspect(&first.summary) else {
        return Ok(false);
    };
    let first_log = first.log.clone();
    let _ = std::fs::remove_dir_all(clean);
    let again = cmd.clone().args(settings(root, clean));
    let retry_step = format!("{step} (clean cache)");
    let started = SystemTime::now();
    let ok = steps.run_logged_with(runner, &retry_step, &again, |_| fields(true, started));
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

/// The cache hits and misses Xcode reported in the activity log the build wrote to `logs`
/// (the newest one changed since `started`), or `None` if there is none. kr builds with
/// `-quiet`, which keeps these remarks out of the console. Each remark is a string record
/// of its own in the log: "Replay cache hit" / "Replay cache miss" for Swift jobs (lower
/// case for modules), "replayed cache hit: llvmcas://…" / "cache miss: llvmcas://…" for
/// Clang. The same words inside longer notes are not counted again.
fn remarks(logs: &Path, started: SystemTime) -> Option<(u64, u64)> {
    let newest = std::fs::read_dir(logs)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "xcactivitylog"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(modified, _)| *modified >= started)
        .max()?
        .1;
    let mut text = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(newest).ok()?)
        .read_to_end(&mut text)
        .ok()?;
    let text = text.to_ascii_lowercase();
    let count = |pattern: &[u8]| {
        text.windows(pattern.len())
            .filter(|w| *w == pattern)
            .count() as u64
    };
    let hits = count(b"16\"replay cache hit") + count(b"\"replayed cache hit: llvmcas://");
    let misses = count(b"17\"replay cache miss") + count(b"\"cache miss: llvmcas://");
    Some((hits, misses))
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
