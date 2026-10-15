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

#[test]
fn a_test_that_fails_again_on_its_rerun_fails_the_run() {
    let s = Sample::new();
    // That run reused a build; give this fresh repo the build from the gate recording.
    let mut recs: Vec<Recording> = recordings("gate-default")
        .into_iter()
        .filter(|r| {
            r.program == "git"
                || r.program == "sh"
                || r.args.first().is_some_and(|a| a == "build-for-testing")
        })
        .collect();
    recs.extend(recordings("broken"));
    let (code, out) = s.kr(&recs, &["run", "broken"]);
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
    recs.extend((0..4).map(|_| sysctl_with_load(50.0)));
    // First round: the recorded failing run of `broken`, including its test rerun.
    recs.extend(recordings("broken").into_iter().skip(1));
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
