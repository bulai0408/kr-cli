//! Repository-owned quarantine policy; execution and raw test outcomes never change.
use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use jiff::{SignedDuration, Timestamp};
use kr_events::{Event, EventKind, RunId};
use kr_run::Final;
use kr_run::decision::{Affects, Choice, Decision, Suggestion};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::run::ItemResult;
use crate::Loaded;

pub(crate) const FILE: &str = ".kr/quarantine.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Quarantine {
    schema_version: u32,
    limit: usize,
    basis: String,
    tests: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    item: String,
    test: String,
    reason: String,
    added: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issue: Option<String>,
}

impl Quarantine {
    pub(crate) fn load(root: &Path) -> Result<Self> {
        let file = root.join(FILE);
        let bytes = match std::fs::read(&file) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "cannot read {}; restore the repository quarantine list",
                        file.display()
                    )
                });
            }
        };
        let q: Self = serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "invalid {}; fix its JSON fields as described in ADR 0021",
                file.display()
            )
        })?;
        if q.schema_version != 1 || q.basis.trim().is_empty() {
            bail!(
                "{}: set schema_version to 1 and basis to the kr report range and flaky counts used to choose limit",
                file.display()
            );
        }
        if q.tests.len() > q.limit {
            bail!(
                "{}: {} entries exceed quarantine limit {}; remove entries or use kr report to justify a new limit and basis in a PR",
                file.display(),
                q.tests.len(),
                q.limit
            );
        }
        let mut keys = BTreeSet::new();
        for e in &q.tests {
            if [&e.item, &e.test, &e.reason]
                .iter()
                .any(|s| s.trim().is_empty())
                || e.added.parse::<jiff::civil::Date>().is_err()
                || e.issue.as_ref().is_some_and(|s| s.trim().is_empty())
                || !keys.insert((&e.item, &e.test))
            {
                bail!(
                    "{}: invalid or duplicate entry for {}/{}; use non-empty item, exact test, reason, added (YYYY-MM-DD), and optional non-empty issue; keep one entry per item/test",
                    file.display(),
                    e.item,
                    e.test
                );
            }
        }
        Ok(q)
    }

    pub(crate) fn contains(&self, item: &str, test: &str) -> bool {
        self.tests.iter().any(|e| e.item == item && e.test == test)
    }

    pub(crate) fn apply(&self, r: &mut ItemResult) {
        r.quarantined = r
            .cases
            .iter()
            .filter(|c| {
                r.unverified_count() == 0
                    && c.outcome == Final::Failed
                    && self.contains(&r.name, &c.id)
            })
            .map(|c| c.id.clone())
            .collect();
        r.blocking = Some(!r.passed && !self.exempt(r));
    }

    /// An item is exempt only when every failure has an entry and all other checks held.
    fn exempt(&self, r: &ItemResult) -> bool {
        !r.passed
            && r.only_test_failures
            && !r.cases.is_empty()
            && r.cases
                .iter()
                .filter(|c| c.outcome == Final::Failed)
                .all(|c| self.contains(&r.name, &c.id))
    }

    pub(crate) fn rejudge(
        &self,
        saved: &kr_run::rejudge::Saved,
    ) -> Option<kr_run::rejudge::Change> {
        use kr_run::rejudge::{Change, Conclusion, compare};
        let item = saved
            .quarantine
            .as_ref()
            .map_or(saved.item.as_str(), |q| q.item.as_str());
        let now = kr_run::judge(&saved.enumeration, &saved.first, &saved.reruns);
        let eligible = saved
            .quarantine
            .as_ref()
            .map_or_else(|| complete_reruns(saved), |q| q.eligible);
        let before = saved.verdict.passed()
            || saved
                .quarantine
                .as_ref()
                .is_some_and(|q| !q.tests.is_empty() && !q.blocking);
        let after = now.passed()
            || (eligible
                && now.reasons.is_empty()
                && now
                    .tests
                    .iter()
                    .filter(|t| t.outcome == Final::Failed)
                    .all(|t| self.contains(item, &t.id)));
        let raw = compare(&saved.verdict, &now);
        let tests = raw.map(|c| c.tests).unwrap_or_default();
        (before != after || !tests.is_empty()).then(|| Change {
            before: Conclusion {
                passed: before,
                reasons: saved.verdict.reasons.clone(),
            },
            after: Conclusion {
                passed: after,
                reasons: now.reasons,
            },
            tests,
        })
    }

    pub(crate) fn observe(&self, loaded: &Loaded, root: &Path, run: RunId) -> Result<()> {
        let events = super::report::history(loaded)?;
        let previous = events.iter().rev().find(|e| {
            e.kind == EventKind::QuarantineObserved
                && e.project.as_deref() == loaded.project()
                && e.fields["worktree"] == root.to_string_lossy().as_ref()
        });
        let before: Vec<Entry> = previous
            .and_then(|e| serde_json::from_value(e.fields["tests"].clone()).ok())
            .unwrap_or_default();
        for (kind, entries, other) in [
            (EventKind::QuarantineEnter, &self.tests, &before),
            (EventKind::QuarantineExit, &before, &self.tests),
        ] {
            for entry in entries
                .iter()
                .filter(|e| !other.iter().any(|o| o.item == e.item && o.test == e.test))
            {
                loaded.events().append(&Event::now(
                    run,
                    loaded.project(),
                    kind,
                    json!({"file": FILE, "worktree": root, "entry": entry}),
                ))?;
            }
        }
        loaded.events().append(&Event::now(run, loaded.project(), EventKind::QuarantineObserved,
            json!({"file": FILE, "worktree": root, "limit": self.limit, "basis": self.basis, "tests": self.tests})))
    }

    pub(crate) fn suggestions(
        &self,
        events: &[Event],
        project: Option<&str>,
        run: RunId,
        results: &[ItemResult],
        offset: usize,
    ) -> Vec<Decision> {
        let now = Timestamp::now();
        let mut points = Vec::new();
        let mut add = |item: &str, test: &str, action: &str, evidence: serde_json::Value| {
            points.push(Decision {
                id: format!("{run}-{}", offset + points.len() + 1),
                kind: format!("quarantine_{action}"), affects: Affects::Efficiency,
                question: format!("Should {test} in {item} {} the repository quarantine list through a PR?", if action == "add" { "enter" } else { "leave" }),
                item: item.to_owned(), test: Some(test.to_owned()),
                options: vec![Choice { name: "keep".to_owned(), means: "keep the current repository list".to_owned() }, Choice { name: action.to_owned(), means: "approve the suggested edit for a PR; kr does not edit the file or change this run".to_owned() }],
                evidence, default: Suggestion { option: "keep".to_owned(), reason: "kr never edits the repository list; the owner reviews additions and removals through a PR".to_owned() },
            });
        };
        let history = super::report::flaky_history(
            events,
            project,
            now - SignedDuration::from_hours(90 * 24),
            now,
        );
        let initial_limit = history
            .iter()
            .filter(|f| !f.test.ends_with(" (whole item)"))
            .count();
        for f in history {
            if f.flaky < 2
                || f.test.ends_with(" (whole item)")
                || self.contains(&f.item, &f.test)
                || !results.iter().any(|r| r.name == f.item)
            {
                continue;
            }
            let entry = json!({"item": f.item, "test": f.test, "reason": "repeated flakes; link the repair issue", "added": now.to_zoned(jiff::tz::TimeZone::UTC).date().to_string()});
            add(
                &f.item,
                &f.test,
                "add",
                json!({"days":90,"flaky": f.flaky,"item_runs": f.item_runs,"file":FILE,"edit":{"append_to":"tests","entry":entry},"limit":self.limit,"basis":self.basis,"initial_file": {"schema_version":1,"limit":initial_limit,"basis":format!("kr report: {initial_limit} individually flaky tests in the last 90 days for {}", project.unwrap_or("this project")),"tests":[entry]},"capacity_available":self.tests.len() < self.limit}),
            );
        }
        for entry in &self.tests {
            if !results.iter().any(|r| r.name == entry.item) {
                continue;
            }
            let since = now - SignedDuration::from_hours(30 * 24);
            let (complete, bad) =
                super::report::test_health(events, project, since, now, &entry.item, &entry.test);
            if complete >= 10 && bad == 0 {
                add(
                    &entry.item,
                    &entry.test,
                    "remove",
                    json!({"days":30,"complete_runs":complete,"failed_or_flaky":bad,"file":FILE,"edit":{"remove_from":"tests","entry":entry}}),
                );
            }
        }
        points
    }
}

/// Every failed test must have a complete, non-timeout solo result, not just an attempt.
pub(crate) fn complete_reruns(saved: &kr_run::rejudge::Saved) -> bool {
    kr_run::Verdict::to_rerun(&saved.first)
        .iter()
        .enumerate()
        .all(|(i, test)| {
            saved
                .reruns
                .get(i)
                .and_then(Option::as_ref)
                .is_some_and(|a| {
                    !a.timed_out
                        && a.results.as_ref().is_some_and(|r| {
                            r.iter().any(|c| {
                                &c.id == test && c.outcome != kr_xcode::TestOutcome::Skipped
                            })
                        })
                })
        })
}

pub(crate) fn save_state(r: &ItemResult) -> Result<()> {
    let Some(log) = &r.log else {
        return Ok(());
    };
    let file = log.join("verdict.json");
    if !file.is_file() {
        return Ok(());
    }
    let mut saved: kr_run::rejudge::Saved = serde_json::from_slice(&std::fs::read(&file)?)?;
    saved.quarantine = Some(kr_run::rejudge::QuarantineState {
        item: r.name.clone(),
        eligible: r.only_test_failures && r.unverified_count() == 0,
        blocking: r.blocking(),
        tests: r.quarantined.clone(),
    });
    std::fs::write(&file, serde_json::to_vec(&saved)?)
        .with_context(|| format!("cannot write {}", file.display()))
}

/// A reviewed edit is reconciled when a later run actually observes that entry enter
/// or leave. Running before a PR lands does not count as a wrong decision.
pub(crate) fn reconcile(loaded: &Loaded, run: RunId, events: &[Event]) -> Result<()> {
    let done: BTreeSet<_> = events
        .iter()
        .filter(|e| e.kind == EventKind::Reconciliation)
        .filter_map(|e| e.fields["id"].as_str())
        .collect();
    for d in events.iter().filter(|e| {
        e.kind == EventKind::Decision
            && matches!(
                e.fields["kind"].as_str(),
                Some("quarantine_add" | "quarantine_remove")
            )
            && e.fields["via"] != "default"
            && e.fields["id"].as_str().is_some_and(|id| !done.contains(id))
    }) {
        let Some(later) = events.iter().find(|e| {
            matches!(
                e.kind,
                EventKind::QuarantineEnter | EventKind::QuarantineExit
            ) && e.ts > d.ts
                && e.project == d.project
                && e.fields["worktree"] == d.fields["by"]["worktree"]
                && e.fields["entry"]["item"] == d.fields["item"]
                && e.fields["entry"]["test"] == d.fields["test"]
        }) else {
            continue;
        };
        let present = later.kind == EventKind::QuarantineEnter;
        let expected = match d.fields["choice"].as_str() {
            Some("add") => true,
            Some("remove") => false,
            _ => d.fields["kind"] == "quarantine_remove",
        };
        loaded.events().append(&Event::now(run, d.project.as_deref(), EventKind::Reconciliation, json!({
            "id":d.fields["id"], "decided_run":d.run_id, "kind":d.fields["kind"],
            "item":d.fields["item"], "test":d.fields["test"], "choice":d.fields["choice"],
            "later":{"run_id":later.run_id,"outcome":if present {"quarantined"} else {"not_quarantined"}},
            "supported":present == expected,"rule":"first later observed entry change on the same project and worktree; PR edits only"
        })))?;
    }
    Ok(())
}
