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
