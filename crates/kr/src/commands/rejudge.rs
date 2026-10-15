//! `kr rejudge` (ADR 0010, level 3): judges saved results again with this kr's rules
//! and lists only the verdicts that change. Reads the `verdict.json` that every
//! `kr run` / `kr gate` saves per item and, with `--evidence`, result bundles from
//! before kr. The only command it runs is xcresulttool, once per new bundle, on a copy:
//! xcresulttool writes a database into a bundle it reads.
//!
//! It also lists the decision points this kr's rules raise on the saved runs (ADR 0011,
//! ADR 0015), with what was decided and how later runs reconcile it. It writes no events.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Args;
use jiff::Timestamp;
use kr_events::{Event, EventKind, RunId};
use kr_exec::Runner;
use kr_run::decision::{suggest, unseen};
use kr_run::rejudge::{Change, Evidence, SCHEMA_VERSION, Saved, parse_enabled_tests};
use kr_run::{Verdict, judge};
use kr_xcode::results_command;
use serde::Serialize;

use super::decide;
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
    /// Failures of saved runs that this kr leaves to the agent: what it would ask now.
    decision_points: Vec<Point>,
}

/// A decision point of a saved run under this kr's rules.
#[derive(Debug, Serialize)]
struct Point {
    run_id: String,
    item: String,
    test: String,
    signature: Option<String>,
    category: Option<kr_run::Category>,
    /// kr's default now.
    default: String,
    /// The decision point's ID if the run raised it when it ran; `None` for a run of a
    /// kr without decision points, which judged the failure FAIL.
    id: Option<String>,
    /// What was decided: choice, reason, via.
    decision: Option<serde_json::Value>,
    /// The `reconciliation` event, or (with `recorded: false`) what the later runs in
    /// the events say now.
    reconciliation: Option<serde_json::Value>,
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
        decision_points: Vec::new(),
    };
    let events = loaded
        .events()
        .read_since(Timestamp::UNIX_EPOCH)
        .unwrap_or_default();
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
            if let Ok(id) = run_id.parse::<RunId>() {
                report.decision_points.extend(points(&events, id, &saved));
            }
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

/// The decision points this kr raises on one saved item, as `kr run` would have at the
/// end of that run: failures not seen in earlier runs' events.
fn points(events: &[Event], run_id: RunId, saved: &Saved) -> Vec<Point> {
    let verdict = judge(&saved.enumeration, &saved.first, &saved.reruns);
    let cases = kr_run::cases(&verdict, saved.first.timed_out);
    // A failed test that was not run again (an invalid filter, a lost device) fails the
    // item by kr's rules, as in `kr run`.
    let other = !verdict.reasons.is_empty()
        || saved.reruns.iter().any(Option::is_none)
        || saved.reruns.len() < Verdict::to_rerun(&saved.first).len();
    let seen = decide::seen_before(events, run_id);
    unseen(&cases, other, &seen)
        .into_iter()
        .map(|case| {
            let of_run = |kind: EventKind, key: &str, value: &serde_json::Value| {
                events
                    .iter()
                    .find(|e| e.kind == kind && e.run_id == run_id && e.fields[key] == *value)
            };
            let raised = events.iter().find(|e| {
                e.kind == EventKind::DecisionPoint
                    && e.run_id == run_id
                    && e.fields["item"] == saved.item
                    && e.fields["test"] == case.id
            });
            let id = raised.and_then(|e| e.fields["id"].as_str().map(str::to_owned));
            let decided = id
                .as_ref()
                .and_then(|id| of_run(EventKind::Decision, "id", &serde_json::json!(id)));
            let reconciliation = id.as_ref().and_then(|id| {
                events
                    .iter()
                    .find(|e| e.kind == EventKind::Reconciliation && e.fields["id"] == *id)
                    .map(|e| {
                        let mut r = e.fields.clone();
                        r["recorded"] = serde_json::json!(true);
                        r
                    })
                    .or_else(|| {
                        let mut r = decide::reconciliation(events, decided?)?;
                        r["recorded"] = serde_json::json!(false);
                        Some(r)
                    })
            });
            Point {
                run_id: run_id.to_string(),
                item: saved.item.clone(),
                test: case.id.clone(),
                signature: case.signature.clone(),
                category: case.category,
                default: suggest(case.category).option,
                id,
                decision: decided.map(|e| {
                    serde_json::json!({
                        "choice": e.fields["choice"],
                        "reason": e.fields["reason"],
                        "via": e.fields["via"],
                    })
                }),
                reconciliation,
            }
        })
        .collect()
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
        crate::output::print_json(report)?;
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
        // Why the item passes or fails differently now.
        if c.change.before.passed && !c.change.after.passed {
            for reason in &c.change.after.reasons {
                println!("  now: {reason}");
            }
        } else if !c.change.before.passed && c.change.after.passed {
            for reason in &c.change.before.reasons {
                println!("  was: {reason}");
            }
        }
    }
    for p in &report.decision_points {
        println!(
            "DECISION POINT: {} {}: {}: unseen {} failure; kr suggests {}",
            p.run_id,
            p.item,
            p.test,
            p.category
                .and_then(|c| serde_json::to_value(c).ok())
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".to_owned()),
            p.default
        );
        match (&p.id, &p.decision) {
            (None, _) => println!(
                "  not raised when it ran (a kr without decision points): it was FAIL then"
            ),
            (Some(id), None) => println!("  {id}: raised, not decided yet"),
            (Some(id), Some(d)) => println!(
                "  {id}: decided {} via {}: {}",
                d["choice"].as_str().unwrap_or_default(),
                d["via"].as_str().unwrap_or_default(),
                d["reason"].as_str().unwrap_or_default()
            ),
        }
        if let Some(r) = &p.reconciliation {
            println!(
                "  reconciled{} by run {}: {} → {}",
                if r["recorded"] == true {
                    ""
                } else {
                    " (not recorded yet)"
                },
                r["later"]["run_id"].as_str().unwrap_or_default(),
                r["later"]["outcome"].as_str().unwrap_or_default(),
                if r["supported"] == true {
                    "supported"
                } else {
                    "not supported"
                }
            );
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
    let points = &report.decision_points;
    let decisions = if points.is_empty() {
        String::new()
    } else {
        let decided = points.iter().filter(|p| p.decision.is_some()).count();
        let reconciled: Vec<_> = points
            .iter()
            .filter_map(|p| p.reconciliation.as_ref())
            .collect();
        let supported = reconciled.iter().filter(|r| r["supported"] == true).count();
        format!(
            "; {} ({decided} decided, {} reconciled, {supported} supported)",
            plural(points.len(), "decision point", "decision points"),
            reconciled.len()
        )
    };
    println!(
        "rejudge: {} in {}{evidence}; {changed}{decisions}",
        plural(report.items, "item", "items"),
        plural(report.runs, "run", "runs"),
    );
    Ok(())
}
