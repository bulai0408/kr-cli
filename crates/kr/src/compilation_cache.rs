//! Xcode's compilation cache, shared by the worktrees of one project (ADR 0013).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use kr_config::ProjectConfig;
use kr_events::{Event, EventKind};
use kr_exec::{Command, Runner};
use serde::Serialize;
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

const GIB: u64 = 1 << 30;
/// The default limit is the smaller of this and `1 / DEFAULT_DISK_SHARE` of the disk.
const DEFAULT_MAX: u64 = 20 * GIB;
const DEFAULT_DISK_SHARE: u64 = 20;

/// How large Xcode lets one project's cache grow (ADR 0029). Xcode starts a new
/// generation of the cache once the newest one passes half the limit and deletes the
/// generation before the previous one at the start of the next build, so the cache
/// settles near the limit instead of at Xcode's default of half the free disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Limit {
    /// 0 means no limit.
    pub bytes: u64,
    /// Where the value comes from: `[compilation_cache] limit` or the default formula.
    pub source: String,
}

impl Limit {
    /// `[compilation_cache] limit` if set, else the default for a disk of `disk_bytes`.
    pub fn new(machine: &kr_config::MachineConfig, disk_bytes: u64) -> Result<Self> {
        if let Some(text) = &machine.compilation_cache.limit {
            let bytes = parse_size(text).with_context(|| {
                format!(
                    "[compilation_cache] limit = \"{text}\" in the machine config is not a size; write bytes or a number ending in K, M, G or T, e.g. \"30G\" (\"0\" for no limit)"
                )
            })?;
            return Ok(Self {
                bytes,
                source: "[compilation_cache] limit".to_owned(),
            });
        }
        Ok(Self {
            bytes: DEFAULT_MAX.min(disk_bytes / DEFAULT_DISK_SHARE),
            source: format!(
                "default: min(20 GB, 5% of the {} GB disk)",
                disk_bytes / GIB
            ),
        })
    }
}

/// Size of the disk that holds `path` (its nearest existing ancestor), 0 if unknown.
pub fn disk_bytes(path: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    path.ancestors()
        .find_map(|p| {
            let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0)
                .then(|| st.f_blocks as u64 * st.f_frsize)
        })
        .unwrap_or(0)
}

/// `30G` → bytes. Suffixes are powers of 1024; no suffix is bytes.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, unit) = match text.char_indices().last()? {
        (i, c) if c.is_ascii_alphabetic() => (&text[..i], c.to_ascii_uppercase()),
        _ => (text, 'B'),
    };
    let shift = match unit {
        'B' => 0,
        'K' => 10,
        'M' => 20,
        'G' => 30,
        'T' => 40,
        _ => return None,
    };
    digits.parse::<u64>().ok()?.checked_mul(1 << shift)
}

/// Build settings that compile through the cache at `dir`, kept under `limit`. Prefix
/// mapping makes the cache keys independent of where the worktree `root` is. The
/// project mapping does not cover the directory a package target compiles in (the one
/// of the `.xcodeproj`), so the whole worktree is mapped as well.
fn settings(root: &Path, dir: &Path, limit: &Limit) -> Vec<String> {
    vec![
        "COMPILATION_CACHE_ENABLE_CACHING=YES".to_owned(),
        format!("COMPILATION_CACHE_CAS_PATH={}", dir.display()),
        format!("COMPILATION_CACHE_LIMIT_SIZE={}", limit.bytes),
        "COMPILATION_CACHE_ENABLE_DIAGNOSTIC_REMARKS=YES".to_owned(),
        "SWIFT_ENABLE_PREFIX_MAPPING=YES".to_owned(),
        "SWIFT_ENABLE_PROJECT_PREFIX_MAPPING=YES".to_owned(),
        "CLANG_ENABLE_PREFIX_MAPPING=YES".to_owned(),
        "CLANG_ENABLE_PROJECT_PREFIX_MAPPING=YES".to_owned(),
        format!("SWIFT_OTHER_PREFIX_MAPPINGS={}=/^worktree", root.display()),
        format!("CLANG_OTHER_PREFIX_MAPPINGS={}=/^worktree", root.display()),
    ]
}

/// What `kr lock --compilation-cache` does for one command (ADR 0028).
pub enum ForCommand {
    /// The child gets `XCODE_XCCONFIG_FILE=<xcconfig>`, which compiles through `dir`.
    On { dir: PathBuf, xcconfig: PathBuf },
    /// The command runs without the cache, for this reason.
    Ignored(String),
}

impl ForCommand {
    /// Decides for `command`, run from the project of `loaded`. The settings go in an
    /// xcconfig written to `run_dir`: xcodebuild reads `XCODE_XCCONFIG_FILE` for every
    /// target, package targets included, and lets it override all other settings.
    pub fn decide(
        loaded: &Loaded,
        callers_xcconfig: Option<&str>,
        command: &[String],
        run_dir: &Path,
    ) -> Result<Self> {
        let ignored = |reason: String| Ok(Self::Ignored(reason));
        let Ok((project, root)) = loaded.require_project() else {
            return ignored(
                "no .kr.toml found from here up to the repo root; the cache is a project setting"
                    .to_owned(),
            );
        };
        let Some(dir) = dir(project, loaded) else {
            let file = loaded
                .project_file
                .as_deref()
                .unwrap_or(Path::new(".kr.toml"));
            return ignored(format!(
                "{} does not turn the cache on; add `compilation_cache = true` under [project]",
                file.display()
            ));
        };
        if let Some(temp) = symlinked_temp(&root) {
            return ignored(format!(
                "the worktree {} is under {temp}, a symlink to /private{temp}; Xcode and SwiftPM see different paths there and the cache never hits. Put the worktree under /Users",
                root.display()
            ));
        }
        if let Some(theirs) = callers_xcconfig {
            return ignored(format!(
                "XCODE_XCCONFIG_FILE is already set to {theirs}; kr does not replace it. Unset it, or add the cache settings (ADR 0013) to that file"
            ));
        }
        if command.iter().any(|a| a == "archive") {
            return ignored(
                "the command archives; prefix mapping rewrites the source paths in its debug information"
                    .to_owned(),
            );
        }
        let limit = Limit::new(&loaded.config.machine, disk_bytes(&dir))?;
        std::fs::create_dir_all(run_dir)?;
        let xcconfig = run_dir.join("compilation-cache.xcconfig");
        let lines: String = settings(&root, &dir, &limit)
            .iter()
            .filter_map(|s| s.split_once('='))
            .map(|(k, v)| format!("{k} = {v}\n"))
            .collect();
        std::fs::write(&xcconfig, lines)?;
        Ok(Self::On { dir, xcconfig })
    }

    /// The `compilation_cache` field of the `slot_released` event.
    pub fn event(&self) -> serde_json::Value {
        match self {
            Self::On { dir, xcconfig } => {
                json!({ "enabled": true, "dir": dir, "xcconfig": xcconfig })
            }
            Self::Ignored(reason) => json!({ "enabled": false, "ignored": reason }),
        }
    }
}

/// `/tmp` or `/var` if `root` is under one of them (macOS links both to /private).
fn symlinked_temp(root: &Path) -> Option<&'static str> {
    ["/tmp", "/var"]
        .into_iter()
        .find(|t| root.starts_with(t) || root.starts_with(Path::new("/private").join(&t[1..])))
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
    let limit = Limit::new(&steps.loaded.config.machine, disk_bytes(dir))?;
    let shared = cmd.clone().args(settings(root, dir, &limit));
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
    let again = cmd.clone().args(settings(root, clean, &limit));
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

/// One project's shared cache on disk.
#[derive(Debug, Serialize)]
pub struct Usage {
    pub project: String,
    pub dir: PathBuf,
    pub bytes: u64,
    /// The newest modification time of anything in the cache.
    pub last_used: Option<jiff::Timestamp>,
}

/// The caches under `<state>/cache/<project>/compilation`, largest first.
pub fn usage(state_dir: &Path) -> Vec<Usage> {
    let Ok(projects) = std::fs::read_dir(state_dir.join("cache")) else {
        return Vec::new();
    };
    let mut all: Vec<Usage> = projects
        .filter_map(|e| e.ok())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.path().join("compilation"),
            )
        })
        .filter(|(_, dir)| dir.is_dir())
        .map(|(project, dir)| {
            let (bytes, newest) = walk(&dir);
            Usage {
                project,
                dir,
                bytes,
                last_used: newest.and_then(|t| jiff::Timestamp::try_from(t).ok()),
            }
        })
        .collect();
    all.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.project.cmp(&b.project)));
    all
}

/// Allocated bytes and newest modification time of the files under `dir`.
fn walk(dir: &Path) -> (u64, Option<SystemTime>) {
    use std::os::unix::fs::MetadataExt;
    let mut bytes = 0;
    let mut newest = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
                continue;
            }
            bytes += meta.blocks() * 512;
            newest = newest.max(meta.modified().ok());
        }
    }
    (bytes, newest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(limit: Option<&str>) -> kr_config::MachineConfig {
        let mut m = kr_config::MachineConfig::default();
        m.compilation_cache.limit = limit.map(str::to_owned);
        m
    }

    #[test]
    fn the_default_limit_is_twenty_gib_or_five_percent_of_a_small_disk() {
        let on = |disk_gib: u64| Limit::new(&machine(None), disk_gib * GIB).unwrap().bytes;
        assert_eq!(on(926), 20 * GIB);
        assert_eq!(on(256), 256 * GIB / 20);
        assert!(
            Limit::new(&machine(None), 926 * GIB)
                .unwrap()
                .source
                .contains("926 GB")
        );
    }

    #[test]
    fn a_configured_limit_wins_and_zero_means_none() {
        assert_eq!(
            Limit::new(&machine(Some("30G")), GIB).unwrap().bytes,
            30 * GIB
        );
        assert_eq!(
            Limit::new(&machine(Some("512m")), GIB).unwrap().bytes,
            512 << 20
        );
        assert_eq!(Limit::new(&machine(Some("1000")), GIB).unwrap().bytes, 1000);
        assert_eq!(Limit::new(&machine(Some("0")), GIB).unwrap().bytes, 0);
    }

    #[test]
    fn an_unreadable_limit_names_the_key_and_an_example() {
        for bad in ["", "G", "30GB", "-1G", "1.5G", "99999999999T"] {
            let err = format!("{:#}", Limit::new(&machine(Some(bad)), GIB).unwrap_err());
            assert!(
                err.contains("[compilation_cache] limit") && err.contains("30G"),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn usage_reports_each_project_cache_and_tolerates_a_missing_directory() {
        let state = tempfile::tempdir().unwrap();
        assert!(usage(state.path()).is_empty());
        let dir = state.path().join("cache/Demo/compilation/builtin/v1.1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("v9.data"), vec![1u8; 64 << 10]).unwrap();
        std::fs::create_dir_all(state.path().join("cache/Other")).unwrap();
        let all = usage(state.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].project, "Demo");
        assert!(all[0].bytes >= 64 << 10);
        assert!(all[0].last_used.is_some());
    }
}
