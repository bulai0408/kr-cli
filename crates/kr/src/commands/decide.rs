//! Decision points of `kr run` / `kr gate` and `kr decide` (ADR 0011, ADR 0015).
//!
//! A run whose rules do not cover a failure runs to its end, then stops without a
//! verdict: it records its decision points and exits 4. The agent hands each decision
//! back with `kr decide <id> <option> --reason …` (or `--decide <id>=<option>` on the
//! next `kr run` / `kr gate`); once all are decided, kr gives the verdict from the
//! results it has, without running anything again. Later runs of the same test on the
//! same build inputs reconcile the decision: was the agent right?

use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Args;
use jiff::Timestamp;
use kr_events::{Event, EventKind, RunId};
use kr_run::decision::{Affects, Decision, FAILURE_KIND, FailureKind, Seen, suggest, unseen};
use kr_run::{Case, Final};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::run::{ItemResult, Summary, report};
use crate::output::BadArguments;
use crate::{Ctx, Loaded};

#[derive(Debug, Args)]
#[command(
    after_help = "Exit codes: 0 the run passed, 1 it failed, 4 other decision points of the run are still pending, 2 bad arguments (unknown ID or option, already decided)."
)]
pub struct DecideArgs {
    /// The decision point: its `id` in `decisions` of `kr run --json`, or the ID after
    /// DECIDE in the human output.
    id: String,

    /// The option to choose, one of the decision point's `options` (for a failure:
    /// product, flaky or environment).
    option: String,

    /// Why: what in the evidence made the choice. kr keeps it with the decision.
    #[arg(long)]
    reason: String,
}

/// How long the history of failures and decisions reaches back.
const HISTORY_DAYS: i64 = 90;
/// The decision points of a run and what it needs to give its verdict, in its run
/// directory.
const PENDING: &str = "decisions.json";

/// A run that stopped at decision points.
#[derive(Debug, Serialize, Deserialize)]
struct Pending {
    schema_version: u32,
    run_id: RunId,
    label: String,
    build: serde_json::Value,
    build_text: String,
    duration_ms: u64,
    /// The items as kr judged them, before any decision.
    items: Vec<ItemResult>,
    decisions: Vec<Decision>,
    /// What was chosen so far, by decision ID.
    #[serde(default)]
    made: BTreeMap<String, Made>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Made {
    choice: String,
    reason: String,
    via: String,
}

pub fn run(ctx: &Ctx, args: DecideArgs) -> Result<ExitCode> {
    decide(ctx, &[(args.id, args.option)], &args.reason, "kr decide")
}

/// `kr run --decide <id>=<option> …` / `kr gate --decide …`: the same as `kr decide`.
pub fn from_flags(ctx: &Ctx, flags: &[String], reason: &str, via: &str) -> Result<ExitCode> {
    let choices = flags
        .iter()
        .map(|f| {
            f.split_once('=')
                .map(|(id, option)| (id.to_owned(), option.to_owned()))
                .ok_or_else(|| {
                    BadArguments(format!(
                        "--decide takes <id>=<option>, e.g. --decide {f}=product; got `{f}`"
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    decide(ctx, &choices, reason, via)
}

fn decide(ctx: &Ctx, choices: &[(String, String)], reason: &str, via: &str) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let mut run_id = None;
    for (id, _) in choices {
        let of = run_of(id)?;
        if run_id.is_some_and(|r| r != of) {
            return Err(
                BadArguments("decide the decision points of one run at a time".to_owned()).into(),
            );
        }
        run_id = Some(of);
    }
    let run_id = run_id.context("nothing to decide")?;
    let file = loaded.run_dir(run_id).join(PENDING);
    let mut pending: Pending = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| {
            BadArguments(format!(
                "no decision points saved for run {run_id} ({} is missing); take the ID from `decisions` of the run's --json output",
                file.display()
            ))
        })?;
    for (id, option) in choices {
        let Some(decision) = pending.decisions.iter().find(|d| d.id == *id) else {
            let ids: Vec<&str> = pending.decisions.iter().map(|d| d.id.as_str()).collect();
            return Err(BadArguments(format!(
                "run {run_id} has no decision point `{id}`; its decision points: {}",
                ids.join(", ")
            ))
            .into());
        };
        if let Some(made) = pending
            .made
            .get(id)
            .filter(|m| !(m.via == "default" && decision.affects == Affects::Efficiency))
        {
            return Err(BadArguments(format!(
                "`{id}` is decided already: {} ({}); a decision is not changed afterwards",
                made.choice, made.reason
            ))
            .into());
        }
        if decision.option(option).is_none() {
            let names: Vec<&str> = decision.options.iter().map(|o| o.name.as_str()).collect();
            return Err(BadArguments(format!(
                "`{option}` is not an option of `{id}`; choose one of: {}",
                names.join(", ")
            ))
            .into());
        }
        let made = Made {
            choice: option.clone(),
            reason: reason.to_owned(),
            via: via.to_owned(),
        };
        decision_event(ctx, &loaded, run_id, decision, &made)?;
        pending.made.insert(id.clone(), made);
    }
    std::fs::write(&file, serde_json::to_vec(&pending)?)
        .with_context(|| format!("failed to write {}", file.display()))?;

    let items = verdicts(&pending);
    let left: Vec<&Decision> = pending
        .decisions
        .iter()
        .filter(|d| d.affects == Affects::Conclusion && !pending.made.contains_key(&d.id))
        .collect();
    let ok = left.is_empty() && items.iter().all(|r| !r.blocking());
    if left.is_empty() {
        loaded.events().append(&Event::now(
            run_id,
            loaded.project(),
            EventKind::RunDecided,
            json!({
                "label": pending.label,
                "ok": ok,
                "passed": items.iter().filter(|r| r.passed).count(),
                "total": items.len(),
                "decisions": pending.decisions.iter().map(|d| &d.id).collect::<Vec<_>>(),
            }),
        ))?;
    }
    let summary = Summary {
        label: &pending.label,
        build: &pending.build,
        build_text: &pending.build_text,
        elapsed: Duration::from_millis(pending.duration_ms),
    };
    report(ctx, &summary, &items, ok, None, &left)?;
    Ok(if !left.is_empty() {
        ExitCode::from(crate::output::DECISIONS_PENDING)
    } else if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The run of a decision ID `<run id>-<n>`.
fn run_of(id: &str) -> Result<RunId, BadArguments> {
    id.rsplit_once('-')
        .filter(|(_, n)| n.parse::<u32>().is_ok())
        .and_then(|(run, _)| run.parse().ok())
        .ok_or_else(|| {
            BadArguments(format!(
                "`{id}` is not a decision ID; IDs look like <run id>-<n>, see `decisions` in the run's --json output"
            ))
        })
}

/// Who decided: the agent and where it works.
fn by(ctx: &Ctx, loaded: &Loaded) -> serde_json::Value {
    json!({
        "agent": ctx.env_var("KR_REQUESTER").or_else(|| ctx.env_var("AI_AGENT")),
        "worktree": loaded.project_file.as_deref().and_then(|f| f.parent()),
        // SAFETY: getppid has no preconditions.
        "parent_pid": unsafe { libc::getppid() },
    })
}

fn decision_event(
    ctx: &Ctx,
    loaded: &Loaded,
    run_id: RunId,
    d: &Decision,
    made: &Made,
) -> Result<()> {
    let mut fields = json!({
        "id": d.id,
        "kind": d.kind,
        "item": d.item,
        "test": d.test,
        "choice": made.choice,
        "reason": made.reason,
        "via": made.via,
        "by": by(ctx, loaded),
        "evidence": d.evidence,
    });
    if made.via == "default" {
        fields["review"] = json!("pending");
    }
    loaded.events().append(&Event::now(
        run_id,
        loaded.project(),
        EventKind::Decision,
        fields,
    ))
}

/// The items with every decision made so far applied: a test judged flaky counts as
/// flaky, one judged an environment failure as unverified, one judged a product
/// failure stays failed.
fn verdicts(pending: &Pending) -> Vec<ItemResult> {
    let mut items = pending.items.clone();
    for d in &pending.decisions {
        let (Some(made), Some(test)) = (pending.made.get(&d.id), &d.test) else {
            continue;
        };
        let Some(r) = items.iter_mut().find(|r| r.name == d.item) else {
            continue;
        };
        if d.kind != FAILURE_KIND {
            continue;
        }
        let tests = r.tests.get_or_insert_with(BTreeMap::new);
        let mut moved = |to: &str| {
            if let Some(failed) = tests.get_mut("failed").filter(|n| **n > 0) {
                *failed -= 1;
                *tests.entry(to.to_owned()).or_default() += 1;
            }
        };
        match FailureKind::parse(&made.choice) {
            Some(FailureKind::Flaky) => {
                moved("flaky");
                r.flaky.push(format!(
                    "{test} (failed again when run on its own; the agent judged it flaky: {})",
                    made.reason
                ));
            }
            Some(FailureKind::Environment) => {
                moved("unverified");
                let why = format!(
                    "the agent judged {test} an environment failure: {}",
                    made.reason
                );
                r.summary = std::iter::once(why.clone())
                    .chain(r.summary.clone())
                    .take(3)
                    .collect();
                r.unverified_reason = Some(why);
            }
            Some(FailureKind::Product) | None => {}
        }
    }
    for r in items.iter_mut().filter(|r| !r.decisions.is_empty()) {
        let count = |k: &str| r.tests.as_ref().and_then(|t| t.get(k)).copied();
        if count("failed").is_some_and(|n| n == r.quarantined.len()) && r.unverified_count() == 0 {
            r.passed = count("failed") == Some(0);
            r.blocking = Some(false);
            if r.passed {
                r.summary.clear();
            }
        }
    }
    items
}

/// Events of the past [`HISTORY_DAYS`]: where kr learns which failures it has seen and
/// finds the decisions to reconcile.
pub(crate) fn history(loaded: &Loaded) -> Vec<Event> {
    let since = Timestamp::now() - jiff::SignedDuration::from_hours(24 * HISTORY_DAYS);
    loaded.events().read_since(since).unwrap_or_default()
}

/// The tests each `item_end` of `events` kept.
fn item_cases(e: &Event) -> Vec<Case> {
    serde_json::from_value(e.fields["cases"].clone()).unwrap_or_default()
}

/// Failures of runs before `run_id` (run IDs sort by start).
pub(crate) fn seen_before(events: &[Event], run_id: RunId) -> Seen {
    let mut seen = Seen::default();
    for e in events
        .iter()
        .filter(|e| e.kind == EventKind::ItemEnd && e.run_id < run_id)
    {
        seen.learn(&item_cases(e));
    }
    seen
}

/// The earlier outcomes of a test: in how many items it ran, failed, was flaky.
fn test_history(events: &[Event], run_id: RunId, test: &str) -> serde_json::Value {
    let (mut runs, mut failed, mut flaky) = (0, 0, 0);
    for e in events
        .iter()
        .filter(|e| e.kind == EventKind::ItemEnd && e.run_id < run_id)
    {
        if let Some(c) = item_cases(e).into_iter().find(|c| c.id == test) {
            runs += 1;
            failed += usize::from(c.outcome == Final::Failed);
            flaky += usize::from(c.outcome == Final::Flaky);
        }
    }
    json!({ "runs": runs, "failed": failed, "flaky": flaky, "days": HISTORY_DAYS })
}

/// The decision points of a finished run: each failed test kr has not seen, of an item
/// that only failed tests fail. Adds their IDs to the items.
pub(crate) fn raise(events: &[Event], run_id: RunId, results: &mut [ItemResult]) -> Vec<Decision> {
    let seen = seen_before(events, run_id);
    let mut decisions = Vec::new();
    for r in results
        .iter_mut()
        .filter(|r| r.only_test_failures && r.blocking())
    {
        let cases: Vec<Case> = r
            .cases
            .iter()
            .filter(|c| !r.quarantined.contains(&c.id))
            .cloned()
            .collect();
        for case in unseen(&cases, false, &seen) {
            let d = failure_kind(
                format!("{run_id}-{}", decisions.len() + 1),
                &r.name,
                case,
                json!({
                    "summary": r.summary,
                    "xcresult": r.xcresult,
                    "log": r.log,
                    "load": r.load,
                    "high_load": r.high_load,
                    "history": test_history(events, run_id, &case.id),
                }),
            );
            r.decisions.push(d.id.clone());
            decisions.push(d);
        }
    }
    decisions
}

/// The decision point for one unseen failure; `more` adds to its evidence.
pub(crate) fn failure_kind(
    id: String,
    item: &str,
    case: &Case,
    mut more: serde_json::Value,
) -> Decision {
    more["signature"] = json!(case.signature);
    more["category"] = json!(case.category);
    Decision {
        id,
        kind: FAILURE_KIND.to_owned(),
        affects: Affects::Conclusion,
        question: format!(
            "What kind of failure is {} in {item}? kr has not seen it before, and it failed again when run on its own.",
            case.id
        ),
        item: item.to_owned(),
        test: Some(case.id.clone()),
        options: FailureKind::options(),
        evidence: more,
        default: suggest(case.category),
    }
}

/// Records a finished run's decision points. Those that decide the verdict are saved
/// for `kr decide`; those that only affect efficiency are taken with kr's default and
/// recorded as pending review.
pub(crate) fn record(
    ctx: &Ctx,
    loaded: &Loaded,
    run_id: RunId,
    summary: &Summary<'_>,
    results: &[ItemResult],
    decisions: &[Decision],
) -> Result<()> {
    if decisions.is_empty() {
        return Ok(());
    }
    let mut made = BTreeMap::new();
    for d in decisions {
        loaded.events().append(&Event::now(
            run_id,
            loaded.project(),
            EventKind::DecisionPoint,
            serde_json::to_value(d)?,
        ))?;
        if d.affects == Affects::Efficiency {
            let m = Made {
                choice: d.default.option.clone(),
                reason: d.default.reason.clone(),
                via: "default".to_owned(),
            };
            decision_event(ctx, loaded, run_id, d, &m)?;
            made.insert(d.id.clone(), m);
        }
    }
    let pending = Pending {
        schema_version: crate::output::SCHEMA_VERSION,
        run_id,
        label: summary.label.to_owned(),
        build: summary.build.clone(),
        build_text: summary.build_text.to_owned(),
        duration_ms: summary.elapsed.as_millis() as u64,
        items: results.to_vec(),
        decisions: decisions.to_vec(),
        made,
    };
    let dir = loaded.run_dir(run_id);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(PENDING);
    std::fs::write(&file, serde_json::to_vec(&pending)?)
        .with_context(|| format!("failed to write {}", file.display()))
}

/// What a later run says about a recorded failure decision: the first `item_end` of
/// the same item on the same build inputs after the decided run that ran the test.
/// `None` while there is none, or when the decided run's inputs are unknown.
pub(crate) fn reconciliation(events: &[Event], decision: &Event) -> Option<serde_json::Value> {
    let f = &decision.fields;
    let (item, test) = (f["item"].as_str()?, f["test"].as_str()?);
    let choice = FailureKind::parse(f["choice"].as_str()?)?;
    let hash_of = |e: &Event| {
        e.fields["identity"]["input_hash"]
            .as_str()
            .map(str::to_owned)
    };
    let ends = || {
        events
            .iter()
            .filter(move |e| e.kind == EventKind::ItemEnd && e.fields["item"] == item)
    };
    let hash = ends()
        .find(|e| e.run_id == decision.run_id)
        .and_then(hash_of)?;
    let (later, outcome) = ends()
        .filter(|e| e.run_id > decision.run_id && hash_of(e).as_ref() == Some(&hash))
        .filter_map(|e| {
            Some((
                e.run_id,
                item_cases(e).into_iter().find(|c| c.id == test)?.outcome,
            ))
        })
        .min_by_key(|(run, _)| *run)?;
    let supported = choice.supported_by(outcome)?;
    Some(json!({
        "id": f["id"],
        "decided_run": decision.run_id,
        "kind": f["kind"],
        "item": item,
        "test": test,
        "choice": choice.name(),
        "later": { "run_id": later, "outcome": outcome },
        "input_hash": hash,
        "supported": supported,
        "rule": "a later run of the test on the same build inputs: product is supported if it fails again, flaky and environment if it passes",
    }))
}

/// Reconciles the failure decisions that later runs (this one included) can now
/// answer, once each.
pub(crate) fn reconcile(loaded: &Loaded, run_id: RunId, events: &[Event]) -> Result<()> {
    let done: BTreeSet<&str> = events
        .iter()
        .filter(|e| e.kind == EventKind::Reconciliation)
        .filter_map(|e| e.fields["id"].as_str())
        .collect();
    for d in events.iter().filter(|e| {
        e.kind == EventKind::Decision
            && e.fields["kind"] == FAILURE_KIND
            && e.fields["id"].as_str().is_some_and(|id| !done.contains(id))
    }) {
        if let Some(fields) = reconciliation(events, d) {
            loaded.events().append(&Event::now(
                run_id,
                loaded.project(),
                EventKind::Reconciliation,
                fields,
            ))?;
        }
    }
    Ok(())
}

/// The human section of the decision points: what to decide and the next command.
pub(crate) fn print_pending(pending: &[&Decision]) {
    println!(
        "{} to hand back before the verdict (how to judge: `kr skill --reference decisions`):",
        match pending.len() {
            1 => "1 decision point".to_owned(),
            n => format!("{n} decision points"),
        }
    );
    for d in pending {
        println!("DECIDE {}: {}", d.id, d.question);
        let e = &d.evidence;
        let mut evidence = Vec::new();
        if let Some(c) = e["category"].as_str() {
            evidence.push(c.to_owned());
        }
        if let Some(s) = e["signature"].as_str() {
            evidence.push(format!("\"{s}\""));
        }
        let h = &e["history"];
        if let Some(runs) = h["runs"].as_u64() {
            evidence.push(format!(
                "earlier: {} failed, {} flaky in {runs} runs",
                h["failed"], h["flaky"]
            ));
        }
        if let Some(load) = e["high_load"].as_str() {
            evidence.push(format!("high load: {load}"));
        }
        println!("  evidence: {}", evidence.join(" · "));
        for o in &d.options {
            println!("  {}: {}", o.name, o.means);
        }
        println!("  kr suggests {}: {}", d.default.option, d.default.reason);
        println!(
            "  next: kr decide {} <{}> --reason \"…\"",
            d.id,
            d.options
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>()
                .join("|")
        );
    }
}
