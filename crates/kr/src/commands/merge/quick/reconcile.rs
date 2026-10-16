use anyhow::Result;
use kr_events::{Event, EventKind, RunId};
use kr_exec::Runner;
use serde_json::{Value, json};

use super::super::predict::{KIND, Pending, pending_path};
use super::super::{checked_git, git};
use super::problem;
use crate::{Ctx, output::BadArguments};

pub(super) fn run(
    ctx: &Ctx,
    runner: &dyn Runner,
    id: &str,
    verification: RunId,
    compiled: bool,
    attempted: bool,
    problems: &mut Vec<Value>,
) -> Result<Option<Value>> {
    let run: RunId = id
        .rsplit_once('-')
        .and_then(|(r, _)| r.parse().ok())
        .ok_or_else(|| {
            BadArguments("invalid decision ID; use an ID from kr merge predict".to_owned())
        })?;
    let loaded = ctx.load()?;
    let file = pending_path(&loaded, run);
    let pending: Pending = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| {
            BadArguments(
                "merge decision does not exist; use an ID from kr merge predict".to_owned(),
            )
        })?;
    let d = pending
        .decisions
        .iter()
        .find(|d| d.id == id)
        .ok_or_else(|| BadArguments("merge decision does not exist in this run".to_owned()))?;
    let made = pending.made.get(id).ok_or_else(|| {
        BadArguments("hand back the resolution with kr decide before reconciliation".to_owned())
    })?;
    if made["choice"] == "reject" {
        return Err(BadArguments("a rejected merge has no resolution to verify".to_owned()).into());
    }
    let history = crate::commands::decide::history(&loaded);
    if let Some(e) = history
        .iter()
        .find(|e| e.kind == EventKind::Reconciliation && e.fields["id"] == id)
    {
        let mut fields = e.fields.clone();
        fields["already_recorded"] = json!(true);
        if fields["supported"] != true {
            problem(
                problems,
                "resolution_unsupported",
                ".kr.toml",
                1,
                "this resolution previously failed verification",
                "create a new merge prediction for a different resolution",
            );
        }
        return Ok(Some(fields));
    }
    let tree = checked_git(ctx, runner, &["rev-parse", "HEAD^{tree}"])?;
    let clean = checked_git(ctx, runner, &["status", "--porcelain"])?;
    let mut same =
        clean.trim().is_empty() && ctx.cwd.canonicalize()? == pending.root.canonicalize()?;
    for side in ["left", "right"] {
        let commit = d.evidence[side]["commit"].as_str().unwrap_or("");
        same &= git(
            ctx,
            runner,
            &["merge-base", "--is-ancestor", commit, "HEAD"],
        )?
        .success();
    }
    let decision = history
        .iter()
        .find(|e| e.kind == EventKind::Decision && e.fields["id"] == id);
    let ends: Vec<_> = history
        .iter()
        .filter(|e| e.run_id == verification && e.kind == EventKind::ItemEnd)
        .collect();
    let end = history
        .iter()
        .find(|e| e.run_id == verification && e.kind == EventKind::RunEnd);
    let tested = ends
        .iter()
        .filter_map(|e| e.fields["tests"].as_object())
        .map(|t| {
            ["passed", "failed", "flaky"]
                .iter()
                .map(|k| t.get(*k).and_then(Value::as_u64).unwrap_or(0))
                .sum::<u64>()
        })
        .sum::<u64>();
    same &= !ends.is_empty()
        && ends
            .iter()
            .all(|e| e.fields["identity"]["tree"].as_str() == Some(tree.trim()));
    same &= decision.is_some_and(|d| end.is_some_and(|end| end.ts > d.ts)) && verification > run;
    if !same || tested == 0 || end.is_none() || !attempted {
        problem(
            problems,
            "verification_pending",
            ".kr.toml",
            1,
            "resolution needs a clean merged tree, an incremental build and a later nonzero test run on that tree",
            "commit a merge containing both branches, run the affected tests, then pass that run ID to merge check",
        );
        return Ok(None);
    }
    let supported = compiled
        && problems.is_empty()
        && end.is_some_and(|e| e.fields["ok"] == true)
        && ends.iter().all(|e| {
            e.fields["passed"] == true && e.fields["tests"]["unverified"].as_u64().unwrap_or(0) == 0
        });
    let fields = json!({"id":id,"kind":KIND,"item":d.item,"choice":made["choice"],
        "decided_run":run,"later":{"run_id":verification,"tree":tree.trim()},
        "compiled":compiled,"tests":tested,"supported":supported,
        "rule":"incremental compilation and nonzero affected tests on the committed merge tree after the decision"});
    loaded.events().append(&Event::now(
        RunId::new(),
        loaded.project(),
        EventKind::Reconciliation,
        fields.clone(),
    ))?;
    if !supported {
        problem(
            problems,
            "resolution_unsupported",
            ".kr.toml",
            1,
            "compilation or affected tests do not support the resolution",
            "fix the resolution and create a new merge prediction",
        );
    }
    Ok(Some(fields))
}
