//! `kr plan` on the sample project: what `kr run` / `kr gate` would do, without doing it.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use kr_exec::Recording;

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

fn recordings(name: &str) -> Vec<Recording> {
    let dir = root().join("tests/fixtures/run").join(name);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files
        .iter()
        .map(|f| kr_exec::read_recording(f).unwrap())
        .collect()
}

/// Only what `kr plan` runs: `git ls-files` for the input hashes.
fn git_only() -> Vec<Recording> {
    recordings("gate-default")
        .into_iter()
        .filter(|r| r.program == "git")
        .collect()
}

struct Sample {
    home: tempfile::TempDir,
}

impl Sample {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let config = std::fs::read_to_string(root().join("tests/sample/.kr.toml")).unwrap()
            + "\n[gates.empty]\nsuites = []\nscenarios = []\n";
        std::fs::write(repo.join(".kr.toml"), config).unwrap();
        copy_dir(
            &root().join("tests/fixtures/xcode/products"),
            &repo.join("build/kr/DerivedData/Build/Products"),
        );
        Self { home }
    }

    fn kr(&self, recordings: &[Recording], args: &[&str]) -> (Option<i32>, String) {
        let dir = self.home.path().join(format!(
            "replay-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<String> = recordings
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let p = dir.join(format!("{i:03}.json"));
                std::fs::write(&p, serde_json::to_string(r).unwrap()).unwrap();
                p.display().to_string()
            })
            .collect();
        let out = Command::cargo_bin("kr")
            .unwrap()
            .env_clear()
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .env("KR_REPLAY", files.join(":"))
            .env("KR_APPLICATIONS", root().join("tests/fixtures/xcode/apps"))
            .env("KR_RESOURCES__BUILD", "1")
            .env("KR_RESOURCES__SIM", "2")
            .current_dir(self.home.path().join("repo"))
            .args(args)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.code(), text)
    }

    fn json(&self, recordings: &[Recording], args: &[&str], code: i32) -> serde_json::Value {
        let mut all = vec!["--json", "plan"];
        all.extend(args);
        let (got, out) = self.kr(recordings, &all);
        assert_eq!(got, Some(code), "{out}");
        serde_json::from_str(&out).unwrap()
    }

    /// Runs the default gate from the recordings: builds, and leaves history.
    fn ran_the_gate(&self) {
        std::fs::write(self.home.path().join("repo/App.swift"), "let x = 1").unwrap();
        let (code, out) = self.kr(&recordings("gate-default"), &["gate"]);
        assert_eq!(code, Some(0), "{out}");
    }

    fn events(&self) -> usize {
        let dir = self.home.path().join(".local/state/kr/events");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .map(|e| {
                std::fs::read_to_string(e.unwrap().path())
                    .unwrap()
                    .lines()
                    .count()
            })
            .sum()
    }
}

#[test]
fn a_fresh_checkout_needs_its_build_and_has_work_to_do() {
    let s = Sample::new();
    let v = s.json(&git_only(), &[], 0);
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["label"], "gate default");
    assert_eq!(v["has_work"], true);
    let build = v["build"].as_array().unwrap();
    assert_eq!(build.len(), 1);
    assert_eq!(build[0]["cached"], false);
    assert_eq!(build[0]["why"], "never built with kr here");
    let items = v["items"].as_array().unwrap();
    let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["unit", "broken", "launch", "settings"]);
    assert!(items.iter().all(|i| i["why"] == "in gate `default`"), "{v}");
    assert!(items.iter().all(|i| i["estimate_ms"].is_null()), "{v}");
    assert!(v["estimate"]["total_ms"].is_null());
    let missing = v["estimate"]["without_history"].as_array().unwrap();
    assert_eq!(missing.len(), 5, "the build and the four items: {v}");
    // Nothing ran: no events, no slots.
    assert_eq!(s.events(), 0);
}

#[test]
fn after_a_run_the_build_is_cached_and_the_estimate_comes_from_history() {
    let s = Sample::new();
    s.ran_the_gate();
    let before = s.events();
    let v = s.json(&git_only(), &["launch", "settings"], 0);
    assert_eq!(v["label"], "run");
    assert_eq!(v["build"][0]["cached"], true, "{v}");
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["why"] == "named on the command line" && i["estimate_ms"].is_u64())
    );
    assert_eq!(v["sim_capacity"], 2);
    assert_eq!(v["lanes"], serde_json::json!(["default", "default"]));
    assert!(v["estimate"]["total_ms"].is_u64(), "{v}");
    assert_eq!(s.events(), before, "kr plan records nothing");
}

#[test]
fn a_changed_input_says_why_it_builds() {
    let s = Sample::new();
    s.ran_the_gate();
    // A file git lists (see the recorded `git ls-files`).
    let app = s.home.path().join("repo/App/KrSampleApp.swift");
    std::fs::create_dir_all(app.parent().unwrap()).unwrap();
    std::fs::write(app, "let x = 2").unwrap();
    let v = s.json(&git_only(), &["launch"], 0);
    assert_eq!(v["build"][0]["cached"], false);
    assert_eq!(v["build"][0]["why"], "inputs changed since the last build");
}

#[test]
fn with_exit_code_work_to_do_exits_3_and_nothing_to_do_exits_0() {
    // Like `git diff --exit-code`: only on request does the exit code say "work to do",
    // so `kr plan && kr run` still chains.
    let s = Sample::new();
    let v = s.json(&git_only(), &["--exit-code"], 3);
    assert_eq!(v["has_work"], true);
    let v = s.json(&git_only(), &["--gate", "empty", "--exit-code"], 0);
    assert_eq!(v["has_work"], false);
    let (code, out) = s.kr(&git_only(), &["plan", "--gate", "empty"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.starts_with("nothing to do: gate empty"), "{out}");
}

#[test]
fn the_human_plan_starts_with_the_conclusion() {
    let s = Sample::new();
    s.ran_the_gate();
    let (code, out) = s.kr(&git_only(), &["plan", "launch", "settings"]);
    assert_eq!(code, Some(0), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with(
            "work to do: run · build cached · 2 items on 2 lanes (default, default) · about "
        ),
        "{out}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("  launch (scenario, default): named on the command line, ~")),
        "{out}"
    );
    // Replayed durations are a few milliseconds; keep the snapshot independent of them.
    insta::with_settings!({ filters => vec![(r"(~|about )\d+s", "${1}<n>s")] }, {
        insta::assert_snapshot!(out);
    });
}

#[test]
fn an_unknown_gate_lists_the_gates() {
    let s = Sample::new();
    let (code, out) = s.kr(&git_only(), &["plan", "--gate", "nightly"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("no gate `nightly`"), "{out}");
}
