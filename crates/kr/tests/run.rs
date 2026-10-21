//! `kr gate` / `kr run` on the sample project, replaying real recorded runs
//! (tests/fixtures/run), one simulator lane.

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

struct Sample {
    home: tempfile::TempDir,
}

impl Sample {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::copy(root().join("tests/sample/.kr.toml"), repo.join(".kr.toml")).unwrap();
        copy_dir(
            &root().join("tests/fixtures/xcode/products"),
            &repo.join("build/kr/DerivedData/Build/Products"),
        );
        Self { home }
    }

    fn kr(&self, recordings: &[Recording], args: &[&str]) -> (Option<i32>, String) {
        self.kr_env(recordings, args, &[])
    }

    fn kr_env(
        &self,
        recordings: &[Recording],
        args: &[&str],
        env: &[(&str, &str)],
    ) -> (Option<i32>, String) {
        let dir = self.home.path().join(format!("replay-{}", rand_suffix()));
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
            .env("KR_RESOURCES__SIM", "1")
            .envs(env.iter().copied())
            .current_dir(self.home.path().join("repo"))
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

    fn events(&self) -> Vec<serde_json::Value> {
        let dir = self.home.path().join(".local/state/kr/events");
        let mut all = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
            all.extend(
                text.lines()
                    .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()),
            );
        }
        all
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// PASS/FAIL lines without durations.
fn verdict_lines(out: &str) -> Vec<String> {
    out.lines()
        .map(|l| match l.rfind(", ") {
            Some(i) if l.starts_with("PASS") || l.starts_with("FAIL") => format!("{})", &l[..i]),
            _ => l.to_owned(),
        })
        .filter(|l| !l.starts_with("gate ") && !l.starts_with("run:"))
        .collect()
}

#[test]
fn the_default_gate_passes_with_a_flaky_test_reported() {
    let s = Sample::new();
    let (code, out) = s.kr(&recordings("gate-default"), &["gate"]);
    assert_eq!(code, Some(0), "{out}");
    insta::assert_snapshot!(verdict_lines(&out).join("\n"));
    let last = out.lines().last().unwrap();
    assert!(
        last.starts_with("gate default: 4/4 PASS · built in "),
        "{last}"
    );
}

#[test]
fn events_have_per_item_durations_and_the_rerun() {
    let s = Sample::new();
    s.kr(&recordings("gate-default"), &["gate"]);
    let events = s.events();
    let ended: Vec<&str> = events
        .iter()
        .filter(|e| e["type"] == "item_end")
        .map(|e| e["fields"]["item"].as_str().unwrap())
        .collect();
    assert_eq!(ended.len(), 4);
    assert!(
        events
            .iter()
            .filter(|e| e["type"] == "item_end")
            .all(|e| e["fields"]["duration_ms"].is_u64())
    );
    let reruns: Vec<_> = events.iter().filter(|e| e["type"] == "rerun").collect();
    assert_eq!(reruns.len(), 1);
    assert_eq!(
        reruns[0]["fields"]["test"],
        "KrSampleUITests/LaunchUITests/testFlakyOnce()"
    );
    assert_eq!(reruns[0]["fields"]["passed"], true);
}

#[test]
fn a_second_run_with_unchanged_inputs_skips_the_build() {
    let s = Sample::new();
    std::fs::write(s.home.path().join("repo/App.swift"), "let x = 1").unwrap();
    let (code, _) = s.kr(&recordings("gate-default"), &["gate"]);
    assert_eq!(code, Some(0));
    let (code, out) = s.kr(&recordings("gate-default"), &["gate"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(
        out.lines().last().unwrap().contains("build cached"),
        "{out}"
    );
    assert!(s.events().iter().any(|e| e["type"] == "build_cache_hit"));
}

/// The `broken` scenario's run (its test fails, and fails again on its rerun), with
/// the build from the gate recording: that run reused a build.
fn broken() -> Vec<Recording> {
    let mut recs: Vec<Recording> = recordings("gate-default")
        .into_iter()
        .filter(|r| {
            r.program == "git"
                || r.program == "sh"
                || r.args.first().is_some_and(|a| a == "build-for-testing")
        })
        .collect();
    recs.extend(recordings("broken"));
    recs
}

const BROKEN_TEST: &str = "KrSampleUITests/SettingsUITests/testFailsOnRequest()";

impl Sample {
    /// An earlier run in which `test` failed with `signature`: kr has seen that failure.
    fn seen_failure(&self, test: &str, signature: &str) {
        let dir = self.state().join("events");
        std::fs::create_dir_all(&dir).unwrap();
        let event = serde_json::json!({
            "ts": "2026-10-01T00:00:00Z",
            "run_id": "01999999-0000-7000-8000-000000000000",
            "project": "KrSample",
            "type": "item_end",
            "fields": { "item": "broken", "passed": false, "cases": [
                { "id": test, "outcome": "failed", "duration_s": 1.0, "signature": signature, "category": "assertion" },
            ]},
        });
        std::fs::write(dir.join("2026-10-01.ndjson"), format!("{event}\n")).unwrap();
    }
}

#[test]
fn a_test_that_fails_again_on_its_rerun_fails_the_run() {
    let s = Sample::new();
    // kr has seen this failure, so its rules decide: FAIL (ADR 0011 supplement).
    s.seen_failure(BROKEN_TEST, "failed - asked to fail");
    let (code, out) = s.kr(&broken(), &["run", "broken"]);
    assert_eq!(code, Some(1), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with("FAIL: broken (1 tests, KrSample-Verify-1, "),
        "{out}"
    );
    assert_eq!(
        lines[1],
        "  KrSampleUITests/SettingsUITests/testFailsOnRequest(): failed - asked to fail"
    );
    assert!(
        lines[2].starts_with("  xcresult: ")
            && lines[2].ends_with("build/kr/results/broken/run.xcresult"),
        "{out}"
    );
}

/// The recordings of one item, edited to show a false pass kr must catch.
fn edited(item_filter: &str, edit: impl Fn(&mut Recording)) -> Vec<Recording> {
    let mut recs = recordings("gate-default");
    for r in &mut recs {
        if r.args.iter().any(|a| a.contains(item_filter))
            || r.args
                .iter()
                .any(|a| a.contains("build/kr/results/settings/"))
        {
            edit(r);
        }
    }
    recs
}

#[test]
fn a_filter_that_selects_zero_tests_on_the_device_fails_even_with_exit_zero() {
    let recs = edited("SettingsUITests/testLaunchesTwice", |r| {
        if r.args.iter().any(|a| a == "-enumerate-tests") {
            r.stdout = r
                .stdout
                .replace(
                    "\"identifier\" : \"KrSampleUITests/SettingsUITests/testLaunchesTwice()\"",
                    "\"identifier\" : \"X\"",
                )
                .replace("enabledTests", "disabledTests_");
        }
    });
    let s = Sample::new();
    let (code, out) = s.kr(&recs, &["run", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.contains("FAIL: settings") && out.contains("the filters select 0 tests"),
        "{out}"
    );
}

#[test]
fn a_result_bundle_with_fewer_tests_than_selected_fails() {
    let recs = edited("SettingsUITests/testLaunchesTwice", |r| {
        if r.program == "xcrun" && r.args.iter().any(|a| a == "xcresulttool") {
            r.stdout = r
                .stdout
                .replace("\"nodeType\":\"Test Case\"", "\"nodeType\":\"Ignored\"");
        }
    });
    let s = Sample::new();
    let (code, out) = s.kr(&recs, &["run", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("ran 0 of 1 selected tests"), "{out}");
}

#[test]
fn an_unknown_gate_lists_the_gates() {
    let s = Sample::new();
    let (code, _) = s.kr(&[], &["gate", "nightly"]);
    assert_eq!(code, Some(1));
}

/// The recorded sysctl answer with a 1-minute load of `load`.
fn sysctl_with_load(load: f64) -> Recording {
    let mut r =
        kr_exec::read_recording(&root().join("tests/fixtures/sysctl/m3-imac.json")).unwrap();
    let start = r.stdout.find("vm.loadavg:").unwrap();
    let end = start + r.stdout[start..].find('\n').unwrap();
    r.stdout.replace_range(
        start..end,
        &format!("vm.loadavg: {{ {load:.2} {load:.2} {load:.2} }}"),
    );
    r
}

#[test]
fn command_reruns_preserve_the_first_log_and_record_both_paths() {
    for passed in [false, true] {
        let s = Sample::new();
        let config = s.home.path().join("repo/.kr.toml");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("\n[suites.backend]\ntype = \"command\"\nrun = \"printf 'backend failed\\\\n'; exit 1\"\n");
        std::fs::write(config, text).unwrap();
        let mut recs: Vec<_> = recordings("gate-default")
            .into_iter()
            .filter(|r| r.program == "git")
            .collect();
        recs.extend(vec![sysctl_with_load(50.0); 6]);
        let command = |status, stdout: &str| Recording {
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf 'backend failed\\n'; exit 1".to_owned(),
            ],
            status: Some(status),
            stdout: stdout.to_owned(),
            stderr: String::new(),
            ..tree()
        };
        recs.push(command(1, "backend failed\n"));
        recs.push(command(if passed { 0 } else { 1 }, "second attempt\n"));
        let (code, out) = s.kr(&recs, &["run", "backend"]);
        assert_eq!(code, Some(if passed { 0 } else { 1 }), "{out}");
        let events = s.events();
        let end = events.iter().find(|e| e["type"] == "item_end").unwrap();
        let first = PathBuf::from(end["fields"]["log"].as_str().unwrap());
        let again = PathBuf::from(end["fields"]["again_log"].as_str().unwrap());
        assert_eq!(first.file_name().unwrap(), "backend.log");
        assert_eq!(again.file_name().unwrap(), "backend.again.log");
        assert!(
            std::fs::read_to_string(&first)
                .unwrap()
                .contains("backend failed")
        );
        assert!(
            std::fs::read_to_string(&again)
                .unwrap()
                .contains("second attempt")
        );
        let rerun = events.iter().find(|e| e["type"] == "rerun").unwrap();
        assert_eq!(rerun["fields"]["log"], end["fields"]["log"]);
        assert_eq!(rerun["fields"]["again_log"], end["fields"]["again_log"]);
        assert!(out.contains(first.to_str().unwrap()), "{out}");
    }
}

#[test]
fn a_persistent_solo_failure_skips_the_whole_item_rerun_under_high_load() {
    let s = Sample::new();
    s.seen_failure(BROKEN_TEST, "failed - asked to fail");
    // Startup, then a reading before and after each of the build and sim slots.
    let mut recs = vec![sysctl_with_load(50.0); 5];
    recs.extend([
        sysctl_with_load(2.0),
        sysctl_with_load(2.0),
        sysctl_with_load(50.0),
    ]);
    recs.extend(broken());
    let (code, out) = s.kr(&recs, &["run", "broken"]);
    assert_eq!(code, Some(1), "{out}");
    let events = s.events();
    let whole: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "rerun" && e["fields"]["whole_item"] == true)
        .collect();
    assert_eq!(whole.len(), 1);
    assert_eq!(whole[0]["fields"]["skipped"], true);
    assert_eq!(
        whole[0]["fields"]["reason"],
        "persistent test failure after individual rerun under normal load"
    );
    let evidence = &whole[0]["fields"]["solo_rerun"];
    assert_eq!(evidence["test"], BROKEN_TEST);
    assert_eq!(evidence["load_state"], "normal");
    assert_eq!(evidence["load"]["start"]["load"]["one"], 2.0);
    assert_eq!(evidence["load"]["end"]["load"]["one"], 2.0);
    assert!(
        !s.home
            .path()
            .join("repo/build/kr/results/broken-again")
            .exists()
    );
}

#[test]
fn a_failed_solo_rerun_under_high_or_unknown_load_keeps_the_whole_item_rerun() {
    for (before, after, state) in [
        (Some(50.0), Some(50.0), "high"),
        (Some(2.0), Some(50.0), "high"),
        (Some(50.0), Some(2.0), "high"),
        (None, Some(2.0), "unknown"),
        (Some(2.0), None, "unknown"),
    ] {
        let s = Sample::new();
        s.seen_failure(BROKEN_TEST, "failed - asked to fail");
        // Startup, then a reading before and after each of the build and sim slots.
        let mut recs = vec![sysctl_with_load(50.0); 5];
        for load in [before, after] {
            let mut sample = sysctl_with_load(load.unwrap_or(2.0));
            if load.is_none() {
                sample.stdout.clear();
            }
            recs.push(sample);
        }
        recs.push(sysctl_with_load(50.0));
        recs.extend(broken());
        let (_, out) = s.kr(&recs, &["run", "broken"]);
        let events = s.events();
        let solo = events
            .iter()
            .find(|e| e["type"] == "rerun" && e["fields"]["test"] == BROKEN_TEST)
            .unwrap();
        assert_eq!(solo["fields"]["load_state"], state, "{out}");
        let whole = events
            .iter()
            .find(|e| e["type"] == "rerun" && e["fields"]["whole_item"] == true)
            .unwrap();
        assert_ne!(whole["fields"]["skipped"], true);
        assert!(
            s.home
                .path()
                .join("repo/build/kr/results/broken-again")
                .exists()
        );
    }
}

#[test]
fn a_timeout_with_successful_solo_reruns_still_reruns_the_whole_item() {
    let s = Sample::new();
    let mut recs = vec![sysctl_with_load(50.0); 8];
    recs.extend(broken().into_iter().map(|mut r| {
        if r.program == "xcodebuild" && r.args.iter().any(|a| a.ends_with("broken/run.xcresult")) {
            r.timed_out = true;
        }
        if r.program == "xcrun"
            && r.args
                .iter()
                .any(|a| a.ends_with("broken/rerun-1.xcresult"))
        {
            r.stdout = r.stdout.replace("\"Failed\"", "\"Passed\"");
        }
        r
    }));
    let (_, out) = s.kr(&recs, &["run", "broken"]);
    let events = s.events();
    assert!(
        events.iter().any(|e| e["type"] == "rerun"
            && e["fields"]["test"] == BROKEN_TEST
            && e["fields"]["passed"] == true),
        "{out}"
    );
    assert!(
        events.iter().any(|e| e["type"] == "rerun"
            && e["fields"]["whole_item"] == true
            && e["fields"]["skipped"] != true),
        "{out}"
    );
}

#[test]
fn a_failure_under_high_load_reruns_the_whole_item_and_counts_as_flaky() {
    let gate = recordings("gate-default");
    let mut recs: Vec<Recording> = gate
        .iter()
        .filter(|r| {
            r.program == "git"
                || r.program == "sh"
                || r.args.first().is_some_and(|a| a == "build-for-testing")
        })
        .cloned()
        .collect();
    // Load 50 on 8 cores is above 4 × 8: high for the whole run.
    recs.extend((0..6).map(|_| sysctl_with_load(50.0)));
    // No readable first result: there is no test to rerun individually.
    recs.extend(recordings("broken").into_iter().skip(1).map(|mut r| {
        if r.program == "xcrun" && r.args.iter().any(|a| a.ends_with("broken/run.xcresult")) {
            r.status = Some(1);
            r.stdout.clear();
        }
        r
    }));
    // Second round: the recorded passing run of `broken` from the gate, in broken-again.
    recs.extend(
        gate.iter()
            .filter(|r| {
                r.args
                    .iter()
                    .any(|a| a.contains("testFailsOnRequest") || a.contains("results/broken/"))
            })
            .map(|r| {
                let mut r = r.clone();
                for a in &mut r.args {
                    *a = a.replace("results/broken/", "results/broken-again/");
                }
                r
            }),
    );
    let s = Sample::new();
    let (code, out) = s.kr(&recs, &["run", "broken"]);
    assert_eq!(code, Some(0), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with("PASS: broken (1 tests, KrSample-Verify-1, "),
        "{out}"
    );
    assert!(
        lines[1].starts_with("  high load: 1-minute load 50.00"),
        "{out}"
    );
    assert_eq!(
        lines[2],
        "  flaky: broken (whole item: failed during high load (1-minute load 50.00 is above 32 (4 × 8 cores)); passed when run again)"
    );
    let end = s
        .events()
        .into_iter()
        .find(|e| e["type"] == "item_end")
        .unwrap();
    assert_eq!(end["fields"]["load"]["start"]["load"]["one"], 50.0);
    assert!(end["fields"]["high_load"].is_string());
}

#[test]
fn a_long_enumeration_output_is_read_whole_not_just_its_tail() {
    // Krill's enumeration of six test targets is about 90 KB, more than the in-memory tail.
    let recs: Vec<Recording> = recordings("gate-default")
        .into_iter()
        .map(|mut r| {
            if r.args.iter().any(|a| a == "-enumerate-tests") {
                // Thousands of entries make the JSON itself larger than the in-memory tail.
                let many: Vec<String> = (0..3000)
                    .map(|i| {
                        format!("{{ \"identifier\" : \"KrSampleTests/Filler/test{i:05}()\" }}")
                    })
                    .collect();
                r.stdout = r.stdout.replacen(
                    "\"testPlan\"",
                    &format!("\"padding\" : [{}], \"testPlan\"", many.join(",")),
                    1,
                );
                assert!(r.stdout.len() > 70_000);
            }
            r
        })
        .collect();
    let s = Sample::new();
    let (code, out) = s.kr(&recs, &["gate"]);
    assert_eq!(code, Some(0), "{out}");
}

// --- Run reliability (#25) ---

impl Sample {
    /// Starts kr in the background, for tests that act while it runs.
    fn spawn(&self, recordings: &[Recording], args: &[&str]) -> std::process::Child {
        let dir = self.home.path().join(format!("replay-{}", rand_suffix()));
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
        std::process::Command::new(assert_cmd::cargo::cargo_bin("kr"))
            .env_clear()
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .env("KR_REPLAY", files.join(":"))
            .env("KR_APPLICATIONS", root().join("tests/fixtures/xcode/apps"))
            .env("KR_RESOURCES__BUILD", "1")
            .env("KR_RESOURCES__SIM", "1")
            .current_dir(self.home.path().join("repo"))
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn event_count(&self) -> usize {
        if self.home.path().join(".local/state/kr/events").exists() {
            self.events().len()
        } else {
            0
        }
    }

    /// Waits (up to 20 s) for an event after the first `seen` that `pred` accepts.
    fn wait_for_event(&self, seen: usize, pred: impl Fn(&serde_json::Value) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if self.event_count() > seen && self.events()[seen..].iter().any(&pred) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the event did not come");
    }

    fn products(&self) -> PathBuf {
        self.home
            .path()
            .join("repo/build/kr/DerivedData/Build/Products")
    }

    fn pool(&self) -> kr_resource::Pool {
        kr_resource::Pool::new(self.home.path().join(".local/state/kr/resources"))
    }
}

fn wait_with_output(child: std::process::Child) -> (Option<i32>, String) {
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned()
        + &String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| l.starts_with("error"))
            .collect::<Vec<_>>()
            .join("\n");
    (out.status.code(), text)
}

fn holder() -> kr_resource::Holder {
    kr_resource::Holder {
        project: Some("other".into()),
        task: "test".into(),
        pid: std::process::id(),
        child_pid: None,
        started: jiff::Timestamp::now(),
    }
}

#[test]
fn an_item_that_errors_fails_alone_and_the_others_still_get_a_verdict() {
    // Krill #125: an error after the last item ended threw away every verdict.
    let recs: Vec<Recording> = recordings("gate-default")
        .into_iter()
        .filter(|r| {
            !r.args
                .iter()
                .any(|a| a == "build/kr/results/settings/run.xcresult")
        })
        .collect();
    let s = Sample::new();
    let (code, out) = s.kr(&recs, &["run", "launch", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("PASS: launch ("), "{out}");
    assert!(
        lines.iter().any(|l| l.starts_with("FAIL: settings")),
        "{out}"
    );
    assert!(
        out.contains("build/kr/results/settings/run.xcresult"),
        "the failure names the file: {out}"
    );
    assert!(lines.last().unwrap().starts_with("run: 1/2 PASS"), "{out}");
    let ended = s
        .events()
        .into_iter()
        .filter(|e| e["type"] == "item_end")
        .count();
    assert_eq!(ended, 2);
}

#[test]
fn two_lanes_of_one_device_kind_both_run_and_every_item_gets_a_verdict() {
    // Krill #125 ran two `default` lanes; the second lane's queue ticket collided with
    // the first one's and its error discarded the verdicts of the whole run.
    const UDID_1: &str = "31038384-DFE3-45BC-8984-78A3857E5A5F";
    const UDID_2: &str = "16704C23-1F75-488E-B084-76C5E1460276";
    let gate = recordings("gate-default");
    let mut recs = gate.clone();
    // The same recorded runs on the pool's second simulator.
    recs.extend(
        gate.iter()
            .filter(|r| r.args.iter().any(|a| a.contains(UDID_1)))
            .map(|r| {
                let mut r = r.clone();
                r.args = r.args.iter().map(|a| a.replace(UDID_1, UDID_2)).collect();
                r
            }),
    );
    let s = Sample::new();
    let out = Command::cargo_bin("kr")
        .unwrap()
        .env_clear()
        .env("HOME", s.home.path())
        .env("NO_COLOR", "1")
        .env("KR_REPLAY", {
            let dir = s.home.path().join("replay");
            std::fs::create_dir_all(&dir).unwrap();
            let files: Vec<String> = recs
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let p = dir.join(format!("{i:03}.json"));
                    std::fs::write(&p, serde_json::to_string(r).unwrap()).unwrap();
                    p.display().to_string()
                })
                .collect();
            files.join(":")
        })
        .env("KR_APPLICATIONS", root().join("tests/fixtures/xcode/apps"))
        .env("KR_RESOURCES__BUILD", "1")
        .env("KR_RESOURCES__SIM", "2")
        .current_dir(s.home.path().join("repo"))
        .args(["run", "launch", "settings", "broken"])
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("run: 3/3 PASS"), "{text}");
    let lanes = s
        .events()
        .into_iter()
        .filter(|e| e["type"] == "slot_acquired" && e["fields"]["resource"] == "sim")
        .count();
    assert_eq!(lanes, 2, "{text}");
}

#[test]
fn a_missing_result_bundle_fails_the_item_and_names_the_bundle() {
    let recs = edited("SettingsUITests/testLaunchesTwice", |r| {
        if r.program == "xcrun" && r.args.iter().any(|a| a == "xcresulttool") {
            r.status = Some(1);
            r.stdout = String::new();
            r.stderr = "Error: The file couldn't be opened because it doesn't exist.\n".into();
        }
    });
    let s = Sample::new();
    let (code, out) = s.kr(&recs, &["run", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.lines().any(|l| l.contains("no test results")
            && l.contains("build/kr/results/settings/run.xcresult")),
        "{out}"
    );
}

/// The `unit` suite's recordings with `greetsByName` renamed to `testGreetsByName`
/// (a Swift Testing function, as in Krill #125) and failing in the first run, plus
/// its rerun: enumeration, run and results, each made from the recorded run of `unit`.
fn swift_testing_failure() -> Vec<Recording> {
    const ID: &str = "KrSampleTests/GreetingTests/testGreetsByName()";
    const PASSED: &str = r#""nodeType":"Test Case","result":"Passed""#;
    const FAILED: &str = r#""nodeType":"Test Case","result":"Failed","children":[{"name":"Expectation failed","nodeType":"Failure Message"}]"#;
    let renamed: Vec<Recording> = recordings("gate-default")
        .into_iter()
        .map(|mut r| {
            r.stdout = r.stdout.replace("greetsByName", "testGreetsByName");
            r
        })
        .collect();
    let of_unit = |r: &Recording| {
        r.args
            .iter()
            .any(|a| a == "build/kr/results/unit/run.xcresult")
    };
    let find =
        |pred: &dyn Fn(&Recording) -> bool| renamed.iter().find(|r| pred(r)).unwrap().clone();
    let mut recs = renamed.clone();
    for r in recs
        .iter_mut()
        .filter(|r| r.program == "xcrun" && of_unit(r))
    {
        r.stdout = r.stdout.replacen(PASSED, FAILED, 1);
    }
    let rerun_path = |a: &String| a.replace("unit/run.xcresult", "unit/rerun-1.xcresult");

    let mut enumerate = find(&|r| {
        r.args.iter().any(|a| a == "-enumerate-tests")
            && r.args.iter().any(|a| a == "-only-testing:KrSampleTests")
    });
    *enumerate.args.last_mut().unwrap() = format!("-only-testing:{ID}");
    let other = "KrSampleTests/GreetingTests/greetsEmptyName()";
    let start = enumerate
        .stdout
        .find(&format!(
            ",\n        {{\n          \"identifier\" : \"{other}\""
        ))
        .unwrap();
    let end = enumerate.stdout[start..].find('}').unwrap() + start + 1;
    enumerate.stdout.replace_range(start..end, "");

    let mut run = find(&|r| r.program == "xcodebuild" && of_unit(r));
    run.args = run
        .args
        .iter()
        .map(|a| {
            if a == "-only-testing:KrSampleTests" {
                format!("-only-testing:{ID}")
            } else {
                rerun_path(a)
            }
        })
        .collect();

    let mut results = find(&|r| r.program == "xcrun" && of_unit(r));
    results.args = results.args.iter().map(rerun_path).collect();
    let start = results.stdout.find(r#",{"duration""#).unwrap();
    let end = results.stdout[start..]
        .find(r#"}],"name":"GreetingTests""#)
        .unwrap()
        + start
        + 1;
    results.stdout.replace_range(start..end, "");

    recs.extend([enumerate, run, results]);
    recs
}

#[test]
fn a_failed_swift_testing_test_named_test_is_rerun_with_its_parentheses() {
    let s = Sample::new();
    let (code, out) = s.kr(&swift_testing_failure(), &["run", "unit"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.starts_with("PASS: unit ("), "{out}");
    assert!(
        out.contains("  flaky: KrSampleTests/GreetingTests/testGreetsByName()"),
        "{out}"
    );
}

#[test]
fn a_rerun_filter_that_selects_no_test_is_reported_and_not_run() {
    let mut recs = swift_testing_failure();
    // The rerun's enumeration selects nothing; there is no recording of the rerun itself.
    let n = recs.len();
    recs[n - 3].stdout = recs[n - 3].stdout.replace("enabledTests", "disabledTests_");
    recs.truncate(n - 2);
    let s = Sample::new();
    let (code, out) = s.kr(&recs, &["run", "unit"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.starts_with("FAIL: unit ("), "{out}");
    assert!(
        out.contains(
            "rerun filter invalid: `-only-testing:KrSampleTests/GreetingTests/testGreetsByName()` selects 0 tests"
        ),
        "{out}"
    );
    let rerun = s
        .events()
        .into_iter()
        .find(|e| e["type"] == "rerun")
        .unwrap();
    assert_eq!(rerun["fields"]["filter_invalid"], true);
    assert_eq!(rerun["fields"]["passed"], false);
}

#[test]
fn a_sigterm_stops_the_run_like_sigint_and_starts_no_new_item() {
    // Krill: after SIGTERM, kr leased a simulator and started the next scenario.
    let recs: Vec<Recording> = recordings("gate-default")
        .into_iter()
        .map(|mut r| {
            if r.args
                .iter()
                .any(|a| a == "build/kr/results/launch/run.xcresult")
                && r.program == "xcodebuild"
            {
                r.duration_ms = Some(10_000);
            }
            r
        })
        .collect();
    let s = Sample::new();
    let child = s.spawn(&recs, &["run", "launch", "settings"]);
    s.wait_for_event(0, |e| {
        e["type"] == "item_start" && e["fields"]["item"] == "launch"
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    // SAFETY: plain kill(2) to the kr child.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let started = std::time::Instant::now();
    let (code, out) = wait_with_output(child);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "{out}"
    );
    assert_eq!(code, Some(143), "{out}");
    assert!(
        out.contains("ABORTED: run (SIGTERM) · interrupted: launch · not started: settings"),
        "{out}"
    );
    let events = s.events();
    assert!(
        !events
            .iter()
            .any(|e| e["type"] == "item_start" && e["fields"]["item"] == "settings")
    );
    let aborted = events
        .iter()
        .find(|e| e["type"] == "run_aborted")
        .expect("a run_aborted event");
    assert_eq!(aborted["fields"]["signal"], "SIGTERM");
    assert_eq!(
        aborted["fields"]["not_started"],
        serde_json::json!(["settings"])
    );
    assert!(events.iter().any(|e| e["type"] == "sim_released"));
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "slot_released" && e["fields"]["resource"] == "sim")
    );
    assert!(!s.pool().is_held(kr_resource::Resource::Sim, 1));
}

/// Runs `kr run settings` with a cached build while this test holds the only sim
/// slot, deletes the test products while kr waits, then lets kr go on. `restore`
/// puts the products back once kr notices, as its rebuild would.
fn run_with_products_deleted_while_waiting(restore: bool) -> (Sample, Option<i32>, String) {
    let s = Sample::new();
    std::fs::write(s.home.path().join("repo/App.swift"), "let x = 1").unwrap();
    let (code, out) = s.kr(&recordings("gate-default"), &["gate"]);
    assert_eq!(code, Some(0), "{out}");
    let slot = s
        .pool()
        .try_acquire(kr_resource::Resource::Sim, 1, &holder())
        .unwrap()
        .unwrap();
    let recs: Vec<Recording> = recordings("gate-default")
        .into_iter()
        .map(|mut r| {
            if r.args.first().is_some_and(|a| a == "build-for-testing") {
                r.duration_ms = Some(2_000);
            }
            r
        })
        .collect();
    let seen = s.event_count();
    let child = s.spawn(&recs, &["run", "settings"]);
    s.wait_for_event(seen, |e| {
        e["type"] == "queue_start" && e["fields"]["resource"] == "sim"
    });
    std::fs::remove_dir_all(s.products()).unwrap();
    drop(slot);
    if restore {
        s.wait_for_event(seen, |e| e["type"] == "products_missing");
        copy_dir(&root().join("tests/fixtures/xcode/products"), &s.products());
    }
    let (code, out) = wait_with_output(child);
    (s, code, out)
}

#[test]
fn test_products_deleted_while_waiting_for_a_simulator_are_rebuilt_once() {
    let (s, code, out) = run_with_products_deleted_while_waiting(true);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.starts_with("PASS: settings ("), "{out}");
    assert!(
        out.contains("  note: test products were missing before the item started; rebuilt them"),
        "{out}"
    );
    let events = s.events();
    let count = |kind: &str| events.iter().filter(|e| e["type"] == kind).count();
    assert_eq!(count("products_missing"), 1);
    let rebuilt = events
        .iter()
        .find(|e| e["type"] == "products_rebuilt")
        .unwrap();
    assert_eq!(rebuilt["fields"]["ok"], true);
}

#[test]
fn test_products_still_missing_after_the_rebuild_fail_the_item_with_the_next_step() {
    let (_, code, out) = run_with_products_deleted_while_waiting(false);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.starts_with("FAIL: settings"), "{out}");
    assert!(
        out.contains("test products are still missing after rebuilding them once"),
        "{out}"
    );
    assert!(out.contains("run `kr check`"), "{out}");
}

#[test]
fn every_run_ends_with_a_run_end_event_even_when_kr_itself_fails() {
    // #34: a run that ended with an error left no trace that it did not finish.
    let s = Sample::new();
    let (code, _) = s.kr(&recordings("gate-default"), &["gate"]);
    assert_eq!(code, Some(0));
    let end = s
        .events()
        .into_iter()
        .rfind(|e| e["type"] == "run_end")
        .expect("a run_end event");
    assert_eq!(end["fields"]["ok"], true);
    assert_eq!(end["fields"]["passed"], 4);
    assert_eq!(end["fields"]["total"], 4);

    // Without the simulator list kr cannot plan the lanes and stops with an error.
    let recs: Vec<Recording> = recordings("gate-default")
        .into_iter()
        .filter(|r| {
            !(r.program == "xcrun" && r.args.starts_with(&["simctl".into(), "list".into()]))
        })
        .collect();
    let (code, out) = s.kr(&recs, &["run", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    let end = s
        .events()
        .into_iter()
        .rfind(|e| e["type"] == "run_end")
        .unwrap();
    assert_eq!(end["fields"]["ok"], false);
    assert!(
        end["fields"]["error"]
            .as_str()
            .unwrap()
            .contains("simctl list"),
        "{end}"
    );
}

// --- Scheduling (#18) ---

const UDID_1: &str = "31038384-DFE3-45BC-8984-78A3857E5A5F";
const UDID_2: &str = "16704C23-1F75-488E-B084-76C5E1460276";

/// Makes the pool's second simulator an iPad, `KrSample-Verify-iPad-1`, that runs
/// `settings`: edits the sample's config and returns the gate recordings to match.
fn with_an_ipad(s: &Sample) -> Vec<Recording> {
    let config = s.home.path().join("repo/.kr.toml");
    let text = std::fs::read_to_string(&config).unwrap().replace(
        "[scenarios.settings]\n",
        "[scenarios.settings]\ndevice = \"iPad\"\n",
    ) + "\n[simulators.devices.iPad]\ndevice_type = \"iPad Pro 13-inch (M4)\"\n";
    std::fs::write(&config, text).unwrap();
    let gate = recordings("gate-default");
    let mut recs = Vec::new();
    for r in &gate {
        let mut r = r.clone();
        if r.args
            .starts_with(&["simctl".into(), "list".into(), "devices".into()])
        {
            let mut list: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
            for runtime in list["devices"].as_object_mut().unwrap().values_mut() {
                for d in runtime.as_array_mut().unwrap() {
                    if d["udid"] == UDID_2 {
                        d["name"] = "KrSample-Verify-iPad-1".into();
                        d["deviceTypeIdentifier"] =
                            "com.apple.CoreSimulator.SimDeviceType.iPad-Pro-13-inch-M4-8GB".into();
                    }
                }
            }
            r.stdout = serde_json::to_string(&list).unwrap();
        }
        let of_settings = r.args.iter().any(|a| {
            a.contains("SettingsUITests/testLaunchesTwice") || a.contains("results/settings/")
        });
        if of_settings {
            r.args = r.args.iter().map(|a| a.replace(UDID_1, UDID_2)).collect();
        }
        recs.push(r);
    }
    // The iPad lane boots and shuts down its own device.
    recs.extend(
        gate.iter()
            .filter(|r| r.program == "xcrun" && r.args.iter().any(|a| a == UDID_1))
            .filter(|r| r.args[1] == "shutdown" || r.args[1] == "bootstatus")
            .map(|r| {
                let mut r = r.clone();
                r.args = r.args.iter().map(|a| a.replace(UDID_1, UDID_2)).collect();
                r.stdout = r
                    .stdout
                    .replace("KrSample-Verify-1", "KrSample-Verify-iPad-1");
                r
            }),
    );
    recs
}

#[test]
fn ipad_lanes_take_their_simulator_slot_before_iphone_lanes() {
    // ADR 0010: which lane got a slot first was up to thread timing; iPad items are the
    // longest in Krill, so starting them first shortens the gate (41.3 → 37.6 min).
    let s = Sample::new();
    let recs = with_an_ipad(&s);
    let (code, out) = s.kr(&recs, &["run", "launch", "settings"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("run: 2/2 PASS"), "{out}");
    assert!(
        out.contains("PASS: settings (1 tests, KrSample-Verify-iPad-1, "),
        "{out}"
    );
    let events = s.events();
    let lanes: Vec<&str> = events
        .iter()
        .filter(|e| e["type"] == "slot_acquired" && e["fields"]["resource"] == "sim")
        .map(|e| e["fields"]["task"].as_str().unwrap())
        .collect();
    assert_eq!(lanes, ["run iPad lane", "run default lane"]);
    let started: Vec<&str> = events
        .iter()
        .filter(|e| e["type"] == "item_start")
        .map(|e| e["fields"]["item"].as_str().unwrap())
        .collect();
    assert_eq!(started, ["settings", "launch"]);
}

// --- Offline evaluation (#29, ADR 0010) ---

impl Sample {
    fn state(&self) -> PathBuf {
        self.home.path().join(".local/state/kr")
    }

    fn run_id(&self) -> String {
        self.events()
            .iter()
            .rfind(|e| e["type"] == "run_end")
            .expect("a finished run")["run_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn json(&self, recordings: &[Recording], args: &[&str]) -> serde_json::Value {
        let mut all = vec!["--json"];
        all.extend(args);
        let (code, out) = self.kr(recordings, &all);
        assert_eq!(code, Some(0), "{out}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }
}

#[test]
fn every_item_saves_its_parsed_results_and_rejudging_the_run_changes_nothing() {
    let s = Sample::new();
    s.kr(&recordings("gate-default"), &["gate"]);
    let run = s.state().join("runs").join(s.run_id());
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("launch/verdict.json")).unwrap())
            .unwrap();
    assert_eq!(saved["schema_version"], 1);
    assert_eq!(saved["item"], "launch");
    assert_eq!(
        saved["enumeration"]["enabled"].as_array().unwrap().len(),
        2,
        "{saved}"
    );
    assert_eq!(saved["reruns"].as_array().unwrap().len(), 1);
    let flaky = saved["verdict"]["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "KrSampleUITests/LaunchUITests/testFlakyOnce()")
        .unwrap();
    assert_eq!(flaky["outcome"], "flaky");

    let out = s.json(&[], &["rejudge"]);
    assert_eq!(out["schema_version"], 1);
    assert_eq!(out["runs"], 1);
    assert_eq!(out["items"], 4);
    assert_eq!(out["changed"], 0);
    assert_eq!(out["changes"], serde_json::json!([]));
    let (code, text) = s.kr(&[], &["rejudge"]);
    assert_eq!(code, Some(0));
    assert_eq!(text.trim(), "rejudge: 4 items in 1 run; no verdict changed");
}

#[test]
fn a_verdict_recorded_under_another_rule_is_listed_as_changed() {
    // As if the run had been judged by a rule that ignored the passing rerun.
    let s = Sample::new();
    s.kr(&recordings("gate-default"), &["gate"]);
    let file = s
        .state()
        .join("runs")
        .join(s.run_id())
        .join("launch/verdict.json");
    let text = std::fs::read_to_string(&file)
        .unwrap()
        .replace("\"flaky\"", "\"failed\"");
    std::fs::write(&file, text).unwrap();

    let out = s.json(&[], &["rejudge"]);
    assert_eq!(out["changed"], 1);
    let change = &out["changes"][0];
    assert_eq!(change["source"], "run");
    assert_eq!(change["item"], "launch");
    assert_eq!(change["before"]["passed"], false);
    assert_eq!(change["after"]["passed"], true);
    assert_eq!(
        change["tests"],
        serde_json::json!([{
            "id": "KrSampleUITests/LaunchUITests/testFlakyOnce()",
            "before": "failed",
            "after": "flaky",
        }])
    );
    let (_, text) = s.kr(&[], &["rejudge"]);
    let run_id = s.run_id();
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        [
            format!("CHANGED: {run_id} launch: FAIL → PASS").as_str(),
            "  KrSampleUITests/LaunchUITests/testFlakyOnce(): failed → flaky",
            "rejudge: 4 items in 1 run; 1 verdict changed",
        ]
    );
}

const TREE: &str = "3aa0c7f1e5b0d0c4b9a8e7f6d5c4b3a2f1e0d9c8";

/// `git rev-parse HEAD^{tree}` of the sample repo.
fn tree() -> Recording {
    Recording {
        program: "git".into(),
        args: vec!["rev-parse".into(), "HEAD^{tree}".into()],
        status: Some(0),
        stdout: format!("{TREE}\n"),
        stderr: String::new(),
        timed_out: false,
        duration_ms: None,
    }
}

fn event<'a>(events: &'a [serde_json::Value], kind: &str, item: &str) -> &'a serde_json::Value {
    &events
        .iter()
        .find(|e| e["type"] == kind && e["fields"]["item"] == item)
        .unwrap_or_else(|| panic!("no {kind} of {item}"))["fields"]
}

fn is_hash(v: &serde_json::Value) -> bool {
    v.as_str()
        .is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
}

#[test]
fn an_item_end_names_what_it_ran_on_and_keeps_each_test() {
    let s = Sample::new();
    let mut recs = recordings("gate-default");
    recs.push(tree());
    s.kr(&recs, &["gate"]);
    let events = s.events();
    let end = event(&events, "item_end", "launch");

    let identity = &end["identity"];
    assert!(is_hash(&identity["input_hash"]), "{identity}");
    assert_eq!(identity["tree"], TREE);
    assert_eq!(identity["xcode"]["version"], "27.1");
    assert_eq!(identity["runtime"], "iOS 18.6");
    assert_eq!(identity["device_type"], "iPhone 16 Pro");
    assert_eq!(identity["kr_version"], env!("CARGO_PKG_VERSION"));
    assert!(is_hash(&identity["config_hash"]), "{identity}");

    let cases = end["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2, "{end}");
    let flaky = cases
        .iter()
        .find(|c| c["id"] == "KrSampleUITests/LaunchUITests/testFlakyOnce()")
        .unwrap();
    assert_eq!(flaky["outcome"], "flaky");
    assert!(flaky["duration_s"].is_f64());
    assert!(flaky["signature"].is_string(), "{flaky}");
    assert!(flaky["category"].is_string(), "{flaky}");
    assert!(
        end["xcresult"]
            .as_str()
            .unwrap()
            .ends_with("build/kr/results/launch/run.xcresult"),
        "{end}"
    );

    let rerun = event(&events, "rerun", "launch");
    assert_eq!(rerun["attempt"], 2);
    assert!(
        rerun["xcresult"]
            .as_str()
            .unwrap()
            .ends_with("build/kr/results/launch/rerun-1.xcresult"),
        "{rerun}"
    );
}

#[test]
fn a_failed_test_has_a_signature_and_a_category() {
    let s = Sample::new();
    let mut recs: Vec<Recording> = recordings("gate-default")
        .into_iter()
        .filter(|r| {
            r.program == "git"
                || r.program == "sh"
                || r.args.first().is_some_and(|a| a == "build-for-testing")
        })
        .collect();
    recs.extend(recordings("broken"));
    s.kr(&recs, &["run", "broken"]);
    let events = s.events();
    assert_eq!(
        event(&events, "item_end", "broken")["cases"],
        serde_json::json!([{
            "id": "KrSampleUITests/SettingsUITests/testFailsOnRequest()",
            "outcome": "failed",
            "duration_s": event(&events, "item_end", "broken")["cases"][0]["duration_s"],
            "signature": "failed - asked to fail",
            "category": "assertion",
        }])
    );
}

#[test]
fn an_item_start_records_what_else_held_the_machine() {
    let s = Sample::new();
    s.kr(&recordings("gate-default"), &["gate"]);
    let events = s.events();
    // The run's own lane holds the only sim slot; its build slot is back.
    assert_eq!(
        event(&events, "item_start", "launch")["concurrency"],
        serde_json::json!({ "build": 0, "sim": 1, "legacy_holder": null })
    );
}

#[test]
fn the_run_start_names_the_requester_and_its_previous_run() {
    let s = Sample::new();
    let env = [("KR_REQUESTER", "worker-7")];
    s.kr_env(&recordings("gate-default"), &["gate"], &env);
    let first = s.run_id();
    s.kr_env(&recordings("gate-default"), &["gate"], &env);
    let starts: Vec<serde_json::Value> = s
        .events()
        .into_iter()
        .filter(|e| e["type"] == "run_start")
        .map(|e| e["fields"].clone())
        .collect();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0]["label"], "gate default");
    assert_eq!(starts[0]["requester"]["agent"], "worker-7");
    assert!(
        starts[0]["requester"]["worktree"]
            .as_str()
            .unwrap()
            .ends_with("/repo"),
        "{}",
        starts[0]
    );
    assert_eq!(starts[0]["previous"], serde_json::Value::Null);
    assert_eq!(starts[1]["previous"]["run_id"], first.as_str());
    assert!(
        starts[1]["previous"]["ended_ms_ago"].is_u64(),
        "{}",
        starts[1]
    );
}

#[test]
fn the_machine_load_is_sampled_while_the_run_lasts() {
    let s = Sample::new();
    s.kr(&recordings("gate-default"), &["gate"]);
    let run_id = s.run_id();
    let samples: Vec<serde_json::Value> = s
        .events()
        .into_iter()
        .filter(|e| e["type"] == "load_sample" && e["run_id"] == run_id.as_str())
        .collect();
    assert!(!samples.is_empty());
    let fields = &samples[0]["fields"];
    assert!(fields["load"]["one"].is_f64(), "{fields}");
    assert!(fields["pressure"].is_string(), "{fields}");
}

#[test]
fn each_pick_of_the_scheduler_is_recorded_with_its_candidates_and_estimates() {
    let s = Sample::new();
    let recs: Vec<Recording> = recordings("gate-default");
    s.kr(&recs, &["run", "launch", "settings"]);
    s.kr(&recs, &["run", "launch", "settings"]);
    let run_id = s.run_id();
    let events = s.events();
    let picks: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["type"] == "schedule_decision" && e["run_id"] == run_id.as_str())
        .map(|e| &e["fields"])
        .collect();
    assert_eq!(picks.len(), 2, "{picks:?}");
    assert_eq!(picks[0]["rule"], "longest_first");
    assert_eq!(picks[0]["kind"], "default");
    let candidates: Vec<&str> = picks[0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["item"].as_str().unwrap())
        .collect();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0], picks[0]["item"]);
    // The second run has the first one's durations as estimates.
    assert!(picks[0]["estimate_ms"].is_u64(), "{}", picks[0]);
    assert_eq!(picks[1]["candidates"].as_array().unwrap().len(), 1);
}

// --- Decision points (#20, ADR 0011, ADR 0015) ---

impl Sample {
    fn kinds<'a>(events: &'a [serde_json::Value], kind: &str) -> Vec<&'a serde_json::Value> {
        events
            .iter()
            .filter(|e| e["type"] == kind)
            .map(|e| &e["fields"])
            .collect()
    }

    /// `kr --json run broken` on a fresh history: it stops at one decision point.
    fn stopped_at_a_decision(&self) -> serde_json::Value {
        let (code, out) = self.kr(&broken(), &["--json", "run", "broken"]);
        assert_eq!(code, Some(4), "{out}");
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }
}

#[test]
fn an_unseen_failure_ends_the_run_without_a_verdict_and_with_a_decision_point() {
    let s = Sample::new();
    let out = s.stopped_at_a_decision();
    assert_eq!(out["schema_version"], 1);
    assert_eq!(out["ok"], false);
    assert_eq!(out["concluded"], false);
    let decisions = out["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 1, "{out}");
    let d = &decisions[0];
    let run_id = s.run_id();
    assert_eq!(d["id"], format!("{run_id}-1"));
    assert_eq!(d["kind"], "failure_kind");
    assert_eq!(d["affects"], "conclusion");
    assert_eq!(d["item"], "broken");
    assert_eq!(d["test"], BROKEN_TEST);
    let options: Vec<&str> = d["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(options, ["product", "flaky", "environment"]);
    assert_eq!(d["default"]["option"], "product");
    assert_eq!(d["evidence"]["signature"], "failed - asked to fail");
    assert_eq!(d["evidence"]["category"], "assertion");
    assert_eq!(d["evidence"]["history"]["runs"], 0);
    assert!(
        d["evidence"]["xcresult"]
            .as_str()
            .unwrap()
            .ends_with("build/kr/results/broken/run.xcresult"),
        "{d}"
    );
    assert_eq!(out["items"][0]["decisions"], serde_json::json!([d["id"]]));

    let events = s.events();
    assert_eq!(Sample::kinds(&events, "decision_point")[0]["id"], d["id"]);
    let end = Sample::kinds(&events, "run_end")[0];
    assert_eq!(end["ok"], false);
    assert_eq!(end["decisions_pending"], 1);
}

#[test]
fn the_human_report_puts_the_decision_point_in_its_own_section_with_the_next_command() {
    let s = Sample::new();
    let (code, out) = s.kr(&broken(), &["run", "broken"]);
    assert_eq!(code, Some(4), "{out}");
    let id = format!("{}-1", s.run_id());
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with("UNDECIDED: broken (1 tests, "),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "DECIDE {id}: What kind of failure is {BROKEN_TEST} in broken?"
        )),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "  next: kr decide {id} <product|flaky|environment> --reason \"…\""
        )),
        "{out}"
    );
    assert!(out.contains("  kr suggests product: "), "{out}");
    let last = lines.last().unwrap();
    assert!(
        last.starts_with("run: no verdict yet: 1 decision pending · "),
        "{last}"
    );
    assert!(!last.contains("PASS") && !last.contains("FAIL"), "{last}");
}

#[test]
fn a_decision_gives_the_verdict_without_running_again_and_a_later_run_reconciles_it() {
    let s = Sample::new();
    let id = s.stopped_at_a_decision()["decisions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run_id = s.run_id();

    // No recordings: kr decide runs nothing.
    let (code, out) = s.kr_env(
        &[],
        &[
            "decide",
            &id,
            "flaky",
            "--reason",
            "it passed in the last 20 runs",
        ],
        &[("KR_REQUESTER", "claude")],
    );
    assert_eq!(code, Some(0), "{out}");
    let lines = verdict_lines(&out);
    assert_eq!(
        lines[0],
        "PASS: broken (1 tests, 1 flaky, KrSample-Verify-1)"
    );
    assert_eq!(
        lines[1],
        format!(
            "  flaky: {BROKEN_TEST} (failed again when run on its own; the agent judged it flaky: it passed in the last 20 runs)"
        )
    );
    assert!(
        out.lines().last().unwrap().starts_with("run: 1/1 PASS · "),
        "{out}"
    );

    let events = s.events();
    let decision = Sample::kinds(&events, "decision")[0];
    assert_eq!(decision["id"], id.as_str());
    assert_eq!(decision["choice"], "flaky");
    assert_eq!(decision["reason"], "it passed in the last 20 runs");
    assert_eq!(decision["via"], "kr decide");
    assert_eq!(decision["by"]["agent"], "claude");
    assert_eq!(decision["evidence"]["signature"], "failed - asked to fail");
    let decided = events.iter().find(|e| e["type"] == "run_decided").unwrap();
    assert_eq!(decided["run_id"], run_id.as_str());
    assert_eq!(decided["fields"]["ok"], true);
    assert_eq!(decided["fields"]["decisions"], serde_json::json!([id]));

    // The same failure again on the same build inputs: kr has seen it now, so its
    // rules decide (FAIL), and the flaky judgment is not supported.
    let (code, out) = s.kr(&broken(), &["run", "broken"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.starts_with("FAIL: broken"), "{out}");
    let events = s.events();
    let reconciled = Sample::kinds(&events, "reconciliation");
    assert_eq!(reconciled.len(), 1, "{events:?}");
    let r = reconciled[0];
    assert_eq!(r["id"], id.as_str());
    assert_eq!(r["decided_run"], run_id.as_str());
    assert_eq!(r["choice"], "flaky");
    assert_eq!(r["later"]["outcome"], "failed");
    assert_eq!(r["later"]["run_id"], s.run_id().as_str());
    assert_eq!(r["supported"], false);

    // Reconciled once: a third run adds none.
    s.kr(&broken(), &["run", "broken"]);
    assert_eq!(Sample::kinds(&s.events(), "reconciliation").len(), 1);
}

#[test]
fn a_decision_handed_back_on_the_next_run_command_is_recorded_like_kr_decide() {
    let s = Sample::new();
    let id = s.stopped_at_a_decision()["decisions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let flag = format!("{id}=product");
    let (code, out) = s.kr(
        &[],
        &[
            "run",
            "--decide",
            &flag,
            "--reason",
            "asked to fail is a real bug",
        ],
    );
    assert_eq!(code, Some(1), "{out}");
    assert!(out.starts_with("FAIL: broken"), "{out}");
    let events = s.events();
    assert_eq!(
        Sample::kinds(&events, "decision")[0]["via"],
        "kr run --decide"
    );
    assert_eq!(Sample::kinds(&events, "run_decided")[0]["ok"], false);

    s.kr(&broken(), &["run", "broken"]);
    let r = Sample::kinds(&s.events(), "reconciliation")[0].clone();
    assert_eq!(r["choice"], "product");
    assert_eq!(r["supported"], true);
}

#[test]
fn an_environment_judgment_leaves_the_test_unverified_which_is_not_a_pass() {
    let s = Sample::new();
    let id = s.stopped_at_a_decision()["decisions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, out) = s.kr(
        &[],
        &[
            "--json",
            "decide",
            &id,
            "environment",
            "--reason",
            "the simulator hung",
        ],
    );
    assert_eq!(code, Some(1), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["concluded"], true);
    assert_eq!(out["decisions"], serde_json::json!([]));
    let item = &out["items"][0];
    assert_eq!(item["passed"], false);
    assert_eq!(item["tests"]["unverified"], 1);
    assert_eq!(item["tests"]["failed"], 0);
    assert_eq!(
        item["unverified_reason"],
        format!("the agent judged {BROKEN_TEST} an environment failure: the simulator hung")
    );
}

#[test]
fn a_wrong_decision_argument_exits_2_and_says_what_is_valid() {
    let s = Sample::new();
    let id = s.stopped_at_a_decision()["decisions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, out) = s.kr(&[], &["decide", &id, "maybe", "--reason", "x"]);
    assert_eq!(code, Some(2), "{out}");
    assert!(
        out.contains("`maybe` is not an option of")
            && out.contains("choose one of: product, flaky, environment"),
        "{out}"
    );
    let (code, out) = s.kr(&[], &["decide", "nonsense", "product", "--reason", "x"]);
    assert_eq!(code, Some(2), "{out}");
    assert!(out.contains("is not a decision ID"), "{out}");

    let (code, _) = s.kr(&[], &["decide", &id, "product", "--reason", "x"]);
    assert_eq!(code, Some(1));
    let (code, out) = s.kr(&[], &["--json", "decide", &id, "flaky", "--reason", "y"]);
    assert_eq!(code, Some(2), "{out}");
    let err: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(err["error"]["code"], "kr::bad_arguments");
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("is decided already: product"),
        "{err}"
    );
    // Only one decision event: the refused ones left no trace.
    assert_eq!(Sample::kinds(&s.events(), "decision").len(), 1);
}

#[test]
fn rejudge_shows_a_decision_point_with_its_decision_and_reconciliation() {
    let s = Sample::new();
    let id = s.stopped_at_a_decision()["decisions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let decided_run = s.run_id();
    s.kr(&[], &["decide", &id, "flaky", "--reason", "timing"]);
    s.kr(&broken(), &["run", "broken"]);
    let later_run = s.run_id();

    let out = s.json(&[], &["rejudge"]);
    assert_eq!(out["changed"], 0);
    // The second run's failure was seen by then: only the first run has a decision point.
    let points = out["decision_points"].as_array().unwrap();
    assert_eq!(points.len(), 1, "{out}");
    let p = &points[0];
    assert_eq!(p["run_id"], decided_run.as_str());
    assert_eq!(p["item"], "broken");
    assert_eq!(p["test"], BROKEN_TEST);
    assert_eq!(p["default"], "product");
    assert_eq!(p["id"], id.as_str());
    assert_eq!(p["decision"]["choice"], "flaky");
    assert_eq!(p["reconciliation"]["recorded"], true);
    assert_eq!(p["reconciliation"]["supported"], false);

    let (code, text) = s.kr(&[], &["rejudge"]);
    assert_eq!(code, Some(0));
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        [
            format!(
                "DECISION POINT: {decided_run} broken: {BROKEN_TEST}: unseen assertion failure; kr suggests product"
            )
            .as_str(),
            format!("  {id}: decided flaky via kr decide: timing").as_str(),
            format!("  reconciled by run {later_run}: failed → not supported").as_str(),
            "rejudge: 2 items in 2 runs; no verdict changed; 1 decision point (1 decided, 1 reconciled, 0 supported)",
        ]
    );
}

#[test]
fn rejudge_raises_the_decision_points_of_runs_from_before_them() {
    // A run of a kr without decision points: it judged the failure FAIL and recorded no
    // decision point.
    let s = Sample::new();
    s.stopped_at_a_decision();
    let dir = s.state().join("events");
    for e in std::fs::read_dir(&dir).unwrap() {
        let path = e.unwrap().path();
        let kept: String = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| !l.contains("\"type\":\"decision_point\""))
            .map(|l| format!("{l}\n"))
            .collect();
        std::fs::write(&path, kept).unwrap();
    }
    let out = s.json(&[], &["rejudge"]);
    let p = &out["decision_points"][0];
    assert_eq!(p["test"], BROKEN_TEST);
    assert_eq!(p["id"], serde_json::Value::Null);
    let (_, text) = s.kr(&[], &["rejudge"]);
    assert!(
        text.contains("  not raised when it ran (a kr without decision points): it was FAIL then"),
        "{text}"
    );
}

impl Sample {
    fn quarantine(&self, limit: usize, tests: serde_json::Value) {
        let dir = self.home.path().join("repo/.kr");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("quarantine.json"),
            serde_json::json!({
                "schema_version": 1, "limit": limit,
                "basis": "kr report: four individually flaky tests in recent Krill batches",
                "tests": tests,
            })
            .to_string(),
        )
        .unwrap();
    }
}

#[test]
fn a_quarantined_failure_runs_and_stays_failed_but_does_not_block_the_gate() {
    let s = Sample::new();
    s.quarantine(4, serde_json::json!([{
        "item": "broken", "test": BROKEN_TEST, "reason": "repeated flaky failure; fix tracked in PR", "added": "2026-10-16"
    }]));
    let config = s.home.path().join("repo/.kr.toml");
    let text = std::fs::read_to_string(&config).unwrap().replace(
        "[gates.smoke]\nscenarios = [\"launch\"]",
        "[gates.smoke]\nscenarios = [\"broken\"]",
    );
    std::fs::write(config, text).unwrap();
    let (code, out) = s.kr(&broken(), &["gate", "smoke", "--json"]);
    assert_eq!(code, Some(0), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    let r = out["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "broken")
        .unwrap();
    assert_eq!(r["passed"], false);
    assert_eq!(r["tests"]["failed"], 1);
    assert_eq!(r["tests"]["passed"], 0);
    assert_eq!(r["cases"][0]["outcome"], "failed");
    assert_eq!(r["quarantined"], serde_json::json!([BROKEN_TEST]));
    assert_eq!(r["blocking"], false);
    assert_eq!(out["ok"], true);
    assert!(out["decisions"].as_array().unwrap().is_empty());
    assert!(
        s.events()
            .iter()
            .any(|e| e["type"] == "rerun" && e["fields"]["test"] == BROKEN_TEST)
    );
}

#[test]
fn quarantine_over_budget_is_rejected_before_execution_with_a_pr_fix() {
    let s = Sample::new();
    s.quarantine(0, serde_json::json!([{"item":"broken","test":BROKEN_TEST,"reason":"repair","added":"2026-10-16"}]));
    let (code, out) = s.kr(&[], &["run", "broken", "--json"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("exceed quarantine limit 0"), "{out}");
    assert!(
        out.contains(".kr/quarantine.json") && out.contains("kr report") && out.contains("PR"),
        "{out}"
    );
}

#[test]
fn repeated_flakes_suggest_a_repository_edit_without_changing_the_list() {
    let s = Sample::new();
    let mut events = String::new();
    for n in 1..=2 {
        events.push_str(&format!("{}\n", serde_json::json!({
            "ts": jiff::Timestamp::now().to_string(), "run_id": format!("01999999-0000-7000-8000-{n:012}"),
            "project":"KrSample", "type":"item_end", "fields":{"item":"launch","passed":true,"flaky":["KrSampleUITests/LaunchUITests/testFlakyOnce()"]}
        })));
    }
    let dir = s.state().join("events");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("2026-10-16.ndjson"), events).unwrap();
    let (code, out) = s.kr(&recordings("gate-default"), &["run", "launch", "--json"]);
    assert_eq!(code, Some(0), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    let d = &out["decisions"][0];
    assert_eq!(d["kind"], "quarantine_add");
    assert_eq!(d["affects"], "efficiency");
    assert_eq!(d["evidence"]["flaky"], 3);
    assert_eq!(d["evidence"]["edit"]["append_to"], "tests");
    assert_eq!(
        d["evidence"]["edit"]["entry"]["test"],
        "KrSampleUITests/LaunchUITests/testFlakyOnce()"
    );
    assert!(!s.home.path().join("repo/.kr/quarantine.json").exists());
    assert!(s.events().iter().any(|e| e["type"] == "decision"
        && e["fields"]["kind"] == "quarantine_add"
        && e["fields"]["review"] == "pending"));
}

impl Sample {
    fn healthy_history(&self, count: usize, bad: Option<&str>) {
        let dir = self.state().join("events");
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = String::new();
        for n in 0..count {
            let outcome = if n == 0 {
                bad.unwrap_or("passed")
            } else {
                "passed"
            };
            text.push_str(&format!("{}\n", serde_json::json!({
                "ts":jiff::Timestamp::now().to_string(),"run_id":format!("01999999-0000-7000-8000-{n:012}"),"project":"KrSample","type":"item_end",
                "fields":{"item":"launch","passed": outcome == "passed", "flaky":if outcome == "flaky" {vec!["KrSampleUITests/LaunchUITests/testShowsGreeting()"]} else {vec![]},
                "cases":[{"id":"KrSampleUITests/LaunchUITests/testShowsGreeting()","outcome":outcome,"duration_s":1.0}]}
            })));
        }
        std::fs::write(dir.join("2026-10-16.ndjson"), text).unwrap();
        self.quarantine(4, serde_json::json!([{"item":"launch","test":"KrSampleUITests/LaunchUITests/testShowsGreeting()","reason":"repair","added":"2026-10-01","issue":"#35"}]));
    }
}

#[test]
fn ten_complete_clean_runs_suggest_removal_with_the_exact_entry() {
    let s = Sample::new();
    s.healthy_history(9, None);
    let file = s.home.path().join("repo/.kr/quarantine.json");
    let before = std::fs::read(&file).unwrap();
    let (code, out) = s.kr(&recordings("gate-default"), &["run", "launch", "--json"]);
    assert_eq!(code, Some(0), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["decisions"][0]["kind"], "quarantine_remove");
    assert_eq!(out["decisions"][0]["evidence"]["complete_runs"], 10);
    assert_eq!(
        out["decisions"][0]["evidence"]["edit"]["entry"]["issue"],
        "#35"
    );
    assert_eq!(before, std::fs::read(file).unwrap());
}

#[test]
fn nine_clean_runs_or_a_recent_failure_or_flake_do_not_suggest_removal() {
    for (n, bad) in [(8, None), (10, Some("failed")), (10, Some("flaky"))] {
        let s = Sample::new();
        s.healthy_history(n, bad);
        let (code, out) = s.kr(&recordings("gate-default"), &["run", "launch", "--json"]);
        assert_eq!(code, Some(0), "{out}");
        let out: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            out["decisions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|d| d["kind"] != "quarantine_remove"),
            "{out}"
        );
    }
}

#[test]
fn rejudge_applies_the_current_list_and_preserves_failed_test_outcomes() {
    let s = Sample::new();
    s.seen_failure(BROKEN_TEST, "failed - asked to fail");
    assert_eq!(s.kr(&broken(), &["run", "broken"]).0, Some(1));
    s.quarantine(4, serde_json::json!([{"item":"broken","test":BROKEN_TEST,"reason":"repair","added":"2026-10-16"}]));
    let (code, out) = s.kr(&[], &["rejudge", "--json"]);
    assert_eq!(code, Some(0), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["changed"], 1, "{out}");
    assert_eq!(out["changes"][0]["before"]["passed"], false);
    assert_eq!(out["changes"][0]["after"]["passed"], true);
    assert_eq!(out["changes"][0]["tests"], serde_json::json!([]));
    assert!(out["decision_points"].as_array().unwrap().is_empty());
}

#[test]
fn quarantine_never_unblocks_missing_results_timeouts_or_invalid_rerun_filters() {
    for mode in ["missing", "timeout", "invalid", "rerun_missing"] {
        let s = Sample::new();
        s.quarantine(4, serde_json::json!([{"item":"broken","test":BROKEN_TEST,"reason":"repair","added":"2026-10-16"}]));
        let mut recs = broken();
        for r in &mut recs {
            if mode == "missing"
                && r.program == "xcrun"
                && r.args.iter().any(|a| a.contains("broken/run.xcresult"))
            {
                r.status = Some(1);
                r.stdout.clear();
            }
            if mode == "timeout"
                && r.program == "xcodebuild"
                && r.args.iter().any(|a| a.contains("broken/run.xcresult"))
            {
                r.timed_out = true;
            }
            if mode == "invalid"
                && r.args
                    .iter()
                    .any(|a| a == &format!("-only-testing:{}", BROKEN_TEST.trim_end_matches("()")))
                && r.args.iter().any(|a| a == "-enumerate-tests")
            {
                r.stdout = r.stdout.replace("enabledTests", "disabledTests_");
            }
            if mode == "rerun_missing"
                && r.program == "xcrun"
                && r.args.iter().any(|a| a.contains("broken/rerun-1.xcresult"))
            {
                r.status = Some(1);
                r.stdout.clear();
            }
        }
        let (code, out) = s.kr(&recs, &["run", "broken", "--json"]);
        assert_eq!(code, Some(1), "{mode}: {out}");
        let out: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(out["ok"], false, "{mode}: {out}");
        assert_eq!(out["items"][0]["blocking"], true, "{mode}: {out}");
        let (code, rejudged) = s.kr(&[], &["rejudge", "--json"]);
        assert_eq!(code, Some(0), "{rejudged}");
        let rejudged: serde_json::Value = serde_json::from_str(&rejudged).unwrap();
        assert_eq!(rejudged["changed"], 0, "{mode}: {rejudged}");
    }
}

#[test]
fn quarantine_is_scoped_to_the_item_and_observed_entries_and_exits_are_events() {
    let s = Sample::new();
    s.seen_failure(BROKEN_TEST, "failed - asked to fail");
    s.quarantine(4, serde_json::json!([{"item":"launch","test":BROKEN_TEST,"reason":"repair","added":"2026-10-16"}]));
    assert_eq!(s.kr(&broken(), &["run", "broken"]).0, Some(1));
    s.quarantine(4, serde_json::json!([{"item":"broken","test":BROKEN_TEST,"reason":"repair","added":"2026-10-16"}]));
    let (code, out) = s.kr(&broken(), &["run", "broken"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.starts_with("QUARANTINED: broken"), "{out}");
    assert!(out.contains("0/1 PASS"), "{out}");
    let (_, out) = s.kr(&[], &["rejudge", "--json"]);
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["changed"], 1); // only the first run changes under the new list
    std::fs::remove_file(s.home.path().join("repo/.kr/quarantine.json")).unwrap();
    assert_eq!(s.kr(&broken(), &["run", "broken"]).0, Some(1));
    let events = s.events();
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "quarantine_enter")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "quarantine_exit")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "quarantine_observed")
            .count(),
        3
    );
    let (_, out) = s.kr(&[], &["rejudge", "--json"]);
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["changed"], 1); // the once-exempt run now blocks again
    assert_eq!(out["changes"][0]["before"]["passed"], true);
    assert_eq!(out["changes"][0]["after"]["passed"], false);
}

#[test]
fn a_quarantine_removal_can_be_reviewed_and_reconciled_without_kr_editing_the_file() {
    let s = Sample::new();
    s.healthy_history(9, None);
    let file = s.home.path().join("repo/.kr/quarantine.json");
    let before = std::fs::read(&file).unwrap();
    let out = s.json(&recordings("gate-default"), &["run", "launch"]);
    let id = out["decisions"][0]["id"].as_str().unwrap();
    let (code, out) = s.kr(
        &[],
        &["decide", id, "remove", "--reason", "reviewed; prepare PR"],
    );
    assert_eq!(code, Some(0), "{out}");
    assert_eq!(before, std::fs::read(&file).unwrap());
    s.quarantine(4, serde_json::json!([]));
    assert_eq!(
        s.kr(&recordings("gate-default"), &["run", "launch"]).0,
        Some(0)
    );
    let events = s.events();
    let r = events
        .iter()
        .find(|e| e["type"] == "reconciliation" && e["fields"]["id"] == id)
        .expect("the next observed list reconciles the review");
    assert_eq!(r["fields"]["kind"], "quarantine_remove");
    assert_eq!(r["fields"]["supported"], true);
}

#[test]
fn rejudge_does_not_apply_one_projects_quarantine_to_another_projects_saved_run() {
    let s = Sample::new();
    s.seen_failure(BROKEN_TEST, "failed - asked to fail");
    assert_eq!(s.kr(&broken(), &["run", "broken"]).0, Some(1));
    let config = s.home.path().join("repo/.kr.toml");
    std::fs::write(
        &config,
        std::fs::read_to_string(&config)
            .unwrap()
            .replace("name = \"KrSample\"", "name = \"AnotherProject\""),
    )
    .unwrap();
    s.quarantine(4, serde_json::json!([{"item":"broken","test":BROKEN_TEST,"reason":"repair","added":"2026-10-16"}]));
    let out = s.json(&[], &["rejudge"]);
    assert_eq!(out["changed"], 0, "{out}");
}

#[test]
fn an_unquarantined_failure_still_needs_a_decision_and_quarantined_failures_stay_failed_after_it() {
    const OTHER: &str = "KrSampleUITests/SettingsUITests/testOtherFailure()";
    let s = Sample::new();
    s.quarantine(4, serde_json::json!([{"item":"broken","test":BROKEN_TEST,"reason":"repair","added":"2026-10-16"}]));
    let mut recs = broken();
    for r in &mut recs {
        if r.args.iter().any(|a| a == "-enumerate-tests")
            && r.args.iter().any(|a| a.contains("testFailsOnRequest"))
        {
            let start = r.stdout.find("\n{").unwrap();
            let mut v: serde_json::Value = serde_json::from_str(&r.stdout[start..]).unwrap();
            v["values"][0]["enabledTests"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"identifier":OTHER}));
            r.stdout = v.to_string();
        }
        if r.program == "xcrun"
            && r.args
                .iter()
                .any(|a| a == "build/kr/results/broken/run.xcresult")
        {
            let mut v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
            let nodes = v["testNodes"][0]["children"][0]["children"][0]["children"]
                .as_array_mut()
                .unwrap();
            let mut other = nodes[0].clone();
            other["name"] = serde_json::json!("testOtherFailure()");
            other["nodeIdentifier"] = serde_json::json!("SettingsUITests/testOtherFailure()");
            other["nodeIdentifierURL"] = serde_json::json!(
                "test://com.apple.xcode/KrSample/KrSampleUITests/SettingsUITests/testOtherFailure"
            );
            nodes.push(other);
            r.stdout = v.to_string();
        }
    }
    let solo: Vec<_> = recs
        .iter()
        .filter(|r| r.args.iter().any(|a| a.contains("rerun-1.xcresult")))
        .cloned()
        .collect();
    for mut r in solo {
        r.args = r
            .args
            .iter()
            .map(|a| {
                a.replace("rerun-1.xcresult", "rerun-2.xcresult")
                    .replace("testFailsOnRequest", "testOtherFailure")
            })
            .collect();
        r.stdout = r.stdout.replace("testFailsOnRequest", "testOtherFailure");
        recs.push(r);
    }
    let mut enumerate = recs
        .iter()
        .find(|r| {
            r.args.iter().any(|a| a == "-enumerate-tests")
                && r.args.iter().any(|a| a.contains("testFailsOnRequest"))
        })
        .unwrap()
        .clone();
    enumerate.args = enumerate
        .args
        .iter()
        .map(|a| a.replace("testFailsOnRequest", "testOtherFailure"))
        .collect();
    let mut v: serde_json::Value = serde_json::from_str(&enumerate.stdout).unwrap();
    v["values"][0]["enabledTests"] = serde_json::json!([{"identifier":OTHER}]);
    enumerate.stdout = v.to_string();
    recs.push(enumerate);
    let (code, out) = s.kr(&recs, &["run", "broken", "--json"]);
    assert_eq!(code, Some(4), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["decisions"].as_array().unwrap().len(), 1);
    assert_eq!(out["decisions"][0]["test"], OTHER);
    let id = out["decisions"][0]["id"].as_str().unwrap();
    let (code, out) = s.kr(
        &[],
        &[
            "decide",
            id,
            "flaky",
            "--reason",
            "reviewed evidence",
            "--json",
        ],
    );
    assert_eq!(code, Some(0), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["items"][0]["passed"], false);
    assert_eq!(out["items"][0]["blocking"], false);
    assert_eq!(out["items"][0]["tests"]["failed"], 1);
    assert_eq!(out["items"][0]["tests"]["flaky"], 1);
}

#[test]
fn quarantine_counts_flakes_by_run_start_like_report_and_excludes_whole_item_flakes() {
    let s = Sample::new();
    let dir = s.state().join("events");
    std::fs::create_dir_all(&dir).unwrap();
    let mut starts = String::new();
    let mut ends = String::new();
    for n in 1..=2 {
        let run = format!("01999999-0000-7000-8000-{n:012}");
        starts.push_str(&format!("{}\n", serde_json::json!({"ts":(jiff::Timestamp::now()-jiff::SignedDuration::from_hours(91*24)).to_string(),"run_id":run,"project":"KrSample","type":"run_start","fields":{"label":"run"}})));
        ends.push_str(&format!("{}\n", serde_json::json!({"ts":jiff::Timestamp::now().to_string(),"run_id":run,"project":"KrSample","type":"item_end","fields":{"item":"launch","passed":true,"flaky":["KrSampleUITests/LaunchUITests/testFlakyOnce()","launch (whole item: high load; passed when run again)"]}})));
    }
    std::fs::write(dir.join("2026-01-01.ndjson"), starts).unwrap();
    std::fs::write(dir.join("2026-10-16.ndjson"), ends).unwrap();
    let out = s.json(&recordings("gate-default"), &["run", "launch"]);
    assert!(out["decisions"].as_array().unwrap().is_empty(), "{out}");
}

#[test]
fn preparation_success_is_reported_and_cleaned_before_the_item_ends() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(
        "\n[[scenarios.settings.prepare]]\nrun = \"echo prepared\"\ncleanup = \"echo cleaned\"\n",
    );
    std::fs::write(path, text).unwrap();
    let mut recs = recordings("gate-default");
    recs.push(shell_recording("echo prepared", 0));
    recs.push(shell_recording("echo cleaned", 0));
    let (code, out) = s.kr(&recs, &["--json", "run", "settings"]);
    assert_eq!(code, Some(0), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["items"][0]["preparation"][0]["status"], "ready");
    assert_eq!(out["items"][0]["preparation"][0]["cleanup"], "passed");
    let end = s
        .events()
        .into_iter()
        .find(|e| e["type"] == "item_end")
        .unwrap();
    assert_eq!(end["fields"]["preparation"][0]["cleanup"], "passed");
}

fn shell_recording(script: &str, status: i32) -> Recording {
    Recording {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
        status: Some(status),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        duration_ms: None,
    }
}

#[test]
fn preparation_failure_is_unverified_and_still_runs_cleanup() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(
        "\n[[scenarios.settings.prepare]]\nrun = \"exit 17\"\ncleanup = \"echo cleaned\"\n",
    );
    std::fs::write(path, text).unwrap();
    let mut recs = recordings("gate-default");
    recs.push(shell_recording("exit 17", 17));
    recs.push(shell_recording("echo cleaned", 0));
    let (code, out) = s.kr(&recs, &["--json", "run", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["items"][0]["tests"]["unverified"], 1);
    assert_eq!(out["items"][0]["tests"]["failed"], 0);
    assert_eq!(out["items"][0]["preparation"][0]["cleanup"], "passed");
    assert!(out["decisions"].as_array().unwrap().is_empty());
}

#[test]
fn preparation_config_errors_name_the_file_and_key_path() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[[scenarios.settings.prepare]]\nrun = \"echo ok\"\nbackground = true\n");
    std::fs::write(path, text).unwrap();
    let (code, out) = s.kr(&[], &["--json", "config"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains(".kr.toml"), "{out}");
    assert!(out.contains("scenarios.settings.prepare.0.ready"), "{out}");
    assert!(out.contains("add a readiness command"), "{out}");
}

#[test]
fn preparation_background_waits_until_ready_and_cleans_up() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[[scenarios.settings.prepare]]\nrun = \"python3 service.py start\"\nbackground = true\nready = \"python3 service.py ready\"\ncleanup = \"python3 service.py cleanup\"\n");
    std::fs::write(path, text).unwrap();
    let mut config = std::fs::read_to_string(s.home.path().join("repo/.kr.toml")).unwrap();
    config = config.replace("scenarios = [\"launch\"]", "scenarios = [\"settings\"]");
    std::fs::write(s.home.path().join("repo/.kr.toml"), config).unwrap();
    let mut recs = recordings("gate-default");
    for name in ["start", "ready", "cleanup"] {
        recs.push(
            kr_exec::read_recording(&root().join(format!("tests/fixtures/prepare/{name}.json")))
                .unwrap(),
        );
    }
    let (code, out) = s.kr(&recs, &["--json", "gate", "smoke"]);
    assert_eq!(code, Some(0), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(out["items"][0]["preparation"][0]["ready_attempts"], 1);
    assert_eq!(out["items"][0]["preparation"][0]["cleanup"], "passed");
}

#[test]
fn preparation_shared_instance_is_started_once_for_two_scenarios() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    for name in ["launch", "settings"] {
        text.push_str(&format!("\n[[scenarios.{name}.prepare]]\nshared = \"backend\"\nrun = \"echo serving\"\nbackground = true\nready = \"echo healthy\"\ncleanup = \"echo cleaned\"\n"));
    }
    std::fs::write(path, text).unwrap();
    let mut recs = with_an_ipad(&s);
    recs.push(shell_recording("echo serving", 0));
    recs.push(shell_recording("echo healthy", 0));
    recs.push(shell_recording("echo cleaned", 0));
    let (code, out) = s.kr_env(
        &recs,
        &["--json", "run", "launch", "settings"],
        &[("KR_RESOURCES__SIM", "2")],
    );
    assert_eq!(code, Some(0), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    for item in out["items"].as_array().unwrap() {
        assert_eq!(item["preparation"][0]["shared"], "backend");
        assert_eq!(
            item["preparation"][0]["scenarios"],
            serde_json::json!(["launch", "settings"])
        );
        assert_eq!(item["preparation"][0]["cleanup"], "passed");
    }
    assert_ne!(out["items"][0]["device"], out["items"][1]["device"]);
    let events = s.events();
    let ends: Vec<_> = events
        .iter()
        .filter(|e| e["type"] == "preparation_end")
        .collect();
    assert_eq!(ends.len(), 1, "only one shared service was cleaned up");
    assert_eq!(ends[0]["fields"]["shared"], "backend");
    assert_eq!(
        ends[0]["fields"]["scenarios"],
        serde_json::json!(["launch", "settings"])
    );
    let cleanup = events
        .iter()
        .position(|e| e["type"] == "preparation_end")
        .unwrap();
    assert!(
        events
            .iter()
            .enumerate()
            .filter(|(_, e)| e["type"] == "item_end")
            .all(|(index, _)| index > cleanup)
    );
}

#[test]
fn quarantine_preserves_preparation_and_cleanup_failures_as_unverified() {
    for (prepare, cleanup, expected_cases) in [
        ("exit 17", "echo cleaned", 0),
        ("echo prepared", "exit 17", 1),
        ("echo prepared", "echo cleaned", 1),
    ] {
        let s = Sample::new();
        s.quarantine(
            4,
            serde_json::json!([{
                "item": "broken", "test": BROKEN_TEST, "reason": "tracked flaky failure",
                "added": "2026-10-16"
            }]),
        );
        let path = s.home.path().join("repo/.kr.toml");
        let mut text = std::fs::read_to_string(&path).unwrap().replace(
            "[gates.smoke]\nscenarios = [\"launch\"]",
            "[gates.smoke]\nscenarios = [\"broken\"]",
        );
        text.push_str(&format!(
            "\n[[scenarios.broken.prepare]]\nrun = \"{prepare}\"\ncleanup = \"{cleanup}\"\n"
        ));
        std::fs::write(path, text).unwrap();
        let mut recs = broken();
        recs.push(shell_recording(
            prepare,
            if prepare == "exit 17" { 17 } else { 0 },
        ));
        recs.push(shell_recording(
            cleanup,
            if cleanup == "exit 17" { 17 } else { 0 },
        ));
        let unverified = prepare == "exit 17" || cleanup == "exit 17";
        let (code, out) = s.kr(&recs, &["--json", "gate", "smoke"]);
        assert_eq!(code, Some(i32::from(unverified)), "{out}");
        let out: serde_json::Value = serde_json::from_str(&out).unwrap();
        let item = &out["items"][0];
        assert_eq!(item["passed"], false);
        assert_eq!(item["blocking"], unverified);
        assert_eq!(item["tests"]["failed"], usize::from(!unverified));
        assert_eq!(item["cases"].as_array().map_or(0, Vec::len), expected_cases);
        if unverified {
            assert_eq!(item["tests"]["unverified"], 1);
            assert!(!item["unverified_reason"].as_str().unwrap().is_empty());
            assert!(item["quarantined"].as_array().unwrap().is_empty());
        } else {
            assert_eq!(item["quarantined"], serde_json::json!([BROKEN_TEST]));
        }
        let events = s.events();
        let ends: Vec<_> = events.iter().filter(|e| e["type"] == "item_end").collect();
        assert_eq!(ends.len(), 1);
        assert_eq!(ends[0]["fields"]["blocking"], unverified);
        assert_eq!(ends[0]["fields"]["quarantined"], item["quarantined"]);
        assert_eq!(
            ends[0]["fields"]["unverified_reason"],
            item["unverified_reason"]
        );
        if expected_cases == 0 {
            assert!(!events.iter().any(|e| e["type"] == "rerun"));
            assert!(item["xcresult"].is_null());
        }
    }
}

#[test]
fn preparation_shared_failure_marks_all_users_unverified_without_retry() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    for name in ["launch", "settings"] {
        text.push_str(&format!("\n[[scenarios.{name}.prepare]]\nshared = \"backend\"\nrun = \"exit 17\"\ncleanup = \"echo cleaned\"\n"));
    }
    std::fs::write(path, text).unwrap();
    let mut recs = recordings("gate-default");
    recs.push(shell_recording("exit 17", 17));
    recs.push(shell_recording("echo cleaned", 0));
    let (code, out) = s.kr(&recs, &["--json", "run", "launch", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        out["items"][0]["unverified_reason"],
        out["items"][1]["unverified_reason"]
    );
    for item in out["items"].as_array().unwrap() {
        assert_eq!(item["tests"]["unverified"], 1);
        assert_eq!(item["tests"]["failed"], 0);
        assert_eq!(item["preparation"][0]["cleanup"], "passed");
    }
}

#[test]
fn preparation_shared_mismatch_names_both_scenarios() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let text = std::fs::read_to_string(&path).unwrap()
        + "\n[[scenarios.launch.prepare]]\nshared = \"backend\"\nrun = \"echo first\"\n\n[[scenarios.settings.prepare]]\nshared = \"backend\"\nrun = \"echo second\"\n";
    std::fs::write(path, text).unwrap();
    let (code, out) = s.kr(&[], &["--json", "config"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("scenarios.launch.prepare.0"), "{out}");
    assert!(out.contains("scenarios.settings.prepare.0"), "{out}");
}

#[test]
fn preparation_readiness_timeout_is_unverified_and_cleaned() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let text = std::fs::read_to_string(&path).unwrap()
        + "\n[[scenarios.settings.prepare]]\nrun = \"echo serving\"\nbackground = true\nready = \"false\"\ntimeout = \"1s\"\ncleanup = \"echo cleaned\"\n";
    std::fs::write(path, text).unwrap();
    let mut recs = recordings("gate-default");
    recs.push(shell_recording("echo serving", 0));
    let mut probe = shell_recording("false", 1);
    probe.duration_ms = Some(1000);
    recs.push(probe);
    recs.push(shell_recording("echo cleaned", 0));
    let (code, out) = s.kr(&recs, &["--json", "run", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        out["items"][0]["unverified_reason"]
            .as_str()
            .unwrap()
            .contains("readiness timed out")
    );
    assert_eq!(out["items"][0]["tests"]["unverified"], 1);
    assert_eq!(out["items"][0]["preparation"][0]["cleanup"], "passed");
}

#[test]
fn preparation_interruption_cleans_up_and_returns_unverified() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let s = Sample::new();
        let path = s.home.path().join("repo/.kr.toml");
        let text = std::fs::read_to_string(&path).unwrap()
            + "\n[[scenarios.launch.prepare]]\nrun = \"echo serving\"\nbackground = true\nready = \"echo healthy\"\ncleanup = \"echo cleaned\"\n";
        std::fs::write(path, text).unwrap();
        let mut recs = recordings("gate-default");
        for r in &mut recs {
            if r.program == "xcodebuild"
                && r.args
                    .iter()
                    .any(|a| a == "build/kr/results/launch/run.xcresult")
            {
                r.duration_ms = Some(10_000);
            }
        }
        recs.push(shell_recording("echo serving", 0));
        recs.push(shell_recording("echo healthy", 0));
        recs.push(shell_recording("echo cleaned", 0));
        let child = s.spawn(&recs, &["--json", "run", "launch", "settings"]);
        s.wait_for_event(0, |e| {
            e["type"] == "item_start" && e["fields"]["item"] == "launch"
        });
        std::thread::sleep(std::time::Duration::from_millis(150));
        // SAFETY: only this test's own kr subprocess receives the signal.
        unsafe {
            libc::kill(child.id() as libc::pid_t, signal);
        }
        let (code, out) = wait_with_output(child);
        assert_eq!(code, Some(128 + signal), "{out}");
        let out: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(out["items"][0]["tests"]["unverified"], 1);
        assert_eq!(out["items"][0]["preparation"][0]["cleanup"], "passed");
        assert!(!s.pool().is_held(kr_resource::Resource::Sim, 1));
    }
}

#[test]
fn preparation_cleanup_failure_keeps_test_evidence_but_is_unverified() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let text = std::fs::read_to_string(&path).unwrap()
        + "\n[[scenarios.settings.prepare]]\nrun = \"echo prepared\"\ncleanup = \"exit 17\"\n";
    std::fs::write(path, text).unwrap();
    let mut recs = recordings("gate-default");
    recs.push(shell_recording("echo prepared", 0));
    recs.push(shell_recording("exit 17", 17));
    let (code, out) = s.kr(&recs, &["run", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("UNVERIFIED: settings"), "{out}");
    let end = s
        .events()
        .into_iter()
        .find(|e| e["type"] == "item_end")
        .unwrap();
    assert_eq!(end["fields"]["cases"][0]["outcome"], "passed");
    assert_eq!(end["fields"]["tests"]["unverified"], 1);
}

#[test]
fn preparation_invalid_fields_have_actionable_config_errors() {
    for (fields, expected) in [
        ("run = \" \"", "scenarios.settings.prepare.0.run"),
        (
            "run = \"true\"\ntimeout = \"never\"",
            "scenarios.settings.prepare.0.timeout",
        ),
        (
            "run = \"true\"\ndir = \"missing-directory\"",
            "scenarios.settings.prepare.0.dir",
        ),
        (
            "run = \"true\"\ndir = \"/tmp\"",
            "scenarios.settings.prepare.0.dir",
        ),
        (
            "run = \"true\"\nready = \"true\"",
            "scenarios.settings.prepare.0.ready",
        ),
        (
            "run = \"true\"\nshared = \" \"",
            "scenarios.settings.prepare.0.shared",
        ),
        (
            "run = \"true\"\ntimeout = \"18446744073709551615h\"",
            "scenarios.settings.prepare.0.timeout",
        ),
        ("run = \"true\"\nreadi = \"true\"", "unknown field `readi`"),
    ] {
        let s = Sample::new();
        let path = s.home.path().join("repo/.kr.toml");
        let text = std::fs::read_to_string(&path).unwrap()
            + "\n[[scenarios.settings.prepare]]\n"
            + fields
            + "\n";
        std::fs::write(path, text).unwrap();
        let (code, out) = s.kr(&[], &["--json", "config"]);
        assert_eq!(code, Some(1), "{fields}: {out}");
        assert!(out.contains(expected) && out.contains(".kr.toml"), "{out}");
        let out: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(out["error"]["code"], "kr::config::file");
        assert!(out["error"]["help"].as_str().is_some_and(|s| !s.is_empty()));
    }
}

#[test]
fn preparation_context_variables_are_not_machine_config_keys() {
    let s = Sample::new();
    let (code, out) = s.kr_env(
        &[],
        &["--json", "config"],
        &[
            ("KR_RUN_ID", "sample-run"),
            ("KR_PROJECT", "KrSample"),
            ("KR_SCENARIO", "settings"),
            ("KR_DEVICE_UDID", "sample-udid"),
            ("KR_RUN_DIR", "/tmp/sample-run"),
        ],
    );
    assert_eq!(code, Some(0), "a preparation command may call kr: {out}");
}

#[test]
fn pool_lease_variables_are_not_machine_config_keys() {
    let s = Sample::new();
    let (code, out) = s.kr_env(
        &[],
        &["--json", "config"],
        &[
            ("KR_SIM_UDID", "sample-udid"),
            ("KR_SIM_NAME", "KrSample-Verify-1"),
        ],
    );
    assert_eq!(
        code,
        Some(0),
        "a command inside `kr pool lease` may call kr: {out}"
    );
}

#[test]
fn preparation_offline_comparison_separates_environment_from_product_failure() {
    let s = Sample::new();
    s.seen_failure(BROKEN_TEST, "failed - asked to fail");
    let recs = broken();
    let (code, before) = s.kr(&recs, &["--json", "run", "broken"]);
    assert_eq!(code, Some(1), "{before}");
    let before: serde_json::Value = serde_json::from_str(&before).unwrap();
    assert_eq!(before["items"][0]["tests"]["failed"], 1);
    let path = s.home.path().join("repo/.kr.toml");
    let text = std::fs::read_to_string(&path).unwrap()
        + "\n[[scenarios.broken.prepare]]\nrun = \"exit 17\"\ncleanup = \"echo cleaned\"\n";
    std::fs::write(path, text).unwrap();
    let mut recs = recs;
    recs.push(shell_recording("exit 17", 17));
    recs.push(shell_recording("echo cleaned", 0));
    let (code, after) = s.kr(&recs, &["--json", "run", "broken"]);
    assert_eq!(code, Some(1), "{after}");
    let after: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_eq!(after["items"][0]["tests"]["failed"], 0);
    assert_eq!(after["items"][0]["tests"]["unverified"], 1);
    assert!(after["items"][0].get("xcresult").unwrap().is_null());
    println!(
        "ADR 0010 level 3 injected environment failure: failed 1 -> 0; unverified 0 -> 1; exit 1 -> 1; exact count error 0"
    );
}

#[test]
fn preparation_unused_shared_dependency_does_not_delay_cleanup_past_slot_release() {
    let s = Sample::new();
    let path = s.home.path().join("repo/.kr.toml");
    let mut text = std::fs::read_to_string(&path).unwrap()
        + "\n[[scenarios.launch.prepare]]\nrun = \"exit 17\"\n";
    for name in ["launch", "settings"] {
        text.push_str(&format!("\n[[scenarios.{name}.prepare]]\nshared = \"backend\"\nrun = \"echo serving\"\nbackground = true\nready = \"echo healthy\"\ncleanup = \"echo cleaned\"\n"));
    }
    std::fs::write(path, text).unwrap();
    let mut recs = recordings("gate-default");
    recs.push(shell_recording("exit 17", 17));
    for script in ["echo serving", "echo healthy", "echo cleaned"] {
        recs.push(shell_recording(script, 0));
    }
    let (code, out) = s.kr(&recs, &["run", "launch", "settings"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("PASS: settings"), "{out}");
    let events = s.events();
    let cleaned = events
        .iter()
        .position(|e| e["type"] == "preparation_end" && e["fields"]["shared"] == "backend")
        .unwrap();
    assert_eq!(
        events[cleaned]["fields"]["scenarios"],
        serde_json::json!(["settings"])
    );
    let released = events
        .iter()
        .position(|e| e["type"] == "sim_released")
        .unwrap();
    assert!(
        cleaned < released,
        "a scenario that never reached the shared step must not keep it alive"
    );
}

#[test]
fn changed_run_and_gate_default_efficiency_decisions_continue_and_record_pending_review() {
    for command in ["run", "gate"] {
        let s = Sample::new();
        let mut recs = recordings("gate-default");
        for (args, stdout) in [
            (
                vec!["diff", "--name-only", "-z", "--no-renames", "base", "--"],
                "App/KrSampleApp.swift\0",
            ),
            (vec!["ls-files", "--others", "--exclude-standard", "-z"], ""),
        ] {
            recs.push(kr_exec::Recording {
                program: "git".to_owned(),
                args: args.into_iter().map(str::to_owned).collect(),
                status: Some(0),
                stdout: stdout.to_owned(),
                stderr: String::new(),
                timed_out: false,
                duration_ms: None,
            });
        }
        let (code, out) = s.kr(
            &recs,
            &["--json", command, "--changed", "base", "--explain"],
        );
        assert_eq!(code, Some(0), "{command}: {out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["concluded"], true, "{v}");
        assert_eq!(v["total"], 4, "{v}");
        assert_eq!(v["decisions"][0]["kind"], "additional_scenarios", "{v}");
        assert_eq!(v["decisions"][0]["affects"], "efficiency", "{v}");
        let events = s.events();
        let defaults: Vec<_> = events
            .iter()
            .filter(|e| e["type"] == "decision" && e["fields"]["kind"] == "additional_scenarios")
            .collect();
        assert_eq!(defaults.len(), 1, "{events:?}");
        assert_eq!(defaults[0]["fields"]["via"], "default");
        assert_eq!(defaults[0]["fields"]["review"], "pending");
        assert_eq!(defaults[0]["fields"]["choice"], "add");
    }
}

#[test]
fn selection_defaults_are_recorded_even_when_the_build_fails() {
    let s = Sample::new();
    let mut recs = recordings("gate-default");
    for r in &mut recs {
        if r.program == "xcodebuild" && r.args.first().is_some_and(|a| a == "build-for-testing") {
            r.status = Some(65);
            r.stdout = "error: build failed".to_owned();
        }
    }
    for (args, stdout) in [
        (
            vec!["diff", "--name-only", "-z", "--no-renames", "base", "--"],
            "unknown.txt\0",
        ),
        (vec!["ls-files", "--others", "--exclude-standard", "-z"], ""),
    ] {
        recs.push(Recording {
            program: "git".to_owned(),
            args: args.into_iter().map(str::to_owned).collect(),
            status: Some(0),
            stdout: stdout.to_owned(),
            stderr: String::new(),
            timed_out: false,
            duration_ms: None,
        });
    }
    let (code, out) = s.kr(&recs, &["--json", "gate", "--changed", "base"]);
    assert_eq!(code, Some(1), "{out}");
    let events = s.events();
    assert_eq!(
        events
            .iter()
            .filter(|e| e["type"] == "decision"
                && e["fields"]["review"] == "pending"
                && e["fields"]["kind"] == "additional_scenarios")
            .count(),
        1,
        "{events:?}"
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["decisions"][0]["kind"], "additional_scenarios");
}

#[test]
fn explain_without_changed_describes_explicit_and_gate_scenarios() {
    for args in [
        vec!["--json", "run", "launch", "--explain"],
        vec!["--json", "gate", "--explain"],
    ] {
        let s = Sample::new();
        let (code, text) = s.kr(&recordings("gate-default"), &args);
        assert_eq!(code, Some(0), "{text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let launch = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "launch")
            .unwrap();
        assert!(
            launch["notes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n.as_str().unwrap().contains("selected")),
            "{v}"
        );
    }
}

#[test]
fn selection_review_and_a_failure_decision_have_distinct_ids_and_independent_verdicts() {
    let s = Sample::new();
    let mut recs = broken();
    recs.extend(recordings("gate-default"));
    for (args, stdout) in [
        (
            vec!["diff", "--name-only", "-z", "--no-renames", "base", "--"],
            "unknown.txt\0",
        ),
        (vec!["ls-files", "--others", "--exclude-standard", "-z"], ""),
    ] {
        recs.push(Recording {
            program: "git".to_owned(),
            args: args.into_iter().map(str::to_owned).collect(),
            status: Some(0),
            stdout: stdout.to_owned(),
            stderr: String::new(),
            timed_out: false,
            duration_ms: None,
        });
    }
    let (code, text) = s.kr(&recs, &["--json", "run", "broken", "--changed", "base"]);
    assert_eq!(code, Some(4), "{text}");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let decisions = v["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 2, "{v}");
    assert_ne!(decisions[0]["id"], decisions[1]["id"]);
    let failure = decisions
        .iter()
        .find(|d| d["kind"] == "failure_kind")
        .unwrap();
    let (code, text) = s.kr(
        &[],
        &[
            "--json",
            "decide",
            failure["id"].as_str().unwrap(),
            "product",
            "--reason",
            "the product assertion failed",
        ],
    );
    assert_eq!(code, Some(1), "{text}");
    let decided: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(decided["concluded"], true);
    let (code, text) = s.kr(&[], &["--json", "report", "--since", "1h"]);
    assert_eq!(code, Some(0), "{text}");
    let report: serde_json::Value = serde_json::from_str(&text).unwrap();
    let efficiency = report["accuracy"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "additional_scenarios")
        .unwrap();
    assert_eq!(efficiency["default_pending_review"], 1, "{report}");
}

#[test]
fn changed_runs_keep_selection_and_quarantine_decisions_in_one_namespace() {
    let s = Sample::new();
    s.healthy_history(9, None);
    let mut recs = recordings("gate-default");
    for (args, stdout) in [
        (
            vec!["diff", "--name-only", "-z", "--no-renames", "base", "--"],
            "App/KrSampleApp.swift\0",
        ),
        (vec!["ls-files", "--others", "--exclude-standard", "-z"], ""),
    ] {
        recs.push(Recording {
            program: "git".to_owned(),
            args: args.into_iter().map(str::to_owned).collect(),
            status: Some(0),
            stdout: stdout.to_owned(),
            stderr: String::new(),
            timed_out: false,
            duration_ms: None,
        });
    }
    let out = s.json(&recs, &["run", "--changed", "base"]);
    let decisions = out["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 2, "{out}");
    assert_eq!(decisions[0]["kind"], "additional_scenarios");
    assert_eq!(decisions[1]["kind"], "quarantine_remove");
    assert_ne!(decisions[0]["id"], decisions[1]["id"]);
    let events = s.events();
    for d in decisions {
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "decision_point" && e["fields"]["id"] == d["id"])
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "decision"
                    && e["fields"]["id"] == d["id"]
                    && e["fields"]["review"] == "pending")
                .count(),
            1
        );
    }
}
