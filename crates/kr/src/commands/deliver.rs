//! `kr deliver` (spec phase 3 §8): the three-part delivery report draft of one run —
//! decided by machine, needs human judgment of feel, not verified — from the run's
//! events and logs. To-dos that the run or the changed files show are listed for the
//! author; the draft decides nothing (ADR 0011).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::Args;
use jiff::Timestamp;
use kr_events::{Event, EventKind, RunId};
use kr_exec::Command;
use serde::Serialize;

use crate::Ctx;

#[derive(Debug, Args)]
pub struct DeliverArgs {
    /// Run ID, or a unique start of one. Default: the latest run with results (of this
    /// project, inside a project).
    run: Option<String>,

    /// Branch or commit the work started from. Files changed since then add to-dos:
    /// schema changes are pending deploy, Artboards need a feel check, legal text needs
    /// owner review.
    #[arg(long)]
    base: Option<String>,
}

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
struct Draft {
    schema_version: u32,
    run_id: RunId,
    project: Option<String>,
    started: Timestamp,
    machine: Machine,
    /// Needs human judgment of feel.
    human: Vec<Todo>,
    /// Not verified, or waiting for something outside the run.
    unverified: Vec<Todo>,
}

#[derive(Debug, Serialize)]
struct Machine {
    passed: usize,
    total: usize,
    items: Vec<Item>,
}

#[derive(Debug, Serialize)]
struct Item {
    name: String,
    kind: String,
    /// "pass", "fail" or "unverified".
    result: &'static str,
    device: Option<String>,
    tests: Option<BTreeMap<String, u64>>,
    flaky: Vec<String>,
    /// Tests that failed their rerun.
    failed_tests: Vec<String>,
    high_load: Option<String>,
    /// Set when the whole item ran again (high load).
    run_again: Option<String>,
    xcresults: Vec<Bundle>,
}

#[derive(Debug, Serialize)]
struct Bundle {
    path: PathBuf,
    /// From the second round of the item (`<item>-again`).
    again: bool,
    exists: bool,
}

#[derive(Debug, Serialize)]
struct Todo {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    item: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<String>,
    note: String,
}

impl Todo {
    fn item(kind: &'static str, item: &str, note: String) -> Self {
        Self {
            kind,
            item: Some(item.to_owned()),
            files: Vec::new(),
            note,
        }
    }

    fn files(kind: &'static str, files: Vec<String>, note: &str) -> Self {
        Self {
            kind,
            item: None,
            files,
            note: note.to_owned(),
        }
    }
}

pub fn run(ctx: &Ctx, args: DeliverArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let log = loaded.events();
    let events = log.read_since(Timestamp::UNIX_EPOCH)?;
    let mut runs: BTreeMap<RunId, Vec<Event>> = BTreeMap::new();
    for e in events {
        runs.entry(e.run_id).or_default().push(e);
    }
    let run_id = match &args.run {
        Some(want) => {
            let found: Vec<RunId> = runs
                .keys()
                .filter(|id| id.to_string().starts_with(want.as_str()))
                .copied()
                .collect();
            match found.as_slice() {
                [id] => *id,
                [] => bail!(
                    "no run `{want}` in the events under {}; run `kr deliver` without an ID for the latest run",
                    log.dir().display()
                ),
                _ => bail!(
                    "`{want}` matches {} runs ({}); give more of the ID",
                    found.len(),
                    found
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }
        }
        None => {
            let project = loaded.project();
            let latest = runs.iter().rev().find(|(_, events)| {
                events.iter().any(|e| {
                    e.kind == EventKind::ItemEnd
                        && (project.is_none() || e.project.as_deref() == project)
                })
            });
            match latest {
                Some((id, _)) => *id,
                None => bail!(
                    "no run with scenario or suite results in the events under {}; run `kr run` or `kr gate` first",
                    log.dir().display()
                ),
            }
        }
    };

    let mut draft = draft(run_id, &runs[&run_id], &loaded.run_dir(run_id));
    match &args.base {
        Some(base) => changed_files_todos(ctx, base, &mut draft)?,
        None => draft.unverified.push(Todo {
            kind: "changes_not_checked",
            item: None,
            files: Vec::new(),
            note: "changed files were not checked for to-dos (schema, Artboards, legal text); run again with --base <branch the work started from>".to_owned(),
        }),
    }

    if ctx.json {
        crate::output::print_json(&draft)?;
    } else {
        print!("{}", markdown(&draft));
    }
    Ok(ExitCode::SUCCESS)
}

/// The draft from the events of one run. Items are in the order they ended.
fn draft(run_id: RunId, events: &[Event], run_dir: &Path) -> Draft {
    let mut items: Vec<Item> = Vec::new();
    let mut started: Vec<(String, Option<String>)> = Vec::new();
    let mut failed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut again: BTreeMap<String, String> = BTreeMap::new();
    let mut unverified = Vec::new();
    let mut later = Vec::new();
    for e in events {
        let f = &e.fields;
        let item = f["item"].as_str().unwrap_or_default();
        match e.kind {
            EventKind::ItemStart => {
                started.push((item.to_owned(), f["device"].as_str().map(str::to_owned)));
            }
            EventKind::Rerun => {
                if let Some(test) = f["test"].as_str() {
                    let list = failed.entry(item.to_owned()).or_default();
                    if f["passed"] == false && !list.iter().any(|t| t == test) {
                        list.push(test.to_owned());
                    }
                } else if let Some(reason) = f["reason"].as_str() {
                    again.insert(item.to_owned(), reason.to_owned());
                }
            }
            EventKind::FilterMismatch => unverified.push(Todo::item(
                "filter_mismatch",
                item,
                format!(
                    "filter `{}` selects no test on the device",
                    f["filter"].as_str().unwrap_or_default()
                ),
            )),
            EventKind::ItemEnd => {
                let tests: Option<BTreeMap<String, u64>> =
                    serde_json::from_value(f["tests"].clone()).ok();
                let not_verified = tests
                    .as_ref()
                    .and_then(|t| t.get("unverified"))
                    .copied()
                    .unwrap_or(0);
                let result = if not_verified > 0 {
                    let reason = f["unverified_reason"]
                        .as_str()
                        .unwrap_or("no reason recorded");
                    later.push(Todo::item(
                        "unverified",
                        item,
                        format!("{not_verified} tests unverified: {reason}"),
                    ));
                    "unverified"
                } else if f["passed"] == true {
                    "pass"
                } else {
                    "fail"
                };
                if tests.is_none() && f["passed"] == true {
                    later.push(Todo::item(
                        "exit_code_only",
                        item,
                        "only the exit code was checked; the test count was not verified"
                            .to_owned(),
                    ));
                }
                items.push(Item {
                    name: item.to_owned(),
                    kind: f["kind"].as_str().unwrap_or_default().to_owned(),
                    result,
                    device: f["device"].as_str().map(str::to_owned),
                    tests,
                    flaky: serde_json::from_value(f["flaky"].clone()).unwrap_or_default(),
                    failed_tests: Vec::new(),
                    high_load: f["high_load"].as_str().map(str::to_owned),
                    run_again: None,
                    xcresults: bundles(run_dir, item),
                });
            }
            _ => {}
        }
    }
    for item in &mut items {
        if item.result != "pass" {
            item.failed_tests = failed.remove(&item.name).unwrap_or_default();
        }
        item.run_again = again.remove(&item.name);
    }
    for (name, device) in started {
        if !items.iter().any(|i| i.name == name) {
            let on = device.map(|d| format!(" on {d}")).unwrap_or_default();
            unverified.push(Todo::item(
                "no_result",
                &name,
                format!("started{on} but has no result; the run stopped before it ended"),
            ));
        }
    }
    unverified.extend(later);
    Draft {
        schema_version: SCHEMA_VERSION,
        run_id,
        project: events.iter().find_map(|e| e.project.clone()),
        started: events.first().map_or(Timestamp::UNIX_EPOCH, |e| e.ts),
        machine: Machine {
            passed: items.iter().filter(|i| i.result == "pass").count(),
            total: items.len(),
            items,
        },
        human: Vec::new(),
        unverified,
    }
}

/// Result bundles of an item, read from xcodebuild's own line in the run log.
fn bundles(run_dir: &Path, item: &str) -> Vec<Bundle> {
    [(item.to_owned(), false), (format!("{item}-again"), true)]
        .into_iter()
        .filter_map(|(dir, again)| {
            let log = std::fs::read_to_string(run_dir.join(dir).join("run.log")).ok()?;
            let mut lines = log.lines();
            lines.find(|l| l.trim() == "Writing result bundle at path:")?;
            let path = PathBuf::from(lines.next()?.trim());
            Some(Bundle {
                exists: path.exists(),
                path,
                again,
            })
        })
        .collect()
}

/// To-dos from the files changed since `base`, including uncommitted changes.
fn changed_files_todos(ctx: &Ctx, base: &str, draft: &mut Draft) -> Result<()> {
    let runner = ctx.runner()?;
    let out = runner.run(
        &Command::new("git")
            .args(["diff", "--name-only", "--merge-base", base])
            .current_dir(&ctx.cwd),
    )?;
    if !out.success() {
        let why = out.stderr.lines().next().unwrap_or("git diff failed");
        draft.unverified.push(Todo {
            kind: "changes_not_checked",
            item: None,
            files: Vec::new(),
            note: format!(
                "changed files were not checked: `git diff --merge-base {base}` failed ({why}); run in the repo of the run, with a --base that exists"
            ),
        });
        return Ok(());
    }
    let files: Vec<&str> = out.stdout.lines().filter(|l| !l.is_empty()).collect();
    let pick = |rule: fn(&str) -> bool| -> Vec<String> {
        files
            .iter()
            .filter(|f| rule(f))
            .map(|f| (*f).to_owned())
            .collect()
    };
    let artboards = pick(is_artboard);
    if !artboards.is_empty() {
        draft.human.push(Todo::files(
            "artboard",
            artboards,
            "compare the app with the changed Artboard; attach a screenshot or screen recording",
        ));
    }
    let mut todos = Vec::new();
    let schema = pick(is_schema);
    if !schema.is_empty() {
        todos.push(Todo::files(
            "pending_deploy",
            schema,
            "schema changed; deploy it, or say who deploys it and when",
        ));
    }
    let legal = pick(is_legal);
    if !legal.is_empty() {
        todos.push(Todo::files(
            "owner_review",
            legal,
            "legal text changed; the owner must review it",
        ));
    }
    // Before the run's own to-dos: these block the delivery itself.
    draft.unverified.splice(0..0, todos);
    Ok(())
}

fn components(path: &str) -> impl Iterator<Item = String> + '_ {
    path.split('/').map(str::to_lowercase)
}

fn is_schema(path: &str) -> bool {
    path.to_lowercase().ends_with(".sql")
        || components(path).any(|c| c.contains("migration") || c.contains("schema"))
}

fn is_artboard(path: &str) -> bool {
    path.ends_with(".html") && components(path).any(|c| c == "artboards")
}

fn is_legal(path: &str) -> bool {
    components(path).any(|c| c == "legal" || c.contains("privacy"))
}

fn markdown(d: &Draft) -> String {
    let mut s = String::new();
    let mut line = |l: String| {
        s.push_str(&l);
        s.push('\n');
    };
    line("# Delivery report draft".to_owned());
    line(format!(
        "run {} · {} · started {}",
        d.run_id,
        d.project.as_deref().unwrap_or("no project"),
        d.started.strftime("%Y-%m-%d %H:%M UTC")
    ));
    line(String::new());
    line(format!(
        "## 1. Decided by machine: {}/{} PASS",
        d.machine.passed, d.machine.total
    ));
    for i in &d.machine.items {
        let mut detail: Vec<String> = Vec::new();
        match &i.tests {
            Some(t) => {
                let total: u64 = t.values().sum();
                let mut parts = vec![format!("{total} tests")];
                for k in ["failed", "flaky", "skipped", "unverified"] {
                    if let Some(n) = t.get(k).filter(|n| **n > 0) {
                        parts.push(format!("{n} {k}"));
                    }
                }
                detail.push(parts.join(", "));
            }
            None => detail.push(format!("{}, exit code only", i.kind)),
        }
        if let Some(d) = &i.device {
            detail.push(d.clone());
        }
        line(format!(
            "- {} {} ({})",
            i.result.to_uppercase(),
            i.name,
            detail.join(", ")
        ));
        for t in i.failed_tests.iter().take(3) {
            line(format!("  failed: {t}"));
        }
        if i.failed_tests.len() > 3 {
            line(format!("  and {} more failed", i.failed_tests.len() - 3));
        }
        for f in &i.flaky {
            line(format!("  flaky: {f}"));
        }
        if let Some(r) = &i.run_again {
            line(format!("  whole item ran again: {r}"));
        } else if let Some(h) = &i.high_load {
            line(format!("  high load: {h}"));
        }
        for b in &i.xcresults {
            line(format!(
                "  xcresult{}: {}{}",
                if b.again { " (again)" } else { "" },
                b.path.display(),
                if b.exists { "" } else { " (not found)" }
            ));
        }
    }
    line(String::new());
    line(format!(
        "## 2. Needs human judgment of feel: {}",
        d.human.len()
    ));
    for t in &d.human {
        line(todo_line(t));
    }
    if d.human.is_empty() {
        line("- None found. List each animation, visual or touch response a human must judge, with a screenshot or screen recording.".to_owned());
    }
    line(String::new());
    line(format!("## 3. Not verified: {}", d.unverified.len()));
    for t in &d.unverified {
        line(todo_line(t));
    }
    s
}

fn todo_line(t: &Todo) -> String {
    let title = match t.kind {
        "artboard" => "Feel check",
        "pending_deploy" => "Pending deploy",
        "owner_review" => "Owner review",
        _ => "",
    };
    match (&t.item, title) {
        (Some(item), _) => format!("- {item}: {}", t.note),
        (None, "") => format!("- {}", t.note),
        (None, title) => format!("- {title}: {} ({})", t.files.join(", "), t.note),
    }
}
