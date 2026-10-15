//! `kr rejudge` (ADR 0010, level 3): judges saved results again with this kr's rules
//! and lists only the verdicts that change. Reads the `verdict.json` that every
//! `kr run` / `kr gate` saves per item and, with `--evidence`, result bundles from
//! before kr. The only command it runs is xcresulttool, once per new bundle, on a copy:
//! xcresulttool writes a database into a bundle it reads.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Args;
use kr_exec::Runner;
use kr_run::judge;
use kr_run::rejudge::{Change, Evidence, SCHEMA_VERSION, Saved, parse_enabled_tests};
use kr_xcode::results_command;
use serde::Serialize;

use crate::Ctx;

#[derive(Debug, Args)]
pub struct RejudgeArgs {
    /// Runs to judge again, by run ID or its start. Default: every saved run.
    runs: Vec<String>,

    /// Also judge the result bundles (`*.xcresult`) under this directory, e.g. Krill's
    /// v0-evidence. kr does not write there: it reads each new bundle once from a copy
    /// and keeps what it read in its state directory. A `status` file next to a bundle
    /// is the exit code, an `enabled-tests.txt` the enumeration.
    #[arg(long, value_name = "DIR")]
    evidence: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    runs: usize,
    items: usize,
    /// `None` without `--evidence`.
    evidence: Option<EvidenceCount>,
    changed: usize,
    changes: Vec<Changed>,
    /// Saved files that cannot be read (e.g. written by a newer kr).
    unreadable: Vec<PathBuf>,
}

#[derive(Debug, Default, Serialize)]
struct EvidenceCount {
    bundles: usize,
    /// Bundles read with xcresulttool this time; the others came from the cache.
    read: usize,
}

#[derive(Debug, Serialize)]
struct Changed {
    /// "run": a saved kr run; "evidence": a bundle under `--evidence`.
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundle: Option<PathBuf>,
    #[serde(flatten)]
    change: Change,
}

pub fn run(ctx: &Ctx, args: RejudgeArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let mut report = Report {
        schema_version: SCHEMA_VERSION,
        runs: 0,
        items: 0,
        evidence: None,
        changed: 0,
        changes: Vec::new(),
        unreadable: Vec::new(),
    };
    for (run_id, dir) in saved_runs(&loaded.state_dir.join("runs"), &args.runs)? {
        report.runs += 1;
        for file in verdict_files(&dir) {
            let saved = std::fs::read(&file)
                .ok()
                .and_then(|b| serde_json::from_slice::<Saved>(&b).ok());
            let Some(saved) = saved else {
                report.unreadable.push(file);
                continue;
            };
            report.items += 1;
            if let Some(change) = saved.rejudge(&judge) {
                report.changes.push(Changed {
                    source: "run",
                    run_id: Some(run_id.clone()),
                    item: Some(saved.item),
                    bundle: None,
                    change,
                });
            }
        }
    }
    if !args.evidence.is_empty() {
        let runner = ctx.runner()?;
        let cache = loaded.state_dir.join("rejudge/evidence");
        let mut count = EvidenceCount::default();
        for dir in &args.evidence {
            for bundle in bundles(dir)? {
                count.bundles += 1;
                let (evidence, read) = evidence(&bundle, &cache, runner.as_ref())?;
                count.read += usize::from(read);
                if let Some(change) = evidence.rejudge(&judge) {
                    report.changes.push(Changed {
                        source: "evidence",
                        run_id: None,
                        item: None,
                        bundle: Some(bundle),
                        change,
                    });
                }
            }
        }
        report.evidence = Some(count);
    }
    report.changed = report.changes.len();
    print(ctx, &report)?;
    Ok(ExitCode::SUCCESS)
}

/// Run directories with saved verdicts, oldest first (run IDs sort by time).
fn saved_runs(runs: &Path, wanted: &[String]) -> Result<Vec<(String, PathBuf)>> {
    let Ok(entries) = std::fs::read_dir(runs) else {
        return Ok(Vec::new());
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.file_name().to_str()?.to_owned(), e.path())))
        .filter(|(id, dir)| {
            (wanted.is_empty() || wanted.iter().any(|w| id.starts_with(w.as_str())))
                && !verdict_files(dir).is_empty()
        })
        .collect();
    found.sort();
    for w in wanted {
        if !found.iter().any(|(id, _)| id.starts_with(w.as_str())) {
            return Err(anyhow::anyhow!(
                "no saved verdicts for run `{w}` in {}; runs from before kr saved them cannot be judged again",
                runs.display()
            ))
            .context("nothing to judge");
        }
    }
    Ok(found)
}

/// `<run>/<item>/verdict.json`, by item directory name.
fn verdict_files(run: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(run) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path().join("verdict.json"))
        .filter(|f| f.is_file())
        .collect();
    files.sort();
    files
}

/// Result bundles under `dir`, in path order; does not look inside a bundle.
fn bundles(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    paths.sort();
    for path in paths {
        if path.extension().is_some_and(|x| x == "xcresult") {
            found.push(path);
        } else {
            found.extend(bundles(&path)?);
        }
    }
    Ok(found)
}

/// The bundle's cached reading, or a new one; true if it was read now.
fn evidence(bundle: &Path, cache: &Path, runner: &dyn Runner) -> Result<(Evidence, bool)> {
    let bundle = std::path::absolute(bundle)?;
    let file = cache.join(format!(
        "{}.json",
        blake3::hash(bundle.as_os_str().as_encoded_bytes()).to_hex()
    ));
    if let Some(cached) = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice::<Evidence>(&b).ok())
    {
        return Ok((cached, false));
    }
    let dir = bundle.parent().unwrap_or(Path::new(""));
    let mut evidence = Evidence {
        schema_version: SCHEMA_VERSION,
        exit: std::fs::read_to_string(dir.join("status"))
            .ok()
            .and_then(|s| s.trim().parse().ok()),
        enumeration: std::fs::read_to_string(dir.join("enabled-tests.txt"))
            .ok()
            .map(|t| parse_enabled_tests(&t)),
        results: read_copy(&bundle, runner)?,
        verdict: Default::default(),
        bundle,
    };
    evidence.verdict = evidence.judge(&judge);
    std::fs::create_dir_all(cache)?;
    std::fs::write(&file, serde_json::to_vec(&evidence)?)
        .with_context(|| format!("failed to write {}", file.display()))?;
    Ok((evidence, true))
}

/// xcresulttool's test results of a copy of `bundle`; `None` if it cannot read them.
fn read_copy(bundle: &Path, runner: &dyn Runner) -> Result<Option<String>> {
    let tmp = std::env::temp_dir().join(format!("kr-rejudge-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let copy = tmp.join("bundle.xcresult");
    copy_dir(bundle, &copy).with_context(|| format!("cannot copy {}", bundle.display()))?;
    let out = runner.run(&results_command(Path::new("bundle.xcresult")).current_dir(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
    let out = out?;
    Ok(out.success().then_some(out.stdout))
}

/// Copies a directory tree; on APFS each file is a clone, so even large bundles are
/// cheap to copy.
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn print(ctx: &Ctx, report: &Report) -> Result<()> {
    if ctx.json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    let verdict = |passed: bool| if passed { "PASS" } else { "FAIL" };
    let outcome = |o: Option<kr_run::Final>| {
        o.and_then(|o| serde_json::to_value(o).ok())
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "absent".to_owned())
    };
    for c in &report.changes {
        let what = match (&c.run_id, &c.item, &c.bundle) {
            (Some(run), Some(item), _) => format!("{run} {item}"),
            (_, _, Some(bundle)) => bundle.display().to_string(),
            _ => String::new(),
        };
        println!(
            "CHANGED: {what}: {} → {}",
            verdict(c.change.before.passed),
            verdict(c.change.after.passed)
        );
        for t in &c.change.tests {
            println!("  {}: {} → {}", t.id, outcome(t.before), outcome(t.after));
        }
        if c.change.before.passed != c.change.after.passed {
            for reason in &c.change.after.reasons {
                println!("  now: {reason}");
            }
        }
    }
    for file in &report.unreadable {
        eprintln!("warning: cannot read {}; skipped it", file.display());
    }
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("{n} {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let changed = match report.changed {
        0 => "no verdict changed".to_owned(),
        n => format!("{} changed", plural(n, "verdict", "verdicts")),
    };
    let evidence = report.evidence.as_ref().map_or_else(String::new, |e| {
        format!(
            ", {} ({} read now)",
            plural(e.bundles, "evidence bundle", "evidence bundles"),
            e.read
        )
    });
    println!(
        "rejudge: {} in {}{evidence}; {changed}",
        plural(report.items, "item", "items"),
        plural(report.runs, "run", "runs"),
    );
    Ok(())
}
