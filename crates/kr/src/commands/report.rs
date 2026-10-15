//! `kr report` (spec phase 3 §7, ADR 0016): the report of a batch — the runs of a
//! project in a time range — from the event log and the invocation log: queue and run
//! time, rework rounds, flaky tests, the commands the agent called, the bad arguments
//! and detours, and how often its decisions held. Data only: it changes nothing, and
//! whether to trust the agent with more is for the owner to set in the host (ADR 0012).

use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Result;
use clap::Args;
use jiff::{SignedDuration, Timestamp};
use kr_events::{Event, EventKind, RunId};
use serde::Serialize;
use serde_json::Value;

use super::human_duration;
use crate::Ctx;
use crate::output::BadArguments;

#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Start of the batch: an RFC 3339 time (2026-10-14T09:40:00Z) or how long ago
    /// (90m, 24h, 7d). Runs belong to the batch they started in. Default: 24h.
    #[arg(long)]
    since: Option<String>,

    /// End of the batch, in the same forms. Default: now.
    #[arg(long)]
    until: Option<String>,

    /// The project to report on. Default: the project here, else all projects.
    #[arg(long)]
    project: Option<String>,

    /// Keep only the runs and calls from this worktree (repo root; repeatable). Runs
    /// without a requester (before #29, `kr check`, `kr lock`) are left out.
    #[arg(long = "worktree", value_name = "PATH")]
    worktrees: Vec<String>,

    /// Keep only the runs and calls of this agent (`KR_REQUESTER`, else `AI_AGENT`;
    /// repeatable). Runs without a requester are left out.
    #[arg(long = "agent", value_name = "NAME")]
    agents: Vec<String>,
}

/// The kinds of detour, in the order they are listed (ADR 0016 §3).
const DETOURS: [&str; 9] = [
    "bad_arguments",
    "kr_error",
    "aborted",
    "build_failed_in_run",
    "no_result",
    "filter_mismatch",
    "products_missing",
    "repeat_after_pass",
    "undecided",
];

#[derive(Debug, Serialize)]
struct Report {
    batch: Batch,
    time: Time,
    runs: Vec<Run>,
    rework: Rework,
    flaky: Vec<Flaky>,
    usage: Usage,
    accuracy: Vec<Accuracy>,
}

#[derive(Debug, Serialize)]
struct Batch {
    since: Timestamp,
    until: Timestamp,
    project: Option<String>,
    worktrees: Vec<String>,
    agents: Vec<String>,
    /// Runs in the range with no `run_start`, so no requester to filter on.
    runs_without_requester: usize,
}

#[derive(Debug, Serialize)]
struct Time {
    runs: usize,
    wall_ms: u64,
    queued_ms: u64,
    running_ms: u64,
    /// queued_ms ÷ wall_ms, 0 without runs.
    queue_share: f64,
    by_command: Vec<CommandTime>,
}

#[derive(Debug, Serialize)]
struct CommandTime {
    command: &'static str,
    runs: usize,
    /// Runs that failed, ended with a kr error, stopped, or left items without results.
    failed: usize,
    wall_ms: u64,
    queued_ms: u64,
}

#[derive(Debug, Serialize)]
struct Run {
    run_id: RunId,
    /// "check", "run", "gate", "lock" (`kr lock`, `kr pool lease`) or "other".
    command: &'static str,
    label: Option<String>,
    started: Timestamp,
    wall_ms: u64,
    queued_ms: u64,
    /// Scenarios and suites that ended.
    items: usize,
    /// "pass", "fail", "error", "aborted", "no_result"; null if the events do not say.
    result: Option<&'static str>,
    worktree: Option<String>,
    agent: Option<String>,
}

#[derive(Debug, Serialize)]
struct Rework {
    rounds: usize,
    items: Vec<ReworkItem>,
}

#[derive(Debug, Serialize)]
struct ReworkItem {
    item: String,
    /// Null when the runs have no requester: then the item is counted per project.
    worktree: Option<String>,
    runs: usize,
    failed: usize,
    /// Runs after a failed or unverified run of the item.
    rounds: usize,
    /// "pass", "fail" or "unverified".
    last: &'static str,
}

#[derive(Debug, Serialize)]
struct Flaky {
    /// The test, or `<item> (whole item)` when the whole item passed when run again.
    test: String,
    item: String,
    flaky: usize,
    /// How many times the item ran in the batch.
    item_runs: usize,
}

#[derive(Debug, Serialize)]
struct Usage {
    /// "invocations" (the invocation log) or "events" (inferred from the runs, without
    /// the commands that write no events and without bad arguments).
    source: &'static str,
    commands: Vec<CommandUse>,
    bad_arguments: Vec<Call>,
    detours: Vec<Detour>,
    wrong_decisions: Vec<WrongDecision>,
}

#[derive(Debug, Serialize)]
struct CommandUse {
    command: String,
    calls: usize,
    /// Null when the source is events.
    bad_arguments: Option<usize>,
    /// Calls that ended with a kr error (exit 1 from an error, not a failed test).
    errors: usize,
}

#[derive(Debug, Serialize)]
struct Call {
    ts: Timestamp,
    command: Option<String>,
    args: Vec<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct Detour {
    kind: &'static str,
    count: usize,
    examples: Vec<Example>,
}

#[derive(Debug, Serialize)]
struct Example {
    ts: Timestamp,
    run_id: Option<RunId>,
    detail: String,
}

#[derive(Debug, Serialize)]
struct WrongDecision {
    id: String,
    kind: String,
    item: Option<String>,
    test: Option<String>,
    choice: String,
    later_outcome: Option<String>,
    decided_run: RunId,
    later_run: Option<String>,
}

#[derive(Debug, Serialize)]
struct Accuracy {
    kind: String,
    #[serde(flatten)]
    score: Score,
    /// Efficiency decisions kr took by default, waiting for review.
    default_pending_review: usize,
    choices: Vec<ChoiceScore>,
}

#[derive(Debug, Default, Serialize)]
struct Score {
    decided: usize,
    reconciled: usize,
    supported: usize,
    /// supported ÷ reconciled; null before any reconciliation.
    rate: Option<f64>,
    /// Decided, not reconciled yet: no later run of the test on the same inputs.
    pending: usize,
}

#[derive(Debug, Serialize)]
struct ChoiceScore {
    choice: String,
    #[serde(flatten)]
    score: Score,
}

pub fn run(ctx: &Ctx, args: ReportArgs) -> Result<ExitCode> {
    let now = Timestamp::now();
    let until = match &args.until {
        Some(s) => parse_time(s, now, "--until")?,
        None => now,
    };
    let since = match &args.since {
        Some(s) => parse_time(s, now, "--since")?,
        None => until - SignedDuration::from_hours(24),
    };
    let loaded = ctx.load()?;
    let project = args
        .project
        .clone()
        .or_else(|| loaded.project().map(str::to_owned));
    let events = loaded.events().read_since(Timestamp::UNIX_EPOCH)?;
    let calls = crate::invocations::log(&loaded.state_dir).read_since(since)?;
    let filter = Filter {
        since,
        until,
        project: project.as_deref(),
        worktrees: &args.worktrees,
        agents: &args.agents,
    };
    let report = report(&filter, &events, &calls);
    if ctx.json {
        crate::output::print_json(&report)?;
    } else {
        print!("{}", human(&report));
    }
    Ok(ExitCode::SUCCESS)
}

/// An RFC 3339 time, or a duration back from now: `90m`, `24h`, `7d` (also `s`).
fn parse_time(s: &str, now: Timestamp, flag: &str) -> Result<Timestamp> {
    if let Ok(t) = s.parse::<Timestamp>() {
        return Ok(t);
    }
    let unit = s.chars().last().unwrap_or(' ');
    let secs = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => 0,
    };
    match s[..s.len() - unit.len_utf8().min(s.len())].parse::<i64>() {
        Ok(n) if secs > 0 && n >= 0 => Ok(now - SignedDuration::from_secs(n * secs)),
        _ => Err(BadArguments(format!(
            "{flag} `{s}` is neither a time nor a duration; use an RFC 3339 time such as 2026-10-14T09:40:00Z, or a duration back from now such as 90m, 24h or 7d"
        ))
        .into()),
    }
}

struct Filter<'a> {
    since: Timestamp,
    until: Timestamp,
    project: Option<&'a str>,
    worktrees: &'a [String],
    agents: &'a [String],
}

impl Filter<'_> {
    fn by_requester(&self) -> bool {
        !self.worktrees.is_empty() || !self.agents.is_empty()
    }

    /// Whether a requester (`{agent, worktree}`) passes `--worktree` and `--agent`.
    fn requester(&self, r: &Value) -> bool {
        let matches = |list: &[String], v: &Value| {
            list.is_empty() || v.as_str().is_some_and(|v| list.iter().any(|l| l == v))
        };
        matches(self.worktrees, &r["worktree"]) && matches(self.agents, &r["agent"])
    }

    fn project(&self, p: Option<&str>) -> bool {
        self.project.is_none() || p == self.project
    }
}

/// The events of one run, in the order written.
struct RunEvents<'a> {
    id: RunId,
    events: Vec<&'a Event>,
}

impl RunEvents<'_> {
    fn of(&self, kind: EventKind) -> impl Iterator<Item = &Event> {
        self.events.iter().copied().filter(move |e| e.kind == kind)
    }

    fn first(&self, kind: EventKind) -> Option<&Value> {
        self.of(kind).next().map(|e| &e.fields)
    }

    fn requester(&self) -> Option<&Value> {
        self.first(EventKind::RunStart).map(|f| &f["requester"])
    }

    fn command(&self) -> &'static str {
        if let Some(label) = self
            .first(EventKind::RunStart)
            .or_else(|| self.first(EventKind::RunEnd))
            .and_then(|f| f["label"].as_str())
        {
            return if label.starts_with("gate") {
                "gate"
            } else {
                "run"
            };
        }
        if let Some(command) = self.first(EventKind::StepEnd).map(|f| &f["command"]) {
            return if command == "check" { "check" } else { "run" };
        }
        if let Some(task) = self
            .first(EventKind::QueueStart)
            .and_then(|f| f["task"].as_str())
        {
            for c in ["check", "run", "gate"] {
                if task.strip_prefix(c).is_some_and(|r| r.starts_with(' ')) {
                    return c;
                }
            }
            return "lock";
        }
        if self.first(EventKind::ItemStart).is_some() || self.first(EventKind::ItemEnd).is_some() {
            return "run";
        }
        "other"
    }

    fn label(&self) -> Option<String> {
        self.first(EventKind::RunStart)
            .or_else(|| self.first(EventKind::RunEnd))
            .and_then(|f| f["label"].as_str())
            .or_else(|| {
                self.first(EventKind::QueueStart)
                    .and_then(|f| f["task"].as_str())
            })
            .map(str::to_owned)
    }

    /// Items that started and never ended.
    fn unfinished(&self) -> Vec<&str> {
        let ended: BTreeSet<&str> = self
            .of(EventKind::ItemEnd)
            .filter_map(|e| e.fields["item"].as_str())
            .collect();
        let mut started: Vec<&str> = self
            .of(EventKind::ItemStart)
            .filter_map(|e| e.fields["item"].as_str())
            .filter(|i| !ended.contains(i))
            .collect();
        started.dedup();
        started
    }

    fn result(&self, command: &str) -> Option<&'static str> {
        if self.first(EventKind::RunAborted).is_some() {
            return Some("aborted");
        }
        let end = self.first(EventKind::RunEnd);
        if end.is_some_and(|f| f.get("error").is_some_and(|e| !e.is_null())) {
            return Some("error");
        }
        match command {
            "check" => {
                let mut steps = self.of(EventKind::StepEnd).peekable();
                steps.peek()?;
                Some(if steps.all(|e| e.fields["ok"] == true) {
                    "pass"
                } else {
                    "fail"
                })
            }
            "lock" => self
                .first(EventKind::SlotReleased)
                .and_then(|f| f["exit_code"].as_i64())
                .map(|c| if c == 0 { "pass" } else { "fail" }),
            _ => {
                if let Some(end) = end {
                    return Some(if end["ok"] == true { "pass" } else { "fail" });
                }
                if !self.unfinished().is_empty() {
                    return Some("no_result");
                }
                let mut items = self.of(EventKind::ItemEnd).peekable();
                items.peek()?;
                Some(if items.all(|e| e.fields["passed"] == true) {
                    "pass"
                } else {
                    "fail"
                })
            }
        }
    }

    /// When the run's own steps happened: decisions are written with the run ID of the
    /// run they decide, possibly hours later, so they do not count.
    fn steps(&self) -> Vec<&Event> {
        let later = |e: &&Event| {
            matches!(
                e.kind,
                EventKind::Decision | EventKind::RunDecided | EventKind::Reconciliation
            )
        };
        let own: Vec<&Event> = self.events.iter().copied().filter(|e| !later(e)).collect();
        if own.is_empty() {
            self.events.clone()
        } else {
            own
        }
    }

    fn started(&self) -> Timestamp {
        self.steps()
            .iter()
            .map(|e| e.ts)
            .min()
            .expect("a run has events")
    }

    /// Wall time from the first step (or the start of the first wait) to the last, and
    /// the union of the waits for a slot, the old machine-wide lock or the load.
    fn times(&self) -> (u64, u64) {
        let mut waits: Vec<(Timestamp, Timestamp)> = self
            .steps()
            .into_iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    EventKind::SlotAcquired | EventKind::MachineLock | EventKind::LoadOk
                )
            })
            .filter_map(|e| {
                let ms = e.fields["waited_ms"].as_u64()?;
                Some((e.ts - SignedDuration::from_millis(ms as i64), e.ts))
            })
            .collect();
        waits.sort();
        let mut queued = SignedDuration::ZERO;
        let mut current: Option<(Timestamp, Timestamp)> = None;
        for (from, to) in waits.iter().copied() {
            match &mut current {
                Some((_, end)) if from <= *end => *end = (*end).max(to),
                _ => {
                    if let Some((a, b)) = current {
                        queued += b.duration_since(a);
                    }
                    current = Some((from, to));
                }
            }
        }
        if let Some((a, b)) = current {
            queued += b.duration_since(a);
        }
        let first = self.started();
        let start = waits.first().map_or(first, |w| w.0.min(first));
        let end = self
            .steps()
            .iter()
            .map(|e| e.ts)
            .max()
            .expect("a run has events");
        (millis(end.duration_since(start)), millis(queued))
    }
}

fn millis(d: SignedDuration) -> u64 {
    d.as_millis().max(0) as u64
}

fn report(filter: &Filter, events: &[Event], calls: &[Event]) -> Report {
    let mut grouped: BTreeMap<RunId, Vec<&Event>> = BTreeMap::new();
    for e in events {
        if e.kind != EventKind::Invocation && filter.project(e.project.as_deref()) {
            grouped.entry(e.run_id).or_default().push(e);
        }
    }
    let mut runs: Vec<RunEvents> = grouped
        .into_iter()
        .map(|(id, events)| RunEvents { id, events })
        .filter(|r| r.started() >= filter.since && r.started() < filter.until)
        .collect();
    runs.sort_by_key(RunEvents::started);
    let runs_without_requester = runs.iter().filter(|r| r.requester().is_none()).count();
    if filter.by_requester() {
        runs.retain(|r| r.requester().is_some_and(|q| filter.requester(q)));
    }
    let calls: Vec<&Event> = calls
        .iter()
        .filter(|c| c.kind == EventKind::Invocation)
        .filter(|c| c.ts >= filter.since && c.ts < filter.until)
        .filter(|c| filter.project.is_none() || c.project.as_deref() == filter.project)
        .filter(|c| !filter.by_requester() || filter.requester(&c.fields["requester"]))
        .collect();

    let summaries: Vec<Run> = runs.iter().map(summary).collect();
    let time = time(&summaries);
    let usage = usage(&runs, &summaries, &calls, events);
    Report {
        batch: Batch {
            since: filter.since,
            until: filter.until,
            project: filter.project.map(str::to_owned),
            worktrees: filter.worktrees.to_vec(),
            agents: filter.agents.to_vec(),
            runs_without_requester,
        },
        time,
        rework: rework(&runs),
        flaky: flaky(&runs),
        accuracy: accuracy(&runs, events),
        usage,
        runs: summaries,
    }
}

fn summary(r: &RunEvents) -> Run {
    let command = r.command();
    let (wall_ms, queued_ms) = r.times();
    let requester = r.requester();
    let text = |key: &str| requester.and_then(|q| q[key].as_str()).map(str::to_owned);
    Run {
        run_id: r.id,
        command,
        label: r.label(),
        started: r.started(),
        wall_ms,
        queued_ms,
        items: r.of(EventKind::ItemEnd).count(),
        result: r.result(command),
        worktree: text("worktree"),
        agent: text("agent"),
    }
}

fn failed(result: Option<&str>) -> bool {
    matches!(result, Some("fail" | "error" | "aborted" | "no_result"))
}

fn time(runs: &[Run]) -> Time {
    let mut by: BTreeMap<&'static str, CommandTime> = BTreeMap::new();
    for r in runs {
        let c = by.entry(r.command).or_insert(CommandTime {
            command: r.command,
            runs: 0,
            failed: 0,
            wall_ms: 0,
            queued_ms: 0,
        });
        c.runs += 1;
        c.failed += usize::from(failed(r.result));
        c.wall_ms += r.wall_ms;
        c.queued_ms += r.queued_ms;
    }
    let wall_ms: u64 = runs.iter().map(|r| r.wall_ms).sum();
    let queued_ms: u64 = runs.iter().map(|r| r.queued_ms).sum();
    Time {
        runs: runs.len(),
        wall_ms,
        queued_ms,
        running_ms: wall_ms.saturating_sub(queued_ms),
        queue_share: if wall_ms == 0 {
            0.0
        } else {
            (queued_ms as f64 / wall_ms as f64 * 1000.0).round() / 1000.0
        },
        by_command: by.into_values().collect(),
    }
}

/// Item ends of the batch in time order, with the worktree of their run.
fn item_ends<'a>(runs: &'a [RunEvents]) -> Vec<(Option<&'a str>, &'a Event)> {
    let mut all: Vec<(Option<&str>, &Event)> = runs
        .iter()
        .flat_map(|r| {
            let worktree = r.requester().and_then(|q| q["worktree"].as_str());
            r.of(EventKind::ItemEnd).map(move |e| (worktree, e))
        })
        .collect();
    all.sort_by_key(|(_, e)| e.ts);
    all
}

fn outcome(item_end: &Value) -> &'static str {
    if item_end["tests"]["unverified"].as_u64().unwrap_or(0) > 0 {
        "unverified"
    } else if item_end["passed"] == true {
        "pass"
    } else {
        "fail"
    }
}

fn rework(runs: &[RunEvents]) -> Rework {
    let mut items: BTreeMap<(Option<&str>, &str), ReworkItem> = BTreeMap::new();
    for (worktree, e) in item_ends(runs) {
        let name = e.fields["item"].as_str().unwrap_or_default();
        let entry = items.entry((worktree, name)).or_insert(ReworkItem {
            item: name.to_owned(),
            worktree: worktree.map(str::to_owned),
            runs: 0,
            failed: 0,
            rounds: 0,
            last: "pass",
        });
        if entry.runs > 0 && entry.last != "pass" {
            entry.rounds += 1;
        }
        entry.runs += 1;
        entry.last = outcome(&e.fields);
        entry.failed += usize::from(entry.last != "pass");
    }
    let mut items: Vec<ReworkItem> = items.into_values().filter(|i| i.rounds > 0).collect();
    items.sort_by(|a, b| b.rounds.cmp(&a.rounds).then_with(|| a.item.cmp(&b.item)));
    Rework {
        rounds: items.iter().map(|i| i.rounds).sum(),
        items,
    }
}

fn flaky(runs: &[RunEvents]) -> Vec<Flaky> {
    let mut item_runs: BTreeMap<&str, usize> = BTreeMap::new();
    let mut counts: BTreeMap<(String, &str), usize> = BTreeMap::new();
    for (_, e) in item_ends(runs) {
        let item = e.fields["item"].as_str().unwrap_or_default();
        *item_runs.entry(item).or_default() += 1;
        for test in e.fields["flaky"].as_array().into_iter().flatten() {
            let Some(test) = test.as_str() else { continue };
            // A whole item run again: "<item> (whole item: <why>; passed when run again)".
            let test = match test.split_once(" (whole item") {
                Some((name, _)) => format!("{name} (whole item)"),
                None => test.to_owned(),
            };
            *counts.entry((test, item)).or_default() += 1;
        }
    }
    let mut list: Vec<Flaky> = counts
        .into_iter()
        .map(|((test, item), flaky)| Flaky {
            test,
            item: item.to_owned(),
            flaky,
            item_runs: item_runs[item],
        })
        .collect();
    list.sort_by(|a, b| b.flaky.cmp(&a.flaky).then_with(|| a.test.cmp(&b.test)));
    list
}

fn usage(runs: &[RunEvents], summaries: &[Run], calls: &[&Event], all: &[Event]) -> Usage {
    let from_calls = !calls.is_empty();
    let mut commands: BTreeMap<String, CommandUse> = BTreeMap::new();
    let mut bad_arguments = Vec::new();
    let mut detours: BTreeMap<&'static str, Vec<Example>> = BTreeMap::new();
    let mut add = |kind: &'static str, ts: Timestamp, run_id: Option<RunId>, detail: String| {
        detours
            .entry(kind)
            .or_default()
            .push(Example { ts, run_id, detail });
    };

    if from_calls {
        for c in calls {
            let f = &c.fields;
            let command = f["command"].as_str().map(str::to_owned);
            let args: Vec<String> = serde_json::from_value(f["args"].clone()).unwrap_or_default();
            let error = f["error"].as_str().map(str::to_owned);
            let entry = commands
                .entry(command.clone().unwrap_or_else(|| "(none)".to_owned()))
                .or_insert_with_key(|name| CommandUse {
                    command: name.clone(),
                    calls: 0,
                    bad_arguments: Some(0),
                    errors: 0,
                });
            entry.calls += 1;
            let line = format!(
                "kr {}: {}",
                args.join(" "),
                error.as_deref().unwrap_or("no message")
            );
            if f["exit_code"] == crate::output::BAD_ARGUMENTS {
                *entry.bad_arguments.get_or_insert(0) += 1;
                add("bad_arguments", c.ts, None, line);
                bad_arguments.push(Call {
                    ts: c.ts,
                    command,
                    args,
                    error,
                });
            } else if error.is_some() {
                entry.errors += 1;
                add("kr_error", c.ts, None, line);
            }
        }
    } else {
        for s in summaries {
            let entry = commands
                .entry(s.command.to_owned())
                .or_insert_with(|| CommandUse {
                    command: s.command.to_owned(),
                    calls: 0,
                    bad_arguments: None,
                    errors: 0,
                });
            entry.calls += 1;
            entry.errors += usize::from(s.result == Some("error"));
        }
        for r in runs {
            if let Some(end) = r
                .of(EventKind::RunEnd)
                .find(|e| e.fields.get("error").is_some_and(|e| !e.is_null()))
            {
                add(
                    "kr_error",
                    end.ts,
                    Some(r.id),
                    end.fields["error"].as_str().unwrap_or_default().to_owned(),
                );
            }
        }
    }

    let mut passed_inputs: BTreeSet<(Option<&str>, &str, &str)> = BTreeSet::new();
    for (r, s) in runs.iter().zip(summaries) {
        for e in r.of(EventKind::RunAborted) {
            let f = &e.fields;
            add(
                "aborted",
                e.ts,
                Some(r.id),
                format!(
                    "{}; not started: {}",
                    f["signal"].as_str().unwrap_or("signal"),
                    list(&f["not_started"])
                ),
            );
        }
        if matches!(s.command, "run" | "gate") {
            let build_failed = r
                .first(EventKind::RunEnd)
                .is_some_and(|f| f["build_failed"] == true)
                || r.of(EventKind::StepEnd).any(|e| {
                    e.fields["command"] != "check"
                        && e.fields["ok"] == false
                        && e.fields["step"]
                            .as_str()
                            .is_some_and(|s| s.starts_with("build"))
                });
            if build_failed {
                add(
                    "build_failed_in_run",
                    s.started,
                    Some(r.id),
                    "the build failed after the run took its slots; `kr check` finds this before the queue".to_owned(),
                );
            }
            if s.result == Some("no_result") {
                add(
                    "no_result",
                    s.started,
                    Some(r.id),
                    format!("started without a result: {}", r.unfinished().join(", ")),
                );
            }
        }
        for e in r.of(EventKind::FilterMismatch) {
            add(
                "filter_mismatch",
                e.ts,
                Some(r.id),
                format!(
                    "{}: `{}` selects no test on the device",
                    e.fields["item"].as_str().unwrap_or_default(),
                    e.fields["filter"].as_str().unwrap_or_default()
                ),
            );
        }
        for e in r.of(EventKind::ProductsMissing) {
            add(
                "products_missing",
                e.ts,
                Some(r.id),
                format!(
                    "{}: test products gone from {}",
                    e.fields["item"].as_str().unwrap_or_default(),
                    e.fields["products"].as_str().unwrap_or_default()
                ),
            );
        }
        let worktree = r.requester().and_then(|q| q["worktree"].as_str());
        for e in r.of(EventKind::ItemEnd) {
            let (Some(item), Some(hash)) = (
                e.fields["item"].as_str(),
                e.fields["identity"]["input_hash"].as_str(),
            ) else {
                continue;
            };
            if passed_inputs.contains(&(worktree, item, hash)) {
                add(
                    "repeat_after_pass",
                    e.ts,
                    Some(r.id),
                    format!("{item} ran again on build inputs it had passed on"),
                );
            }
            if e.fields["passed"] == true {
                passed_inputs.insert((worktree, item, hash));
            }
        }
        let decided: BTreeSet<&str> = r
            .of(EventKind::Decision)
            .filter_map(|e| e.fields["id"].as_str())
            .collect();
        for e in r.of(EventKind::DecisionPoint) {
            let f = &e.fields;
            if f["id"].as_str().is_some_and(|id| !decided.contains(id)) {
                add(
                    "undecided",
                    e.ts,
                    None,
                    format!(
                        "{}: {} {}",
                        f["id"].as_str().unwrap_or_default(),
                        f["item"].as_str().unwrap_or_default(),
                        f["test"].as_str().unwrap_or_default()
                    ),
                );
            }
        }
    }

    let reconciled = reconciliations(all);
    let wrong_decisions = runs
        .iter()
        .flat_map(|r| r.of(EventKind::Decision).map(move |e| (r.id, e)))
        .filter_map(|(run_id, d)| {
            let rec = reconciled.get(d.fields["id"].as_str()?)?;
            (rec["supported"] == false).then(|| WrongDecision {
                id: d.fields["id"].as_str().unwrap_or_default().to_owned(),
                kind: d.fields["kind"].as_str().unwrap_or_default().to_owned(),
                item: d.fields["item"].as_str().map(str::to_owned),
                test: d.fields["test"].as_str().map(str::to_owned),
                choice: d.fields["choice"].as_str().unwrap_or_default().to_owned(),
                later_outcome: rec["later"]["outcome"].as_str().map(str::to_owned),
                decided_run: run_id,
                later_run: rec["later"]["run_id"].as_str().map(str::to_owned),
            })
        })
        .collect();

    Usage {
        source: if from_calls { "invocations" } else { "events" },
        commands: commands.into_values().collect(),
        bad_arguments,
        detours: DETOURS
            .iter()
            .map(|kind| {
                let examples = detours.remove(kind).unwrap_or_default();
                Detour {
                    kind,
                    count: examples.len(),
                    examples,
                }
            })
            .collect(),
        wrong_decisions,
    }
}

fn list(v: &Value) -> String {
    let names: Vec<&str> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

/// Reconciliations by decision ID, from every event: a decision can be reconciled by a
/// run outside the batch.
fn reconciliations(all: &[Event]) -> BTreeMap<&str, &Value> {
    all.iter()
        .filter(|e| e.kind == EventKind::Reconciliation)
        .filter_map(|e| Some((e.fields["id"].as_str()?, &e.fields)))
        .collect()
}

fn accuracy(runs: &[RunEvents], all: &[Event]) -> Vec<Accuracy> {
    let reconciled = reconciliations(all);
    let mut kinds: BTreeMap<String, (Score, usize, BTreeMap<String, Score>)> = BTreeMap::new();
    for d in runs.iter().flat_map(|r| r.of(EventKind::Decision)) {
        let f = &d.fields;
        let kind = f["kind"].as_str().unwrap_or("unknown").to_owned();
        let (score, pending_review, choices) = kinds.entry(kind).or_default();
        if f["via"] == "default" {
            *pending_review += usize::from(f["review"] == "pending");
            continue;
        }
        let rec = f["id"].as_str().and_then(|id| reconciled.get(id));
        let choice = f["choice"].as_str().unwrap_or("unknown").to_owned();
        for s in [&mut *score, choices.entry(choice).or_default()] {
            s.decided += 1;
            match rec {
                Some(rec) => {
                    s.reconciled += 1;
                    s.supported += usize::from(rec["supported"] == true);
                }
                None => s.pending += 1,
            }
            s.rate = (s.reconciled > 0).then(|| s.supported as f64 / s.reconciled as f64);
        }
    }
    kinds
        .into_iter()
        .map(
            |(kind, (score, default_pending_review, choices))| Accuracy {
                kind,
                score,
                default_pending_review,
                choices: choices
                    .into_iter()
                    .map(|(choice, score)| ChoiceScore { choice, score })
                    .collect(),
            },
        )
        .collect()
}

fn dur(ms: u64) -> String {
    human_duration(Duration::from_millis(ms))
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        "0%".to_owned()
    } else {
        format!("{}%", (part as f64 / whole as f64 * 100.0).round())
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn human(r: &Report) -> String {
    let mut s = String::new();
    let mut line = |l: String| {
        s.push_str(&l);
        s.push('\n');
    };
    let b = &r.batch;
    let range = format!(
        "{} → {} UTC",
        b.since.strftime("%Y-%m-%d %H:%M"),
        b.until.strftime("%Y-%m-%d %H:%M")
    );
    let who = b.project.as_deref().unwrap_or("all projects");
    let filtered = if b.worktrees.is_empty() && b.agents.is_empty() {
        String::new()
    } else {
        format!(
            " (only {}; {} without a requester left out)",
            b.worktrees
                .iter()
                .chain(&b.agents)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
            plural(b.runs_without_requester, "run", "runs")
        )
    };
    if r.runs.is_empty() {
        line(format!(
            "no runs of {who} started in {range}{filtered}; widen the range with --since (e.g. --since 7d) or check --project"
        ));
        return s;
    }
    let t = &r.time;
    let decisions: usize = r.accuracy.iter().map(|a| a.score.reconciled).sum();
    let held: usize = r.accuracy.iter().map(|a| a.score.supported).sum();
    line(format!(
        "{who} · {range}{filtered}: {} · {} wall, {} queued · {} · {} · {held} of {decisions} reconciled decisions held",
        plural(t.runs, "run", "runs"),
        dur(t.wall_ms),
        percent(t.queued_ms, t.wall_ms),
        plural(r.rework.rounds, "rework round", "rework rounds"),
        plural(r.flaky.len(), "flaky test", "flaky tests"),
    ));

    line(String::new());
    line("Time (wall is per run, so parallel runs add up)".to_owned());
    for c in &t.by_command {
        let failed = if c.failed > 0 {
            format!(", {} failed", c.failed)
        } else {
            String::new()
        };
        line(format!(
            "  {:<6} {}{failed} · {} wall · {} queued ({})",
            c.command,
            plural(c.runs, "run", "runs"),
            dur(c.wall_ms),
            dur(c.queued_ms),
            percent(c.queued_ms, c.wall_ms)
        ));
    }
    let mut longest: Vec<&Run> = r.runs.iter().collect();
    longest.sort_by_key(|r| std::cmp::Reverse(r.wall_ms));
    for run in longest.iter().take(3) {
        line(format!(
            "  longest: {} {} · {} ({} queued){}",
            run.started.strftime("%H:%M"),
            run.label.as_deref().unwrap_or(run.command),
            dur(run.wall_ms),
            dur(run.queued_ms),
            run.result.map(|r| format!(" · {r}")).unwrap_or_default()
        ));
    }

    line(String::new());
    line(format!(
        "Rework: {} (runs of an item after it failed or was unverified)",
        plural(r.rework.rounds, "round", "rounds")
    ));
    for i in r.rework.items.iter().take(10) {
        line(format!(
            "  {}: {}, {} failed, last {}{}",
            i.item,
            plural(i.runs, "run", "runs"),
            i.failed,
            i.last,
            i.worktree
                .as_deref()
                .map(|w| format!(" ({w})"))
                .unwrap_or_default()
        ));
    }
    if r.rework.items.len() > 10 {
        line(format!("  and {} more items", r.rework.items.len() - 10));
    }

    line(String::new());
    line(format!("Flaky: {}", plural(r.flaky.len(), "test", "tests")));
    for f in r.flaky.iter().take(10) {
        line(format!(
            "  {}× {} ({}, {} of the item)",
            f.flaky,
            f.test,
            f.item,
            plural(f.item_runs, "run", "runs")
        ));
    }
    if r.flaky.len() > 10 {
        line(format!("  and {} more tests", r.flaky.len() - 10));
    }

    let u = &r.usage;
    line(String::new());
    line(match u.source {
        "invocations" => "Agent usage (from the invocation log)".to_owned(),
        _ => "Agent usage (inferred from events: no invocation log in this range, so commands without events and bad arguments are not counted)".to_owned(),
    });
    line(format!(
        "  commands: {}",
        u.commands
            .iter()
            .map(|c| {
                let mut part = format!("{} {}", c.command, c.calls);
                let mut notes = Vec::new();
                if let Some(n) = c.bad_arguments.filter(|n| *n > 0) {
                    notes.push(plural(n, "bad arguments", "bad arguments"));
                }
                if c.errors > 0 {
                    notes.push(plural(c.errors, "error", "errors"));
                }
                if !notes.is_empty() {
                    part.push_str(&format!(" ({})", notes.join(", ")));
                }
                part
            })
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let detours: Vec<&Detour> = u.detours.iter().filter(|d| d.count > 0).collect();
    if detours.is_empty() {
        line("  detours: none".to_owned());
    } else {
        line("  detours:".to_owned());
    }
    for d in detours {
        line(format!("    {} {}", d.kind, d.count));
        for e in d.examples.iter().take(3) {
            let run = e
                .run_id
                .map(|id| format!("{} ", short(id)))
                .unwrap_or_default();
            line(format!(
                "      {} {run}{}",
                e.ts.strftime("%H:%M"),
                e.detail
            ));
        }
        if d.examples.len() > 3 {
            line(format!("      and {} more", d.examples.len() - 3));
        }
    }
    if u.wrong_decisions.is_empty() {
        line("  decisions that did not hold: none".to_owned());
    } else {
        line("  decisions that did not hold:".to_owned());
    }
    for w in &u.wrong_decisions {
        line(format!(
            "    {} {} {} {}: chose {}, a later run {}",
            w.id,
            w.kind,
            w.item.as_deref().unwrap_or(""),
            w.test.as_deref().unwrap_or(""),
            w.choice,
            w.later_outcome.as_deref().unwrap_or("did not say")
        ));
    }

    line(String::new());
    line("Decision accuracy (from reconciliation events)".to_owned());
    if r.accuracy.is_empty() {
        line("  no decisions in this range".to_owned());
    }
    for a in &r.accuracy {
        line(format!("  {}: {}", a.kind, score(&a.score)));
        for c in &a.choices {
            line(format!("    {}: {}", c.choice, score(&c.score)));
        }
        if a.default_pending_review > 0 {
            line(format!(
                "    taken by default, waiting for review: {}",
                a.default_pending_review
            ));
        }
    }

    line(String::new());
    line("This report is data only. Whether to trust the agent with more is for the owner to set in the agent host (ADR 0012).".to_owned());
    s
}

fn score(s: &Score) -> String {
    let rate = s
        .rate
        .map(|r| format!(" ({}%)", (r * 100.0).round()))
        .unwrap_or_default();
    format!(
        "{} of {} reconciled held{rate} · {} decided · {} waiting for a later run",
        s.supported, s.reconciled, s.decided, s.pending
    )
}

fn short(id: RunId) -> String {
    id.to_string()[..13].to_owned()
}
