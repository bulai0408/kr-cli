use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use kr_events::{Event, EventKind, RunId};
use kr_exec::Runner;
use kr_run::decision::{Affects, Choice, Decision, Suggestion};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{checked_git, git, revision};
use crate::{
    Ctx, Loaded,
    output::{self, BadArguments},
};

pub(super) const KIND: &str = "merge_resolution";

#[derive(Serialize, Deserialize)]
pub(super) struct Pending {
    pub run_id: RunId,
    pub root: PathBuf,
    pub decisions: Vec<Decision>,
    pub made: BTreeMap<String, Value>,
}

pub(crate) fn pending_path(loaded: &Loaded, run: RunId) -> PathBuf {
    loaded.run_dir(run).join("merge-decisions.json")
}

fn paths(ctx: &Ctx, runner: &dyn Runner, base: &str, head: &str) -> Result<BTreeSet<String>> {
    Ok(checked_git(
        ctx,
        runner,
        &["diff", "--name-only", "-z", base, head, "--"],
    )?
    .split('\0')
    .filter(|s| !s.is_empty())
    .map(str::to_owned)
    .collect())
}

fn evidence(ctx: &Ctx, runner: &dyn Runner, base: &str, head: &str, name: &str) -> Result<Value> {
    let commits = checked_git(
        ctx,
        runner,
        &[
            "log",
            "--format=%H%n%B%x00",
            &format!("{base}..{head}"),
            "--",
        ],
    )?;
    let issues: BTreeSet<u64> = commits
        .split('#')
        .skip(1)
        .filter_map(|s| {
            s.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect();
    Ok(
        json!({"branch":name,"commit":head,"commits":commits.split('\0').filter(|s| !s.trim().is_empty()).collect::<Vec<_>>(),
        "issues":issues,"issues_missing":issues.is_empty()}),
    )
}

pub(super) fn run(ctx: &Ctx, base: &str, branches: &[String]) -> Result<ExitCode> {
    let runner = ctx.runner()?;
    let root = checked_git(ctx, &*runner, &["rev-parse", "--show-toplevel"])?;
    let local = Ctx {
        cwd: root.trim().into(),
        env: ctx.env.clone(),
        json: ctx.json,
    };
    let ctx = &local;
    let loaded = ctx.load()?;
    let base = revision(ctx, &*runner, base)?;
    let heads = branches
        .iter()
        .map(|b| revision(ctx, &*runner, b))
        .collect::<Result<Vec<_>>>()?;
    let changed = heads
        .iter()
        .map(|h| paths(ctx, &*runner, &base, h))
        .collect::<Result<Vec<_>>>()?;
    let run_id = RunId::new();
    let mut pairs = Vec::new();
    let mut decisions = Vec::new();
    let mut degrees = vec![0; branches.len()];
    let mut ancestors = vec![BTreeSet::new(); branches.len()];
    for i in 0..heads.len() {
        for j in i + 1..heads.len() {
            for (a, b) in [(i, j), (j, i)] {
                let out = git(
                    ctx,
                    &*runner,
                    &["merge-base", "--is-ancestor", &heads[a], &heads[b]],
                )?;
                match out.status {
                    Some(0) if heads[a] != heads[b] => {
                        ancestors[b].insert(a);
                    }
                    Some(0 | 1) => {}
                    _ => anyhow::bail!("cannot compare branch ancestry; check repository history"),
                }
            }
            let out = git(
                ctx,
                &*runner,
                &[
                    "merge-tree",
                    "--write-tree",
                    "--name-only",
                    "-z",
                    &heads[i],
                    &heads[j],
                ],
            )?;
            if !matches!(out.status, Some(0 | 1)) {
                anyhow::bail!(
                    "cannot predict merge of {} and {}; check their shared history",
                    branches[i],
                    branches[j]
                );
            }
            let conflicts: Vec<&str> = out
                .stdout
                .split('\0')
                .skip(1)
                .take_while(|s| !s.is_empty())
                .collect();
            if !out.success() {
                degrees[i] += 1;
                degrees[j] += 1;
            }
            let overlap: Vec<&String> = changed[i].intersection(&changed[j]).collect();
            pairs.push(
                json!({"left":branches[i],"right":branches[j],"conflicts":conflicts,
                "text_conflict":!out.success(),"overlap":overlap}),
            );
            if (overlap.is_empty()
                || ancestors[i].contains(&j)
                || ancestors[j].contains(&i)
                || heads[i] == heads[j])
                && out.success()
            {
                continue;
            }
            decisions.push(Decision {
                id: format!("{run_id}-{}", decisions.len()+1),
                kind: KIND.to_owned(),
                affects: Affects::Conclusion,
                question: format!("How should the requirements of {} and {} be combined?", branches[i], branches[j]),
                item: format!("{} + {}", branches[i], branches[j]),
                test: None,
                options: ["combine","ours","theirs","manual","reject"].into_iter().map(|name| Choice {
                    name: name.to_owned(),
                    means: match name {
                        "combine" => "preserve both requirements",
                        "ours" => "keep the left requirement",
                        "theirs" => "keep the right requirement",
                        "manual" => "apply the resolution described in --reason",
                        _ => "do not merge these branches",
                    }.to_owned(),
                }).collect(),
                evidence: json!({"left":evidence(ctx,&*runner,&base,&heads[i],&branches[i])?,
                    "right":evidence(ctx,&*runner,&base,&heads[j],&branches[j])?,
                    "overlap":overlap,"conflicts":conflicts,"base":base}),
                default: Suggestion { option:"manual".to_owned(), reason:"file overlap requires review of both requirements; a clean text merge does not prove logical compatibility".to_owned() },
            });
        }
    }
    let mut order = Vec::new();
    let mut remaining: BTreeSet<usize> = (0..heads.len()).collect();
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .copied()
            .filter(|i| ancestors[*i].is_disjoint(&remaining))
            .min_by_key(|i| (degrees[*i], *i))
            .context("branch ancestry cycle")?;
        remaining.remove(&next);
        order.push(branches[next].clone());
    }
    if !decisions.is_empty() {
        let pending = Pending {
            run_id,
            root: ctx.cwd.clone(),
            decisions: decisions.clone(),
            made: BTreeMap::new(),
        };
        let file = pending_path(&loaded, run_id);
        std::fs::create_dir_all(file.parent().unwrap())?;
        std::fs::write(file, serde_json::to_vec_pretty(&pending)?)?;
        for d in &decisions {
            loaded.events().append(&Event::now(
                run_id,
                loaded.project(),
                EventKind::DecisionPoint,
                serde_json::to_value(d)?,
            ))?;
        }
    }
    let ok = decisions.is_empty();
    if ctx.json {
        output::print_json(&json!({"ok":ok,"concluded":ok,"run_id":run_id,
            "pairs":pairs,"suggested_order":order,"order_reason":"ancestors first, then fewer conflicting pairs, then input order",
            "limitations":["pairwise prediction does not simulate cumulative merges","cross-file logical conflicts require agent review"],
            "decisions":decisions}))?;
    } else {
        println!(
            "Merge prediction: {} pair(s), {} decision point(s).",
            pairs.len(),
            decisions.len()
        );
        println!(
            "Suggested order: {} (ancestors first, then fewer conflicts).",
            order.join(" -> ")
        );
        for pair in &pairs {
            println!(
                "{} + {}: {}",
                pair["left"],
                pair["right"],
                if pair["text_conflict"] == true {
                    "text conflict"
                } else {
                    "no text conflict"
                }
            );
        }
        if !decisions.is_empty() {
            println!("No verdict yet; inspect both requirements with --json.");
            super::super::decide::print_pending(&decisions.iter().collect::<Vec<_>>());
        }
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(output::DECISIONS_PENDING)
    })
}

pub(crate) fn decide(
    ctx: &Ctx,
    loaded: &Loaded,
    run: RunId,
    choices: &[(String, String)],
    reason: &str,
    via: &str,
) -> Result<ExitCode> {
    let file = pending_path(loaded, run);
    let mut pending: Pending = serde_json::from_slice(&std::fs::read(&file)?)?;
    // Validate the whole batch before writing any immutable choice.
    let mut ids = BTreeSet::new();
    for (id, option) in choices {
        if !ids.insert(id) || reason.trim().is_empty() {
            return Err(BadArguments(
                "each decision needs one choice and a nonempty --reason".to_owned(),
            )
            .into());
        }
        let d = pending
            .decisions
            .iter()
            .find(|d| d.id == *id)
            .ok_or_else(|| {
                BadArguments(format!(
                    "unknown merge decision `{id}`; use an ID from `kr merge predict --json`"
                ))
            })?;
        if pending.made.contains_key(id) {
            return Err(BadArguments(format!(
                "`{id}` is decided already; a decision cannot change"
            ))
            .into());
        }
        if d.option(option).is_none() {
            return Err(BadArguments(format!(
                "unknown option `{option}`; choose combine, ours, theirs, manual or reject"
            ))
            .into());
        }
    }
    for (id, option) in choices {
        let d = pending.decisions.iter().find(|d| d.id == *id).unwrap();
        let made = json!({"choice":option,"reason":reason,"via":via,"by":{
            "agent":ctx.env_var("KR_REQUESTER").or_else(||ctx.env_var("AI_AGENT")),
                "worktree":ctx.cwd,
                // SAFETY: getppid has no preconditions.
                "parent_pid":unsafe {libc::getppid()}},"evidence":d.evidence,"id":id,"kind":KIND,"item":d.item});
        loaded.events().append(&Event::now(
            run,
            loaded.project(),
            EventKind::Decision,
            made.clone(),
        ))?;
        pending.made.insert(id.clone(), made);
    }
    std::fs::write(file, serde_json::to_vec_pretty(&pending)?)?;
    let left: Vec<&Decision> = pending
        .decisions
        .iter()
        .filter(|d| !pending.made.contains_key(&d.id))
        .collect();
    let rejected = pending.made.values().any(|m| m["choice"] == "reject");
    if left.is_empty() {
        loaded.events().append(&Event::now(
            run,
            loaded.project(),
            EventKind::RunDecided,
            json!({
                "label":"merge predict","ok":false,"passed":0,"total":pending.decisions.len(),
                "verification":if rejected {"rejected"} else {"pending"},
                "decisions":pending.decisions.iter().map(|d|&d.id).collect::<Vec<_>>(),
            }),
        ))?;
    }
    let value = json!({"ok":false,"concluded":left.is_empty(),"run_id":run,
        "verification":if rejected {"rejected"} else {"pending"},"decisions":left,
        "next":"apply the resolution, commit it, run the affected tests, then kr merge check --decision <id> --verification-run <run-id>"});
    if ctx.json {
        output::print_json(&value)?;
    } else {
        println!(
            "Resolution recorded; merge verification {}. Apply and commit the resolution, run affected tests, then `kr merge check --decision <id> --verification-run <run-id>`.",
            if rejected { "rejected" } else { "pending" }
        );
        if !left.is_empty() {
            super::super::decide::print_pending(&left);
        }
    }
    Ok(if left.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::from(output::DECISIONS_PENDING)
    })
}
