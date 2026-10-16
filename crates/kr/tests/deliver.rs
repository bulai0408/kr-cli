//! `kr deliver`: the three-part delivery report draft of a run, from its events and logs.
//! The main fixture is a real Krill run (#125, tests/fixtures/deliver/krill-125) with
//! the real `git diff` of that ticket.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

const KRILL_RUN: &str = "01a13a1f-8950-75e5-8020-4988e20bfa16";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    /// A home with the state directory of the recorded Krill run.
    fn krill() -> Self {
        let home = Self::empty();
        copy_dir(
            &root().join("tests/fixtures/deliver/krill-125/state"),
            &home.state(),
        );
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

    /// Adds event lines to the day file of their date.
    fn add_events(&self, day: &str, lines: &[serde_json::Value]) {
        let path = self.state().join(format!("events/{day}.ndjson"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut text = std::fs::read_to_string(&path).unwrap_or_default();
        for l in lines {
            text.push_str(&l.to_string());
            text.push('\n');
        }
        std::fs::write(path, text).unwrap();
    }

    fn kr(&self, replay: &[PathBuf], args: &[&str]) -> (Option<i32>, String) {
        let files: Vec<String> = replay.iter().map(|p| p.display().to_string()).collect();
        let out = Command::cargo_bin("kr")
            .unwrap()
            .env_clear()
            .env("HOME", self.dir.path())
            .env("NO_COLOR", "1")
            .env("KR_REPLAY", files.join(":"))
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
}

fn krill_diff() -> Vec<PathBuf> {
    vec![root().join("tests/fixtures/deliver/krill-125/git-diff.json")]
}

fn event(ts: &str, run: &str, kind: &str, fields: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "ts": ts, "run_id": run, "project": "Krill", "type": kind, "fields": fields })
}

#[test]
fn a_real_krill_run_drafts_three_parts() {
    let home = Home::krill();
    let (code, out) = home.kr(&krill_diff(), &["deliver", "01a13a1f", "--base", "419120c"]);
    assert_eq!(code, Some(0), "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn the_json_draft_lists_the_todos_it_can_see() {
    let home = Home::krill();
    let (code, out) = home.kr(
        &krill_diff(),
        &["deliver", KRILL_RUN, "--base", "419120c", "--json"],
    );
    assert_eq!(code, Some(0), "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["run_id"], KRILL_RUN);
    assert_eq!(v["machine"]["passed"], 7);
    assert_eq!(v["machine"]["total"], 9);

    let shelf_code = v["machine"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "shelf-code")
        .unwrap();
    assert_eq!(shelf_code["result"], "fail");
    assert_eq!(shelf_code["failed_tests"].as_array().unwrap().len(), 3);
    assert_eq!(
        shelf_code["xcresults"][0]["path"],
        "/Users/me/Krill/v0-125-shelf-code/build/kr/results/shelf-code/run.xcresult"
    );
    assert_eq!(shelf_code["xcresults"][0]["exists"], false);

    let kinds = |part: &str| -> Vec<String> {
        v[part]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["kind"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(kinds("human"), ["artboard"]);
    assert_eq!(
        v["human"][0]["files"][0],
        "docs/artboards/shelf-share-code.html"
    );
    assert_eq!(
        kinds("unverified"),
        ["pending_deploy", "owner_review", "exit_code_only"]
    );
    assert_eq!(
        v["unverified"][0]["files"],
        serde_json::json!(["backend/shelf-migrations/0001_shelf_shares.sql"])
    );
    assert_eq!(v["unverified"][2]["item"], "backend");
}

#[test]
fn without_a_base_the_draft_says_changed_files_were_not_checked() {
    let home = Home::krill();
    let (code, out) = home.kr(&[], &["deliver", "--json"]);
    assert_eq!(code, Some(0), "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let todo = v["unverified"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == "changes_not_checked")
        .expect("a changes_not_checked entry");
    assert!(todo["note"].as_str().unwrap().contains("--base"), "{todo}");
}

#[test]
fn the_latest_run_with_results_is_the_default() {
    let home = Home::krill();
    // A later `kr check`: steps only, no scenario or suite results.
    let later = "01a13a40-0000-7000-8000-000000000000";
    home.add_events(
        "2026-10-14",
        &[event(
            "2026-10-14T12:00:00Z",
            later,
            "step_end",
            serde_json::json!({ "command": "check", "step": "generate", "ok": true, "duration_ms": 300 }),
        )],
    );
    let (code, out) = home.kr(&[], &["deliver", "--json"]);
    assert_eq!(code, Some(0), "{out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["run_id"], KRILL_RUN);
}

#[test]
fn an_item_that_started_but_never_ended_is_not_verified() {
    let home = Home::empty();
    let run = "01a14000-0000-7000-8000-000000000001";
    home.add_events(
        "2026-10-15",
        &[
            event(
                "2026-10-15T01:00:00Z",
                run,
                "item_start",
                serde_json::json!({ "item": "launch", "kind": "scenario", "device": "Krill-Verify-1" }),
            ),
            event(
                "2026-10-15T01:00:01Z",
                run,
                "item_end",
                serde_json::json!({ "item": "unit", "kind": "suite", "passed": true, "duration_ms": 1000, "device": null, "tests": { "passed": 2, "failed": 0, "flaky": 0, "skipped": 0 }, "flaky": [] }),
            ),
        ],
    );
    let (code, out) = home.kr(&[], &["deliver"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("## 1. Decided by machine: 1/1 PASS"), "{out}");
    assert!(
        out.contains(
            "- launch: started on Krill-Verify-1 but has no result; the run stopped before it ended"
        ),
        "{out}"
    );
}

#[test]
fn an_unverified_result_is_listed_with_its_reason() {
    let home = Home::empty();
    let run = "01a14000-0000-7000-8000-000000000002";
    home.add_events(
        "2026-10-15",
        &[event(
            "2026-10-15T01:00:00Z",
            run,
            "item_end",
            serde_json::json!({
                "item": "ipad-layout", "kind": "scenario", "passed": false, "duration_ms": 1000,
                "device": "iPad mini", "flaky": [],
                "tests": { "passed": 3, "failed": 0, "flaky": 0, "skipped": 0, "unverified": 7 },
                "unverified_reason": "the device disconnected during the run",
            }),
        )],
    );
    let (code, out) = home.kr(&[], &["deliver"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("## 1. Decided by machine: 0/1 PASS"), "{out}");
    assert!(out.contains("- UNVERIFIED ipad-layout ("), "{out}");
    assert!(
        out.contains("- ipad-layout: 7 tests unverified: the device disconnected during the run"),
        "{out}"
    );
}

#[test]
fn a_filter_that_selected_nothing_is_not_verified() {
    let home = Home::empty();
    let run = "01a14000-0000-7000-8000-000000000003";
    home.add_events(
        "2026-10-15",
        &[
            event(
                "2026-10-15T01:00:00Z",
                run,
                "filter_mismatch",
                serde_json::json!({ "item": "search", "filter": "KrillUITests/SearchUITests" }),
            ),
            event(
                "2026-10-15T01:00:01Z",
                run,
                "item_end",
                serde_json::json!({ "item": "search", "kind": "scenario", "passed": true, "duration_ms": 1000, "device": "Krill-Verify-1", "tests": { "passed": 1, "failed": 0, "flaky": 0, "skipped": 0 }, "flaky": [] }),
            ),
        ],
    );
    let (_, out) = home.kr(&[], &["deliver"]);
    assert!(
        out.contains("- search: filter `KrillUITests/SearchUITests` selects no test on the device"),
        "{out}"
    );
}

#[test]
fn an_unknown_run_says_where_runs_are_kept() {
    let home = Home::krill();
    let (code, out) = home.kr(&[], &["deliver", "ffff"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.starts_with("error: no run `ffff` in the events under ") && out.contains("kr deliver"),
        "{out}"
    );
}
