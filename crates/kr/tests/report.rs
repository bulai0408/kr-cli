//! `kr report`: the batch report from events and the invocation log (ADR 0016). The main
//! fixture is the real event log of Krill batches 7 and 8 (tests/fixtures/report).

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{Value, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Batch 7: Krill #153, its pre-merge check and the stopped post-merge run.
const BATCH_7: [&str; 4] = [
    "--since",
    "2026-10-14T09:40:00Z",
    "--until",
    "2026-10-14T10:55:00Z",
];
/// Batch 8: Krill #125 and #133 up to their merged pre-merge check.
const BATCH_8: [&str; 4] = [
    "--since",
    "2026-10-14T10:55:00Z",
    "--until",
    "2026-10-14T16:20:00Z",
];

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    /// A home whose state directory has the real events of Krill batches 7 and 8.
    fn krill() -> Self {
        let home = Self::empty();
        let events = home.state().join("events");
        std::fs::create_dir_all(&events).unwrap();
        std::fs::copy(
            root().join("tests/fixtures/report/krill-batch7-8/events/2026-10-14.ndjson"),
            events.join("2026-10-14.ndjson"),
        )
        .unwrap();
        home
    }

    fn empty() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("repo/.git")).unwrap();
        Self { dir }
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join(".local/state/kr")
    }

    /// Adds lines to a log (`events` or `invocations`) in the day file of their date.
    fn add(&self, log: &str, day: &str, lines: &[Value]) {
        let path = self.state().join(format!("{log}/{day}.ndjson"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        for l in lines {
            text.push_str(&l.to_string());
            text.push('\n');
        }
        std::fs::write(path, text).unwrap();
    }

    fn kr(&self, args: &[&str]) -> (Option<i32>, String) {
        let out = Command::cargo_bin("kr")
            .unwrap()
            .env_clear()
            .env("HOME", self.dir.path())
            .env("NO_COLOR", "1")
            .env("KR_REQUESTER", "test-agent")
            .current_dir(self.dir.path().join("repo"))
            .args(args)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr)
                .lines()
                .filter(|l| l.starts_with("error"))
                .collect::<Vec<_>>()
                .join("\n");
        (out.status.code(), text)
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["report", "--json"];
        all.extend_from_slice(args);
        let (code, out) = self.kr(&all);
        assert_eq!(code, Some(0), "{out}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    fn invocations(&self) -> Vec<Value> {
        let dir = self.state().join("invocations");
        let mut all = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            all.extend(
                text.lines()
                    .map(|l| serde_json::from_str::<Value>(l).unwrap()),
            );
        }
        all
    }
}

fn event(ts: &str, run: &str, kind: &str, fields: Value) -> Value {
    json!({ "ts": ts, "run_id": run, "project": "Krill", "type": kind, "fields": fields })
}

fn find<'a>(list: &'a Value, key: &str, value: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|v| v[key] == value)
        .unwrap_or_else(|| panic!("no {key} = {value} in {list:#}"))
}

#[test]
fn the_batch_7_report_starts_with_the_conclusion() {
    let home = Home::krill();
    let mut args = vec!["report", "--project", "Krill"];
    args.extend(BATCH_7);
    let (code, out) = home.kr(&args);
    assert_eq!(code, Some(0), "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn the_batch_8_report_counts_time_rework_and_flaky_tests() {
    let home = Home::krill();
    let mut args = vec!["--project", "Krill"];
    args.extend(BATCH_8);
    let v = home.json(&args);
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["decisions"], json!([]));

    // Cross-checked with an independent script over the same events (see the commit).
    let time = &v["time"];
    assert_eq!(time["runs"], 88);
    assert_eq!(find(&time["by_command"], "command", "check")["runs"], 47);
    assert_eq!(find(&time["by_command"], "command", "run")["runs"], 38);
    assert_eq!(find(&time["by_command"], "command", "lock")["runs"], 3);
    let secs = |key: &str| (time[key].as_u64().unwrap() as f64 / 1000.0).round() as i64;
    assert!((secs("wall_ms") - 14701).abs() <= 1, "{time:#}");
    assert!((secs("queued_ms") - 5275).abs() <= 1, "{time:#}");

    assert_eq!(v["rework"]["rounds"], 22);
    let preload_unit = find(&v["rework"]["items"], "item", "preload-unit");
    assert_eq!(preload_unit["runs"], 22);
    assert_eq!(preload_unit["rounds"], 11);
    assert_eq!(preload_unit["last"], "pass");

    let flaky = v["flaky"].as_array().unwrap();
    assert_eq!(flaky.len(), 4, "{flaky:#?}");
    let whole = find(&v["flaky"], "test", "preload-ipad (whole item)");
    assert_eq!(whole["flaky"], 1);
    assert_eq!(whole["item_runs"], 4);

    // Batch 8 has no invocation log: the commands come from the events.
    assert_eq!(v["usage"]["source"], "events");
    assert_eq!(v["accuracy"], json!([]));
}

#[test]
fn detours_list_the_runs_that_went_around() {
    let home = Home::krill();
    let mut args = vec!["--project", "Krill"];
    args.extend(BATCH_7);
    let v = home.json(&args);
    let detours = &v["usage"]["detours"];
    // The post-merge run that died without results (kr-cli #25) and the one the owner
    // stopped with SIGTERM before run_aborted existed.
    let no_result = find(detours, "kind", "no_result");
    assert_eq!(no_result["count"], 2, "{no_result:#}");
    assert_eq!(
        no_result["examples"][0]["run_id"],
        "01a139fb-9e0a-75b5-8176-1466d6a20cc9"
    );
}

#[test]
fn decisions_are_scored_per_kind_and_choice() {
    let home = Home::empty();
    let run = "01a14000-0000-7000-8000-000000000001";
    let later = "01a14000-0000-7000-8000-000000000002";
    let decision = |id: &str, test: &str, choice: &str| {
        event(
            "2026-10-15T10:05:00Z",
            run,
            "decision",
            json!({ "id": id, "kind": "failure_kind", "item": "modules", "test": test,
                    "choice": choice, "reason": "…", "via": "kr decide" }),
        )
    };
    let reconciled = |id: &str, test: &str, choice: &str, outcome: &str, supported: bool| {
        event(
            "2026-10-15T11:00:00Z",
            later,
            "reconciliation",
            json!({ "id": id, "decided_run": run, "kind": "failure_kind", "item": "modules",
                    "test": test, "choice": choice,
                    "later": { "run_id": later, "outcome": outcome },
                    "input_hash": "abc", "supported": supported }),
        )
    };
    home.add(
        "events",
        "2026-10-15",
        &[
            event(
                "2026-10-15T10:00:00Z",
                run,
                "item_start",
                json!({ "item": "modules", "kind": "scenario", "device": "Krill-Verify-1" }),
            ),
            event(
                "2026-10-15T10:01:00Z",
                run,
                "item_end",
                json!({ "item": "modules", "kind": "scenario", "passed": false,
                        "duration_ms": 60000, "tests": { "failed": 4, "passed": 1 }, "flaky": [] }),
            ),
            event(
                "2026-10-15T10:01:00Z",
                run,
                "decision_point",
                json!({ "id": format!("{run}-4"), "kind": "failure_kind", "affects": "conclusion",
                        "item": "modules", "test": "T/d()" }),
            ),
            event(
                "2026-10-15T10:01:00Z",
                run,
                "run_end",
                json!({ "label": "run", "ok": false, "passed": 0, "total": 1,
                        "duration_ms": 60000, "decisions_pending": 4 }),
            ),
            decision(&format!("{run}-1"), "T/a()", "product"),
            decision(&format!("{run}-2"), "T/b()", "flaky"),
            decision(&format!("{run}-3"), "T/c()", "environment"),
            event(
                "2026-10-15T10:10:00Z",
                later,
                "item_start",
                json!({ "item": "modules", "kind": "scenario", "device": "Krill-Verify-1" }),
            ),
            event(
                "2026-10-15T10:59:00Z",
                later,
                "item_end",
                json!({ "item": "modules", "kind": "scenario", "passed": false,
                        "duration_ms": 60000, "tests": { "failed": 2, "passed": 3 }, "flaky": [] }),
            ),
            event(
                "2026-10-15T10:59:00Z",
                later,
                "run_end",
                json!({ "label": "run", "ok": false, "passed": 0, "total": 1, "duration_ms": 60000 }),
            ),
            reconciled(&format!("{run}-1"), "T/a()", "product", "failed", true),
            reconciled(&format!("{run}-2"), "T/b()", "flaky", "failed", false),
        ],
    );
    let range = [
        "--project",
        "Krill",
        "--since",
        "2026-10-15T09:00:00Z",
        "--until",
        "2026-10-15T12:00:00Z",
    ];
    let v = home.json(&range);
    let kind = find(&v["accuracy"], "kind", "failure_kind");
    assert_eq!(kind["decided"], 3);
    assert_eq!(kind["reconciled"], 2);
    assert_eq!(kind["supported"], 1);
    assert_eq!(kind["rate"], 0.5);
    assert_eq!(kind["pending"], 1);
    assert_eq!(find(&kind["choices"], "choice", "flaky")["supported"], 0);
    assert_eq!(find(&kind["choices"], "choice", "product")["rate"], 1.0);

    let wrong = v["usage"]["wrong_decisions"].as_array().unwrap();
    assert_eq!(wrong.len(), 1);
    assert_eq!(wrong[0]["id"], format!("{run}-2"));
    assert_eq!(wrong[0]["later_outcome"], "failed");
    assert_eq!(
        find(&v["usage"]["detours"], "kind", "undecided")["count"],
        1
    );

    let mut args = vec!["report"];
    args.extend(range);
    let (code, out) = home.kr(&args);
    assert_eq!(code, Some(0), "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn every_invocation_is_logged_and_bad_arguments_are_counted() {
    let home = Home::empty();
    let (code, _) = home.kr(&["run", "--no-such-flag"]);
    assert_eq!(code, Some(2));
    let (code, _) = home.kr(&["skill"]);
    assert_eq!(code, Some(0));
    let (code, _) = home.kr(&["--help"]);
    assert_eq!(code, Some(0));

    let logged = home.invocations();
    assert_eq!(logged.len(), 2, "help is not logged: {logged:#?}");
    let bad = &logged[0];
    assert_eq!(bad["type"], "invocation");
    assert_eq!(bad["fields"]["command"], "run");
    assert_eq!(bad["fields"]["args"], json!(["run", "--no-such-flag"]));
    assert_eq!(bad["fields"]["exit_code"], 2);
    assert!(
        bad["fields"]["error"]
            .as_str()
            .unwrap()
            .contains("--no-such-flag"),
        "{bad:#}"
    );
    assert_eq!(bad["fields"]["requester"]["agent"], "test-agent");
    assert_eq!(logged[1]["fields"]["exit_code"], 0);

    let v = home.json(&["--since", "1h"]);
    let usage = &v["usage"];
    assert_eq!(usage["source"], "invocations");
    let run = find(&usage["commands"], "command", "run");
    assert_eq!(run["calls"], 1);
    assert_eq!(run["bad_arguments"], 1);
    assert_eq!(find(&usage["commands"], "command", "skill")["calls"], 1);
    assert_eq!(usage["bad_arguments"].as_array().unwrap().len(), 1);
    assert_eq!(find(&usage["detours"], "kind", "bad_arguments")["count"], 1);
}

#[test]
fn a_worktree_filter_keeps_its_runs_and_says_how_many_had_no_requester() {
    let home = Home::krill();
    let run = "01a14000-0000-7000-8000-0000000000aa";
    let other = "01a14000-0000-7000-8000-0000000000bb";
    let start = |id: &str, worktree: &str| {
        event(
            "2026-10-14T12:00:00Z",
            id,
            "run_start",
            json!({ "label": "run", "targets": ["smoke"],
                    "requester": { "agent": "codex", "worktree": worktree, "parent_pid": 1 } }),
        )
    };
    let end = |id: &str| {
        event(
            "2026-10-14T12:10:00Z",
            id,
            "run_end",
            json!({ "label": "run", "ok": true, "passed": 1, "total": 1, "duration_ms": 600000 }),
        )
    };
    home.add(
        "events",
        "2026-10-14",
        &[
            start(run, "/work/krill-133"),
            end(run),
            start(other, "/work/krill-125"),
            end(other),
        ],
    );
    let mut args = vec!["--project", "Krill", "--worktree", "/work/krill-133"];
    args.extend(BATCH_8);
    let v = home.json(&args);
    assert_eq!(v["time"]["runs"], 1);
    assert_eq!(v["runs"][0]["run_id"], run);
    assert_eq!(v["batch"]["runs_without_requester"], 88);
}

#[test]
fn an_empty_range_says_so_and_how_to_widen_it() {
    let home = Home::krill();
    let (code, out) = home.kr(&[
        "report",
        "--since",
        "2026-10-13T00:00:00Z",
        "--until",
        "2026-10-13T01:00:00Z",
    ]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.starts_with("no runs"), "{out}");
    assert!(out.contains("--since"), "{out}");
}

#[test]
fn a_time_kr_cannot_read_is_a_bad_argument() {
    let home = Home::empty();
    let (code, out) = home.kr(&["report", "--since", "yesterday"]);
    assert_eq!(code, Some(2), "{out}");
    assert!(out.contains("2026-10-14T09:40:00Z"), "{out}");
}

#[test]
fn a_passed_item_run_again_on_the_same_inputs_is_a_detour() {
    let home = Home::empty();
    let item_end = |ts: &str, run: &str, hash: &str| {
        event(
            ts,
            run,
            "item_end",
            json!({ "item": "smoke", "kind": "scenario", "passed": true, "duration_ms": 1000,
                    "tests": { "passed": 10 }, "flaky": [],
                    "identity": { "input_hash": hash } }),
        )
    };
    home.add(
        "events",
        "2026-10-15",
        &[
            item_end(
                "2026-10-15T10:00:00Z",
                "01a14000-0000-7000-8000-0000000000c1",
                "h1",
            ),
            item_end(
                "2026-10-15T10:10:00Z",
                "01a14000-0000-7000-8000-0000000000c2",
                "h1",
            ),
            item_end(
                "2026-10-15T10:20:00Z",
                "01a14000-0000-7000-8000-0000000000c3",
                "h2",
            ),
        ],
    );
    let v = home.json(&[
        "--project",
        "Krill",
        "--since",
        "2026-10-15T09:00:00Z",
        "--until",
        "2026-10-15T11:00:00Z",
    ]);
    let repeat = find(&v["usage"]["detours"], "kind", "repeat_after_pass");
    assert_eq!(repeat["count"], 1, "{repeat:#}");
    assert_eq!(
        repeat["examples"][0]["run_id"],
        "01a14000-0000-7000-8000-0000000000c2"
    );
}
