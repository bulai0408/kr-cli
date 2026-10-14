//! `kr check` on the sample project, replaying a real recorded run (tests/fixtures/check).

use std::path::{Path, PathBuf};

use assert_cmd::Command;

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

struct Sample {
    home: tempfile::TempDir,
}

impl Sample {
    /// A copy of tests/sample's .kr.toml (edited by `edit`) with the recorded build products.
    fn new(edit: impl Fn(String) -> String) -> Self {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let config = std::fs::read_to_string(root().join("tests/sample/.kr.toml")).unwrap();
        std::fs::write(repo.join(".kr.toml"), edit(config)).unwrap();
        copy_dir(
            &root().join("tests/fixtures/xcode/products"),
            &repo.join("build/kr/DerivedData/Build/Products"),
        );
        Self { home }
    }

    fn check(&self, recording: &str, args: &[&str]) -> (Option<i32>, String) {
        let dir = root().join("tests/fixtures/check").join(recording);
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path().display().to_string())
            .collect();
        files.sort();
        let out = Command::cargo_bin("kr")
            .unwrap()
            .env_clear()
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .env("KR_REPLAY", files.join(":"))
            .env("KR_APPLICATIONS", root().join("tests/fixtures/xcode/apps"))
            .env("KR_RESOURCES__BUILD", "1")
            .current_dir(self.home.path().join("repo"))
            .arg("check")
            .args(args)
            .output()
            .unwrap();
        let canonical = self.home.path().canonicalize().unwrap();
        let text = String::from_utf8_lossy(&out.stdout)
            .replace(canonical.to_str().unwrap(), "<home>")
            .replace(self.home.path().to_str().unwrap(), "<home>");
        (out.status.code(), text)
    }

    fn events(&self) -> Vec<serde_json::Value> {
        let dir = self.home.path().join(".local/state/kr/events");
        std::fs::read_dir(dir)
            .unwrap()
            .flat_map(|e| {
                std::fs::read_to_string(e.unwrap().path())
                    .unwrap()
                    .lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect::<Vec<serde_json::Value>>()
            })
            .collect()
    }
}

#[test]
fn a_clean_project_passes_with_timed_steps_under_a_build_slot() {
    let s = Sample::new(|c| c);
    let (code, out) = s.check("pass", &[]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.starts_with("PASS: check · generate "), "{out}");
    for step in ["build ", "filters ", "isolation "] {
        assert!(out.contains(step), "{out}");
    }
    let events = s.events();
    let steps: Vec<(&str, bool)> = events
        .iter()
        .filter(|e| e["type"] == "step_end")
        .map(|e| {
            (
                e["fields"]["step"].as_str().unwrap(),
                e["fields"]["ok"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(steps.len(), 4);
    assert!(steps.iter().all(|(_, ok)| *ok));
    assert!(
        events
            .iter()
            .filter(|e| e["type"] == "step_end")
            .all(|e| e["fields"]["duration_ms"].is_u64())
    );
    let kinds: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(kinds.first(), Some(&"queue_start"));
    assert_eq!(kinds.last(), Some(&"slot_released"));
    assert_eq!(
        events[0]["fields"]["resource"], "build",
        "check holds only a build slot"
    );
}

#[test]
fn a_compile_error_fails_with_the_compiler_message_and_the_log_path() {
    let s = Sample::new(|c| c);
    let (code, out) = s.check("compile-error", &[]);
    assert_eq!(code, Some(1));
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with("FAIL: check · build build-for-testing KrSample"),
        "{out}"
    );
    assert!(
        lines[1]
            .ends_with("App/KrSampleApp.swift:14:52: error: cannot find 'missingHelper' in scope"),
        "{out}"
    );
    let log = lines
        .last()
        .unwrap()
        .strip_prefix("  log: ")
        .unwrap()
        .replace(
            "<home>",
            s.home.path().canonicalize().unwrap().to_str().unwrap(),
        );
    assert!(
        std::fs::read_to_string(log)
            .unwrap()
            .contains("TEST BUILD FAILED")
    );
}

#[test]
fn a_filter_that_matches_no_test_points_at_its_line() {
    let s = Sample::new(|c| {
        c.replace(
            "KrSampleUITests/LaunchUITests\"",
            "KrSampleUITests/NoSuchTests\"",
        )
    });
    let (code, out) = s.check("pass", &[]);
    assert_eq!(code, Some(1));
    insta::assert_snapshot!(out, @r"
    FAIL: check · filters failed after 0s
      `KrSampleUITests/NoSuchTests` matches no test (<home>/repo/.kr.toml:25:9)
    ");
}

#[test]
fn a_filter_naming_a_target_the_scheme_does_not_build_is_explained() {
    let s = Sample::new(|c| {
        c.replace(
            "\"KrSampleTests\"]",
            "\"KrSampleTests\", \"KrSampleSnapshotTests\"]",
        )
    });
    let (code, out) = s.check("pass", &["unit"]);
    assert_eq!(code, Some(1));
    assert!(out.contains("`KrSampleSnapshotTests` names test target `KrSampleSnapshotTests`, which scheme KrSample does not build (it has KrSampleTests, KrSampleUITests)"), "{out}");
}

#[test]
fn an_app_with_a_real_bundle_id_fails_the_isolation_check() {
    let s = Sample::new(|c| {
        c.replace(
            "bundle_ids = [\"dev.kr.sample.e2e\"]",
            "bundle_ids = [\"dev.kr.sample.uitest\"]",
        )
    });
    let (code, out) = s.check("pass", &[]);
    assert_eq!(code, Some(1));
    assert!(out.contains("isolation failed"), "{out}");
    assert!(out.contains("has bundle id `dev.kr.sample.e2e`, not one of the synthetic ids [\"dev.kr.sample.uitest\"]"), "{out}");
}

#[test]
fn an_entitlement_outside_the_allowed_environments_fails_the_isolation_check() {
    let s = Sample::new(|c| c.replace("= [\"Development\"]", "= [\"Sandbox\"]"));
    let (code, out) = s.check("pass", &[]);
    assert_eq!(code, Some(1));
    assert!(out.contains("entitlement com.apple.developer.icloud-container-environment = `Development`, allowed: [\"Sandbox\"]"), "{out}");
}

#[test]
fn an_unknown_target_name_lists_the_known_ones() {
    let s = Sample::new(|c| c);
    let out = Command::cargo_bin("kr")
        .unwrap()
        .env_clear()
        .env("HOME", s.home.path())
        .current_dir(s.home.path().join("repo"))
        .args(["check", "lunch"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("`lunch` is neither a scenario nor a suite")
            && err.contains("launch, settings, unit"),
        "{err}"
    );
}
