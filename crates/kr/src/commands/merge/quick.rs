use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;
use clap::Args;
use regex::Regex;
use serde_json::{Value, json};

use super::{checked_git, revision};
use crate::{Ctx, slots};
mod build;
mod reconcile;

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Compare this commit to HEAD and the current working tree.
    #[arg(long, default_value = "HEAD~1")]
    base: String,
    /// Reconcile this already decided merge resolution.
    #[arg(long, requires = "verification_run")]
    decision: Option<String>,
    /// The affected kr run/gate whose tests ran on this committed tree.
    #[arg(long, requires = "decision")]
    verification_run: Option<kr_events::RunId>,
}

fn problem(
    problems: &mut Vec<Value>,
    kind: &str,
    file: &str,
    line: usize,
    message: &str,
    next: &str,
) {
    problems.push(json!({"kind":kind,"file":file,"line":line,"message":message,"next":next}));
}

fn without_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut result = bytes.to_vec();
    let (mut i, mut depth, mut quoted) = (0, 0usize, false);
    while i < bytes.len() {
        if depth > 0 {
            if bytes[i..].starts_with(b"/*") {
                depth += 1;
                result[i..i + 2].fill(b' ');
                i += 2;
            } else if bytes[i..].starts_with(b"*/") {
                depth -= 1;
                result[i..i + 2].fill(b' ');
                i += 2;
            } else {
                if bytes[i] != b'\n' {
                    result[i] = b' ';
                }
                i += 1;
            }
        } else if quoted {
            if bytes[i] == b'\\' {
                i = (i + 2).min(bytes.len());
            } else {
                if bytes[i] == b'"' {
                    quoted = false;
                }
                i += 1;
            }
        } else if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                result[i] = b' ';
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            depth = 1;
            result[i..i + 2].fill(b' ');
            i += 2;
        } else {
            if bytes[i] == b'"' {
                quoted = true;
            }
            i += 1;
        }
    }
    String::from_utf8(result).expect("only complete comment bytes were replaced")
}

fn migrations(file: &str, text: &str, problems: &mut Vec<Value>) {
    let clean = without_comments(text);
    let text = clean.as_str();
    let registration = Regex::new(r#"registerMigration\s*\(\s*"([^"]+)""#).unwrap();
    let suffix = Regex::new(r"^(.*?)(\d+)$").unwrap();
    let sql = Regex::new(r#"(?i)\b(DROP|CREATE)\s+TABLE\s+(IF\s+(?:NOT\s+)?EXISTS\s+)?["`\[]?([a-zA-Z_][a-zA-Z_0-9.]*)"#).unwrap();
    let mut names = BTreeSet::new();
    let mut versions = BTreeMap::<String, u64>::new();
    let captures: Vec<_> = registration.captures_iter(text).collect();
    for cap in &captures {
        let start = cap.get(0).unwrap().start();
        let line = text[..start].lines().count() + 1;
        let name = &cap[1];
        if !names.insert(name.to_owned()) {
            problem(
                problems,
                "duplicate_migration",
                file,
                line,
                "migration ID is registered twice",
                "keep one registration and preserve both changes",
            );
        }
        if let Some(n) = suffix.captures(name)
            && let Ok(version) = n[2].parse::<u64>()
        {
            if versions.get(&n[1]).is_some_and(|old| *old >= version) {
                problem(
                    problems,
                    "migration_order",
                    file,
                    line,
                    "migration versions must increase",
                    "renumber the new migration after existing versions",
                );
            }
            versions.insert(n[1].to_owned(), version);
        }
    }
    // Test upgrade fixtures also execute migration SQL, outside registerMigration.
    // Keep functions and migrations separate so independent resets are allowed.
    let scope = Regex::new(r#"\bfunc\s+\w+|registerMigration\s*\(\s*"[^"]+""#).unwrap();
    let mut starts: Vec<usize> = scope.find_iter(text).map(|m| m.start()).collect();
    starts.insert(0, 0);
    starts.push(text.len());
    for range in starts.windows(2) {
        let body = &text[range[0]..range[1]];
        let line = text[..range[0]].lines().count() + 1;
        let mut dropped = BTreeSet::new();
        for statement in sql.captures_iter(body) {
            let table = statement[3].to_ascii_lowercase();
            if statement[1].eq_ignore_ascii_case("CREATE") {
                dropped.remove(&table);
            } else if statement.get(2).is_some() {
                dropped.insert(table);
            } else if !dropped.insert(table) {
                let at = line
                    + body[..statement.get(0).unwrap().start()]
                        .lines()
                        .count()
                        .saturating_sub(1);
                problem(
                    problems,
                    "duplicate_drop",
                    file,
                    at,
                    "table is dropped twice in one migration or upgrade fixture without recreation",
                    "remove the duplicate DROP introduced by conflict resolution",
                );
            }
        }
    }
}

fn credentials(file: &str, text: &str, problems: &mut Vec<Value>) {
    // Detection is deliberately bounded. Never attach the match or source line.
    let token = Regex::new(r"(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|AKIA[A-Z0-9]{16}|sk-(?:proj-)?[A-Za-z0-9_-]{20,}|-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----)").unwrap();
    let assignment = Regex::new(r#"(?i)(?:api[_-]?key|client[_-]?secret|password|access[_-]?token)["']?\s*[:=]\s*["']?([^\s"'#;]{12,})"#).unwrap();
    for (n, line) in text.lines().enumerate() {
        let assigned = assignment.captures_iter(line).any(|c| {
            let v = &c[1];
            !v.starts_with('$')
                && !v.starts_with('<')
                && !v.contains("${")
                && !["example", "placeholder", "redacted", "your_", "your-"]
                    .iter()
                    .any(|s| v.to_ascii_lowercase().contains(s))
        });
        if token.is_match(line) || assigned {
            problem(
                problems,
                "credential",
                file,
                n + 1,
                "possible credential value (redacted)",
                "remove the value, rotate it if real, and read it from the configured credential location",
            );
        }
    }
}

fn scenarios(ctx: &Ctx, files: &[String], problems: &mut Vec<Value>) -> Result<()> {
    if !ctx.cwd.join(".kr.toml").exists() {
        return Ok(());
    }
    let loaded = match ctx.load() {
        Ok(loaded) => loaded,
        Err(_) => {
            problem(
                problems,
                "scenario_config",
                ".kr.toml",
                1,
                "project configuration is invalid",
                "run kr config and fix the reported configuration item",
            );
            return Ok(());
        }
    };
    let Some(project) = &loaded.config.project else {
        return Ok(());
    };
    let class = Regex::new(r"\bclass\s+([A-Za-z_][A-Za-z_0-9]*)\b").unwrap();
    let mut found = BTreeSet::new();
    let mut candidates = BTreeSet::new();
    for file in files.iter().filter(|f| f.ends_with(".swift")) {
        if let Ok(text) = std::fs::read_to_string(ctx.cwd.join(file)) {
            let text = without_comments(&text);
            for cap in class.captures_iter(&text) {
                let name = cap[1].to_owned();
                if name.ends_with("UITests")
                    || (file.contains("UITests/") && name.ends_with("Tests"))
                {
                    candidates.insert(name.clone());
                }
                found.insert(name);
            }
        }
    }
    let registered: BTreeSet<String> = project
        .scenarios
        .values()
        .flat_map(|s| &s.only)
        .filter_map(|filter| filter.split('/').nth(1).map(str::to_owned))
        .collect();
    for name in registered.difference(&found) {
        problem(
            problems,
            "scenario_missing_class",
            ".kr.toml",
            1,
            &format!("registered UI test class {name} was not found"),
            "restore the class or update scenarios.only",
        );
    }
    for name in candidates.difference(&registered) {
        problem(
            problems,
            "scenario_unregistered",
            ".kr.toml",
            1,
            &format!("UI test class {name} has no scenario"),
            "add it to scenarios.only in .kr.toml",
        );
    }
    Ok(())
}

fn native_input(file: &str) -> bool {
    Path::new(file).extension().is_some_and(|e| {
        matches!(
            e.to_str(),
            Some(
                "swift"
                    | "m"
                    | "mm"
                    | "h"
                    | "hpp"
                    | "c"
                    | "cc"
                    | "cpp"
                    | "metal"
                    | "modulemap"
                    | "pbxproj"
                    | "xcconfig"
            )
        )
    }) || file.ends_with("Package.resolved")
}

pub(super) fn run(ctx: &Ctx, args: CheckArgs) -> Result<ExitCode> {
    let started = std::time::Instant::now();
    let runner = ctx.runner()?;
    let root = checked_git(ctx, &*runner, &["rev-parse", "--show-toplevel"])?;
    let local = Ctx {
        cwd: root.trim().into(),
        env: ctx.env.clone(),
        json: ctx.json,
    };
    let ctx = &local;
    let base = revision(ctx, &*runner, &args.base)?;
    let diff = checked_git(ctx, &*runner, &["diff", "--name-only", "-z", &base, "--"])?;
    let untracked = checked_git(
        ctx,
        &*runner,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let changed: BTreeSet<String> = diff
        .split('\0')
        .chain(untracked.split('\0'))
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
        .collect();
    let tracked = checked_git(ctx, &*runner, &["ls-files", "-z"])?;
    let all: Vec<String> = tracked
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(str::to_owned)
        .chain(changed.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut problems = Vec::new();
    for file in &changed {
        let path = ctx.cwd.join(file);
        if !path.exists() {
            continue;
        }
        if path.is_symlink() {
            problem(
                &mut problems,
                "symlink",
                file,
                1,
                "changed symlink was not scanned",
                "review its target manually; keep credentials outside the repository",
            );
            continue;
        }
        let bytes = std::fs::read(&path)?;
        if file.ends_with(".json") || file.ends_with(".xcstrings") {
            if super::json::parse(&bytes).is_err() {
                problem(
                    &mut problems,
                    "invalid_json",
                    file,
                    1,
                    "JSON parse failed",
                    "repair the JSON after resolving the merge",
                );
            } else if file.ends_with(".xcstrings") {
                let out = runner.run(
                    &kr_exec::Command::new("xcrun")
                        .args(["xcstringstool", "print", file])
                        .current_dir(&ctx.cwd),
                )?;
                if !out.success() {
                    problem(
                        &mut problems,
                        "invalid_catalog",
                        file,
                        1,
                        "xcstringstool rejected the catalog",
                        "repair the catalog in Xcode",
                    );
                }
            }
        }
        if let Ok(text) = std::str::from_utf8(&bytes) {
            credentials(file, text, &mut problems);
            if file.ends_with(".swift") {
                migrations(file, text, &mut problems);
            }
        }
    }
    scenarios(ctx, &all, &mut problems)?;
    let source_changed = changed.iter().any(|f| native_input(f));
    let static_duration = started.elapsed().as_millis();
    let mut builds = Vec::new();
    if source_changed && problems.is_empty() {
        builds = build::run(ctx, &*runner, &changed, &mut problems)?;
    }
    let compiled = !builds.is_empty() && builds.iter().all(|b| b["ok"] == true);
    let reconciliation = match (&args.decision, args.verification_run) {
        (Some(id), Some(run)) => reconcile::run(
            ctx,
            &*runner,
            id,
            run,
            compiled,
            !builds.is_empty(),
            &mut problems,
        )?,
        _ => None,
    };
    let ok = problems.is_empty();
    let value = json!({"ok":ok,"concluded":true,"changed":changed,"problems":problems,
        "compiled":compiled,"builds":builds,"reconciliation":reconciliation,"static_duration_ms":static_duration,
        "credential_scan":"bounded patterns; absence of matches is not proof of absence"});
    if ctx.json {
        crate::output::print_json(&value)?;
    } else {
        println!(
            "Merge check: {} ({} problem(s)).",
            if ok { "PASS" } else { "FAIL" },
            problems.len()
        );
        for p in &problems {
            println!(
                "{}:{}: {}. Next: {}",
                p["file"], p["line"], p["message"], p["next"]
            );
        }
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
