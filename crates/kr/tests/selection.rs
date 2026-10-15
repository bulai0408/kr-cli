//! Selection through the CLI: real Krill descriptions, no Xcode execution.
use assert_cmd::Command;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
struct Sample {
    home: tempfile::TempDir,
}
impl Sample {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join(".kr.toml"),
            r#"
[project]
name = "Krill"
scheme = "Krill"
xcodeproj = "Krill.xcodeproj"
platforms = ["iOS"]
[simulators]
runtime = "iOS 18.6"
[simulators.devices.default]
device_type = "iPhone 16 Pro"
[scenarios.favorites]
only = ["KrillUITests/FavoritesUITests"]
modules = ["Favorites"]
[scenarios.detail]
only = ["KrillUITests/DetailUITests"]
modules = ["TitleDetail"]
[scenarios.settings]
only = ["KrillUITests/SettingsUITests"]
modules = ["Settings"]
[modules.Favorites]
paths = ["Krill/Favorites"]
depends_on = ["TitleDetail"]
[modules.TitleDetail]
paths = ["Krill/TitleDetail"]
depends_on = ["Metadata", "Sources"]
[modules.Settings]
paths = ["Krill/Settings"]
[gates.default]
scenarios = ["*"]
[gates.smoke]
scenarios = ["settings"]
"#,
        )
        .unwrap();
        Self { home }
    }
    fn repo(&self) -> PathBuf {
        self.home.path().join("repo")
    }
    fn append(&self, text: &str) {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(self.repo().join(".kr.toml"))
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    fn recordings(&self, changed: &[&str]) -> Vec<Value> {
        let package: Value = serde_json::from_str(
            &std::fs::read_to_string(root().join("tests/fixtures/selection/krill-package.json"))
                .unwrap(),
        )
        .unwrap();
        vec![
            json!({"program":"git","args":["ls-files","-co","--exclude-standard","-z"],"status":0,"stdout":".kr.toml\u{0}KrillKit/Package.swift\u{0}project.yml\u{0}KrillKit/Sources/Metadata/Metadata.swift\u{0}"}),
            json!({"program":"git","args":["diff","--name-only","-z","--no-renames","base","--"],"status":0,"stdout":changed.join("\0") + "\0"}),
            json!({"program":"git","args":["ls-files","--others","--exclude-standard","-z"],"status":0,"stdout":""}),
            json!({"program":"swift","args":["package","--scratch-path",self.home.path().join(".local/state/kr/package-descriptions").join(blake3::hash(self.repo().canonicalize().unwrap().join("KrillKit").to_string_lossy().as_bytes()).to_hex().as_str()).display().to_string(),"describe","--type","json"],"status":0,"stdout":package.to_string()}),
            json!({"program":"xcodegen","args":["dump","--type","json","--no-env"],"status":0,"stdout":std::fs::read_to_string(root().join("tests/fixtures/selection/krill-project.json")).unwrap()}),
        ]
    }
    fn kr(&self, args: &[&str], recordings: &[Value]) -> (i32, String) {
        let replay = self.home.path().join("replay");
        std::fs::create_dir_all(&replay).unwrap();
        let paths: Vec<_> = recordings
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let p = replay.join(format!("{i}.json"));
                std::fs::write(&p, r.to_string()).unwrap();
                p.display().to_string()
            })
            .collect();
        let out = Command::cargo_bin("kr")
            .unwrap()
            .env_clear()
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .env("KR_RESOURCES__SIM", "2")
            .env("KR_REPLAY", paths.join(":"))
            .current_dir(self.repo())
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code().unwrap(),
            String::from_utf8(out.stdout).unwrap() + &String::from_utf8_lossy(&out.stderr),
        )
    }
    fn plan(&self, changed: &[&str], extra: &[&str]) -> Value {
        let mut args = vec!["--json", "plan", "--changed", "base"];
        args.extend(extra);
        let (code, text) = self.kr(&args, &self.recordings(changed));
        assert_eq!(code, 0, "{text}");
        serde_json::from_str(&text).unwrap()
    }
}
fn names(v: &Value) -> Vec<&str> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect()
}
#[test]
fn shared_metadata_selects_favorites_through_detail_even_outside_the_gate() {
    let s = Sample::new();
    let v = s.plan(
        &["KrillKit/Sources/Metadata/Metadata.swift"],
        &["--gate", "smoke", "--explain"],
    );
    assert_eq!(names(&v), ["detail", "favorites"]);
    assert_eq!(v["selection"]["required"], json!(["detail", "favorites"]));
    assert!(
        v["items"][1]["why"]
            .as_str()
            .unwrap()
            .contains("TitleDetail"),
        "{v}"
    );
    assert_eq!(v["decisions"], json!([]));
}

#[test]
fn unknown_changes_add_candidates_but_declared_ignores_raise_no_decision() {
    let s = Sample::new();
    let v = s.plan(&["docs/guide.md"], &[]);
    assert_eq!(names(&v), ["detail", "favorites", "settings"]);
    assert_eq!(v["decisions"][0]["affects"], "efficiency");
    assert_eq!(v["decisions"][0]["default"]["option"], "add");
    s.append("\n[selection]\nignore = [\"docs/**\", \"**/*.md\"]\n");
    let v = s.plan(&["docs/guide.md"], &["--explain"]);
    assert!(names(&v).is_empty());
    assert_eq!(v["decisions"], json!([]));
    assert_eq!(
        v["selection"]["ignored"]["docs/guide.md"],
        "ignored by docs/**"
    );
    let (code, text) = s.kr(
        &["plan", "--changed", "base", "--explain"],
        &s.recordings(&["docs/guide.md"]),
    );
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("docs/guide.md: ignored by docs/**"), "{text}");
}

#[test]
fn invalid_ignore_and_unknown_dependencies_name_the_config_key_and_fix() {
    let s = Sample::new();
    s.append("\n[selection]\nignore = [\"[\"]\n");
    let (code, text) = s.kr(
        &["plan", "--changed", "base"],
        &s.recordings(&["docs/guide.md"]),
    );
    assert_ne!(code, 0);
    assert!(
        text.contains(".kr.toml")
            && text.contains("selection.ignore")
            && text.contains("valid glob"),
        "{text}"
    );
    let s = Sample::new();
    s.append("\n[modules.Broken]\npaths = [\"Feature\"]\ndepends_on = [\"Typo\"]\n");
    let (code, text) = s.kr(
        &["plan", "--changed", "base"],
        &s.recordings(&["Feature/A.swift"]),
    );
    assert_ne!(code, 0);
    assert!(
        text.contains("modules.Broken.depends_on")
            && text.contains("Typo")
            && text.contains("add [modules.Typo]"),
        "{text}"
    );
}

#[test]
fn additional_scenarios_include_project_scoped_cofailure_and_flaky_sample_counts() {
    use kr_events::{Event, EventKind, EventLog, RunId};
    let s = Sample::new();
    let log = EventLog::new(s.home.path().join(".local/state/kr/events"));
    for (project, pairs) in [
        ("Krill", vec![(false, true), (false, false), (true, true)]),
        ("OtherProject", vec![(false, false); 5]),
    ] {
        for (a, b) in pairs {
            let run = RunId::new();
            for (name, pass) in [("detail", a), ("favorites", b)] {
                log.append(&Event::now(run,Some(project),EventKind::ItemEnd,json!({"item":name,"passed":pass,"flaky":if name=="favorites" && pass {json!(["Favorites/testTap()"]) } else {json!([])},"tests":{"passed":u32::from(pass),"failed":u32::from(!pass)}}))).unwrap();
            }
        }
    }
    let v = s.plan(&["unknown.txt"], &[]);
    let history = &v["decisions"][0]["evidence"]["history"];
    let detail = history["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["item"] == "detail")
        .unwrap();
    assert_eq!(detail["runs"], 3);
    assert_eq!(detail["failed"], 2);
    assert_eq!(detail["flaky_rate"], 0.0);
    let favorites = history["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["item"] == "favorites")
        .unwrap();
    assert_eq!(favorites["runs"], 3);
    assert_eq!(favorites["flaky"], 2);
    assert!((favorites["flaky_rate"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    let pair = &history["cofailures"][0];
    assert_eq!(pair["items"], json!(["detail", "favorites"]));
    assert_eq!(pair["together"], 3);
    assert_eq!(pair["failed_together"], 1);
    assert!((pair["rate"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-9);
    assert_eq!(history["days"], 90);
}

#[test]
fn hash_explains_the_exact_inputs_used_by_plan_and_changes_with_file_content() {
    let s = Sample::new();
    let records = s.recordings(&[]);
    let file = s.repo().join("KrillKit/Sources/Metadata/Metadata.swift");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "let value = 1").unwrap();
    let (code, text) = s.kr(&["--json", "hash", "favorites"], &records);
    assert_eq!(code, 0, "{text}");
    let a: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(a["schema_version"], 1);
    assert_eq!(a["decisions"], json!([]));
    let job = &a["jobs"][0];
    assert!(job["hash"].as_str().is_some_and(|s| s.len() == 64), "{a}");
    assert_eq!(job["inputs"]["command"][0], "xcodebuild");
    let input = job["inputs"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["path"] == "KrillKit/Sources/Metadata/Metadata.swift")
        .unwrap();
    assert_eq!(
        input["content_hash"],
        blake3::hash(b"let value = 1").to_hex().as_str()
    );
    std::fs::write(&file, "let value = 2").unwrap();
    let (code, text) = s.kr(&["--json", "hash", "favorites"], &records);
    assert_eq!(code, 0, "{text}");
    let b: Value = serde_json::from_str(&text).unwrap();
    assert_ne!(a["jobs"][0]["hash"], b["jobs"][0]["hash"]);
}

#[test]
fn an_owned_file_without_scenario_coverage_adds_candidates_instead_of_silently_skipping() {
    let s = Sample::new();
    let v = s.plan(&["Krill/AppOnlyFeature.swift"], &[]);
    assert_eq!(names(&v), ["detail", "favorites", "settings"]);
    assert_eq!(v["decisions"][0]["kind"], "additional_scenarios", "{v}");
}

#[test]
fn explicit_scenarios_can_only_add_to_the_reverse_dependency_closure() {
    let s = Sample::new();
    let v = s.plan(
        &["KrillKit/Sources/Metadata/Metadata.swift"],
        &["settings", "--also", "favorites"],
    );
    assert_eq!(names(&v), ["detail", "favorites", "settings"]);
    assert_eq!(v["selection"]["required"], json!(["detail", "favorites"]));
}

#[test]
fn real_krill_commits_cover_batch_six_indirect_consumers_and_report_unknown_frequency() {
    let s = Sample::new();
    let fixture = root().join("tests/fixtures/selection");
    let config = std::fs::read_to_string(fixture.join("krill-selection.toml")).unwrap();
    std::fs::write(s.repo().join(".kr.toml"), &config).unwrap();
    let impact: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.join("krill-batch6-impact.json")).unwrap(),
    )
    .unwrap();
    // Independent acceptance oracle: the sixth batch's actual hand-maintained map.
    for (path, pattern) in [
        (
            "KrillKit/Sources/Metadata/Metadata.swift",
            "KrillKit/Sources/Metadata/*",
        ),
        (
            "KrillKit/Sources/Sources/Source.swift",
            "KrillKit/Sources/Sources/*",
        ),
    ] {
        let v = s.plan(&[path], &[]);
        assert_eq!(v["decisions"], json!([]), "{v}");
        let expected = impact["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["paths"].as_array().unwrap().iter().any(|p| p == pattern))
            .unwrap();
        println!(
            "batch6 {path}: selected {}, expected {}",
            names(&v).len(),
            expected["scenarios"].as_array().unwrap().len()
        );
        for n in expected["scenarios"].as_array().unwrap() {
            assert!(
                names(&v).contains(&n.as_str().unwrap()),
                "missing {n} for {path}: {v}"
            );
        }
        assert!(
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["name"] == "favorites")
                .unwrap()["why"]
                .as_str()
                .unwrap()
                .contains("TitleDetail")
        );
    }
    let commits: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture.join("krill-commits.json")).unwrap())
            .unwrap();
    let mut fallback = [0, 0];
    let mut total_selected = [0, 0];
    for (i, ignore) in [
        "",
        "\n[selection]\nignore = [\"docs/**\", \"**/*.md\", \".github/**\"]\n",
    ]
    .iter()
    .enumerate()
    {
        std::fs::write(s.repo().join(".kr.toml"), config.clone() + ignore).unwrap();
        for commit in commits.as_array().unwrap() {
            let changed: Vec<_> = commit["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let v = s.plan(&changed, &[]);
            for prefix in [
                "KrillKit/Sources/Metadata/",
                "KrillKit/Sources/Sources/",
                "KrillKit/Sources/Domain/",
                "KrillKit/Sources/UserState/",
            ] {
                if changed.iter().any(|p| p.starts_with(prefix)) {
                    let pattern = format!("{prefix}*");
                    let rule = impact["rules"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|r| r["paths"].as_array().unwrap().iter().any(|p| p == &pattern))
                        .unwrap();
                    let expected: Vec<_> = if rule["scenarios"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|n| n == "*")
                    {
                        impact["synthetic"].as_array().unwrap().iter().collect()
                    } else {
                        rule["scenarios"].as_array().unwrap().iter().collect()
                    };
                    for scene in expected {
                        assert!(
                            names(&v).contains(&scene.as_str().unwrap()),
                            "commit {} misses {scene} for {prefix}: {v}",
                            commit["commit"]
                        );
                    }
                }
            }
            fallback[i] += usize::from(!v["decisions"].as_array().unwrap().is_empty());
            total_selected[i] += names(&v).len();
        }
    }
    println!(
        "Krill 30-commit replay: unknown defaults without/with ignore {fallback:?}; selected scenario totals {total_selected:?}"
    );
    assert!(
        fallback[1] < fallback[0],
        "explicit documentation ignores must reduce unknown defaults: {fallback:?}"
    );
}

#[test]
fn real_event_replay_counts_final_scenario_runs_and_flaky_runs() {
    let s = Sample::new();
    let fixture = root().join("tests/fixtures/selection");
    std::fs::copy(
        fixture.join("krill-selection.toml"),
        s.repo().join(".kr.toml"),
    )
    .unwrap();
    let log = kr_events::EventLog::new(s.home.path().join(".local/state/kr/events"));
    for line in std::fs::read_to_string(fixture.join("krill-item-events.ndjson"))
        .unwrap()
        .lines()
    {
        let mut event: kr_events::Event = serde_json::from_str(line).unwrap();
        event.ts = jiff::Timestamp::now();
        log.append(&event).unwrap();
    }
    let v = s.plan(&["unknown.txt"], &[]);
    let stats = v["decisions"][0]["evidence"]["history"]["scenarios"]
        .as_array()
        .unwrap();
    let favorites = stats.iter().find(|i| i["item"] == "favorites").unwrap();
    assert_eq!(favorites["runs"], 2);
    assert_eq!(favorites["failed"], 0);
    assert_eq!(favorites["flaky"], 0);
    let detail = stats.iter().find(|i| i["item"] == "detail-search").unwrap();
    assert_eq!(detail["runs"], 3);
    assert_eq!(detail["flaky"], 1);
    assert!((detail["flaky_rate"].as_f64().unwrap() - 1.0 / 3.0).abs() < 1e-9);
}

#[test]
fn an_unreadable_or_malformed_package_graph_defaults_to_additional_scenarios() {
    for stdout in ["{\"targets\":null}", "not json"] {
        let s = Sample::new();
        let mut recordings = s.recordings(&["KrillKit/Sources/Metadata/Metadata.swift"]);
        recordings
            .iter_mut()
            .find(|v| v["program"] == "swift")
            .unwrap()["stdout"] = json!(stdout);
        let (code, text) = s.kr(&["--json", "plan", "--changed", "base"], &recordings);
        assert_eq!(code, 0, "{text}");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(names(&v), ["detail", "favorites", "settings"]);
        assert_eq!(v["decisions"][0]["kind"], "additional_scenarios");
    }
}

#[test]
fn deleted_and_untracked_paths_and_resource_only_targets_remain_selectable() {
    let s = Sample::new();
    let mut records = s.recordings(&["KrillKit/Sources/Metadata/Deleted.swift"]);
    records
        .iter_mut()
        .find(|v| v["program"] == "git" && v["args"][0] == "ls-files" && v["args"][1] == "--others")
        .unwrap()["stdout"] = json!("Krill/Settings/New.swift\0");
    let (code, text) = s.kr(&["--json", "plan", "--changed", "base"], &records);
    assert_eq!(code, 0, "{text}");
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(names(&v), ["detail", "favorites", "settings"]);
    assert_eq!(v["decisions"], json!([]));
    assert!(
        v["selection"]["changed_files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "Krill/Settings/New.swift")
    );
    // The directory comes from the package target path, not just its Swift source list.
    let v = s.plan(&["KrillKit/Sources/Metadata/Resources/locale.json"], &[]);
    assert_eq!(names(&v), ["detail", "favorites"]);
}

#[test]
fn ignore_globs_require_double_star_to_cross_directory_boundaries() {
    let s = Sample::new();
    s.append("\n[selection]\nignore = [\"docs/*\"]\n");
    let v = s.plan(&["docs/nested/spec.txt"], &[]);
    assert_eq!(names(&v), ["detail", "favorites", "settings"]);
    assert_eq!(v["decisions"][0]["kind"], "additional_scenarios");
    let v = s.plan(&["docs/spec.txt"], &[]);
    assert!(names(&v).is_empty());
}
