use assert_cmd::Command;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
struct Repo {
    home: tempfile::TempDir,
    path: PathBuf,
}
impl Repo {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("repo");
        std::fs::create_dir(&path).unwrap();
        let s = Self { home, path };
        s.git(&["init", "-b", "main"]);
        s.git(&["config", "user.name", "Fixture"]);
        s.git(&["config", "user.email", "fixture@example.invalid"]);
        s.write("seed", "base\n");
        s.commit("base");
        s
    }
    fn write(&self, path: &str, text: &str) {
        let p = self.path.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    fn git(&self, args: &[&str]) -> String {
        let o = std::process::Command::new("git")
            .current_dir(&self.path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8(o.stdout).unwrap().trim().to_owned()
    }
    fn commit(&self, msg: &str) {
        self.git(&["add", "."]);
        self.git(&["commit", "-qm", msg]);
    }
    fn kr(&self) -> Command {
        let mut c = Command::cargo_bin("kr").unwrap();
        c.current_dir(&self.path)
            .env("HOME", self.home.path())
            .env("KR_STATE_DIR", self.home.path().join("state"))
            .env("NO_COLOR", "1")
            .env_remove("KR_REPLAY")
            .env_remove("KR_RECORD");
        c
    }
    fn replay(&self, recordings: Vec<Value>) -> String {
        recordings
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let p = self.home.path().join(format!("{i}.json"));
                std::fs::write(&p, serde_json::to_vec(r).unwrap()).unwrap();
                p.display().to_string()
            })
            .collect::<Vec<_>>()
            .join(":")
    }
}

#[test]
fn xcstrings_preserves_real_krill_entries_and_validates_before_replacing_ours() {
    let s = Repo::new();
    for n in ["base", "ours", "theirs"] {
        std::fs::copy(
            root().join(format!("tests/fixtures/merge/{n}.xcstrings")),
            s.path.join(format!("{n}.xcstrings")),
        )
        .unwrap();
    }
    let replay=s.replay(vec![json!({"program":"xcrun","args":["xcstringstool","print","ours.xcstrings.kr-merge.xcstrings"],"status":0,"stdout":"About\nAdd Folder\nAudio\n"})]);
    let o = s
        .kr()
        .env("KR_REPLAY", replay)
        .args([
            "merge",
            "xcstrings",
            "base.xcstrings",
            "ours.xcstrings",
            "theirs.xcstrings",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&o).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["schema_version"], 1);
    let merged: Value =
        serde_json::from_slice(&std::fs::read(s.path.join("ours.xcstrings")).unwrap()).unwrap();
    assert_eq!(merged["strings"].as_object().unwrap().len(), 3);
    let original: Value = serde_json::from_slice(
        &std::fs::read(root().join("tests/fixtures/merge/theirs.xcstrings")).unwrap(),
    )
    .unwrap();
    assert_eq!(merged["strings"]["Audio"], original["strings"]["Audio"]);
}

#[test]
fn predict_reports_text_and_logical_conflicts_with_tickets_and_records_a_resolution() {
    let s = Repo::new();
    s.git(&["checkout", "-qb", "left"]);
    s.write("seed", "left\n");
    s.commit("#101 left requirement");
    s.git(&["checkout", "-qb", "right", "main"]);
    s.write("seed", "right\n");
    s.commit("#102 right requirement");
    let before = s.git(&["status", "--porcelain"]);
    let out = s
        .kr()
        .args([
            "merge", "predict", "--base", "main", "left", "right", "--json",
        ])
        .assert()
        .code(4)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["pairs"][0]["conflicts"], json!(["seed"]));
    assert_eq!(v["decisions"][0]["kind"], "merge_resolution");
    assert_eq!(
        v["decisions"][0]["evidence"]["left"]["issues"],
        json!([101])
    );
    assert_eq!(
        v["decisions"][0]["evidence"]["right"]["issues"],
        json!([102])
    );
    assert_eq!(s.git(&["status", "--porcelain"]), before);
    let id = v["decisions"][0]["id"].as_str().unwrap();
    let out = s
        .kr()
        .args([
            "decide",
            id,
            "manual",
            "--reason",
            "preserve both requirements",
            "--json",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["verification"], "pending");
    assert_eq!(v["concluded"], true);
    assert_eq!(v["ok"], false);
    s.kr()
        .args(["decide", id, "ours", "--reason", "replace"])
        .assert()
        .code(2);
}

#[test]
fn install_enables_rerere_in_all_worktrees_and_warns_about_tracked_attributes() {
    let s = Repo::new();
    s.git(&["config", "extensions.worktreeConfig", "true"]);
    let other = s.home.path().join("other");
    s.git(&["worktree", "add", "-b", "other", other.to_str().unwrap()]);
    let status = std::process::Command::new("git")
        .args([
            "-C",
            other.to_str().unwrap(),
            "config",
            "--worktree",
            "rerere.enabled",
            "false",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let out = s
        .kr()
        .args(["merge", "install", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["worktrees"].as_array().unwrap().len(), 2);
    assert_eq!(v["tracked_file_changed"], ".gitattributes");
    assert!(v["next"].as_str().unwrap().contains("commit"));
    assert_eq!(s.git(&["config", "--get", "rerere.enabled"]), "true");
    let output = std::process::Command::new("git")
        .args([
            "-C",
            other.to_str().unwrap(),
            "config",
            "--get",
            "rerere.enabled",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "true");
    assert!(
        s.git(&["config", "--get", "merge.kr-xcstrings.driver"])
            .contains("merge xcstrings")
    );
    let first = std::fs::read(s.path.join(".gitattributes")).unwrap();
    s.kr().args(["merge", "install"]).assert().success();
    assert_eq!(first, std::fs::read(s.path.join(".gitattributes")).unwrap());
}

#[test]
fn quick_check_catches_constructed_batch6_duplicate_drop_before_compilation() {
    let s = Repo::new();
    s.write(
        "Schema.swift",
        r#"migrator.registerMigration("UserState.v14") { db in
try db.execute(sql: "DROP TABLE Legacy")
try db.execute(sql: "DROP TABLE Legacy")
}
"#,
    );
    s.commit("#114 constructed conflict resolution with duplicate DROP");
    let out = s
        .kr()
        .args(["merge", "check", "--base", "HEAD~1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["compiled"], false);
    assert!(
        v["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"] == "duplicate_drop")
    );
}

fn quick_recordings(s: &Repo, base: &str) -> Vec<Value> {
    let resolved = s.git(&[
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{base}^{{commit}}"),
    ]);
    vec![vec!["rev-parse","--show-toplevel"],vec!["rev-parse","--verify","--end-of-options",&format!("{base}^{{commit}}")],vec!["diff","--name-only","-z",&resolved,"--"],vec!["ls-files","--others","--exclude-standard","-z"],vec!["ls-files","-z"]]
    .into_iter().map(|args| {
        let o=std::process::Command::new("git").current_dir(&s.path).args(&args).output().unwrap();
        json!({"program":"git","args":args,"status":o.status.code(),"stdout":String::from_utf8(o.stdout).unwrap(),"stderr":String::from_utf8(o.stderr).unwrap()})
    }).collect()
}

fn xcode_record(scheme: &str, mode: &str) -> Value {
    let mut record: Value = serde_json::from_slice(
        &std::fs::read(root().join("tests/fixtures/run/gate-default/004-xcodebuild.json")).unwrap(),
    )
    .unwrap();
    record["args"] = json!([
        "build",
        "-quiet",
        "-skipMacroValidation",
        "-scheme",
        scheme,
        "-destination",
        "generic/platform=macOS",
        "-derivedDataPath",
        "build/kr/DerivedData"
    ]);
    if mode == "failed" {
        record["status"] = json!(65);
    }
    record["timed_out"] = json!(mode == "timeout");
    record
}

fn incremental_case(mode: &str) {
    let s = Repo::new();
    s.write(".kr.toml", "[project]\nname='Fixture'\nplatforms=['macOS']\n[suites.foo]\ntype='build'\ndir='Modules/Foo'\nscheme='Foo'\n[suites.bar]\ntype='build'\ndir='Modules/Bar'\nscheme='Bar'\n");
    s.write(
        if mode == "header" {
            "Modules/Foo/Foo.h"
        } else {
            "Modules/Foo/Foo.swift"
        },
        "struct Foo {};\n",
    );
    s.commit("#101 change Foo");
    let mut rec = quick_recordings(&s, "HEAD~1");
    rec.push(
        serde_json::from_slice(
            &std::fs::read(root().join("tests/fixtures/run/gate-default/002-sysctl.json")).unwrap(),
        )
        .unwrap(),
    );
    rec.push(xcode_record("Foo", mode));
    let out = s
        .kr()
        .env("KR_REPLAY", s.replay(rec))
        .env("KR_RESOURCES__BUILD", "1")
        .args(["merge", "check", "--base", "HEAD~1", "--json"])
        .assert()
        .code(if mode == "failed" || mode == "timeout" {
            1
        } else {
            0
        })
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["compiled"], mode != "failed" && mode != "timeout");
    assert_eq!(v["builds"][0]["scheme"], "Foo");
    assert_eq!(v["builds"].as_array().unwrap().len(), 1);
    let events = kr_events::EventLog::new(s.home.path().join("state/events"))
        .read_since("2026-01-01T00:00:00Z".parse().unwrap())
        .unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == kr_events::EventKind::SlotReleased)
    );
}

fn resolution_case(mode: &str) {
    let s = Repo::new();
    s.write(".kr.toml","[project]\nname='Fixture'\nplatforms=['macOS']\n[suites.app]\ntype='build'\nscheme='Fixture'\n");
    s.write("Seed.swift", "struct Seed {}\n");
    s.commit("configuration");
    s.git(&["checkout", "-qb", "left"]);
    s.write("Seed.swift", "struct Left {}\n");
    s.commit("#101 left");
    let left = s.git(&["rev-parse", "HEAD"]);
    s.git(&["checkout", "-qb", "right", "main"]);
    s.write("Seed.swift", "struct Right {}\n");
    s.commit("#102 right");
    let right = s.git(&["rev-parse", "HEAD"]);
    let out = s
        .kr()
        .args([
            "merge", "predict", "--base", "main", "left", "right", "--json",
        ])
        .assert()
        .code(4)
        .get_output()
        .stdout
        .clone();
    let p: Value = serde_json::from_slice(&out).unwrap();
    let id = p["decisions"][0]["id"].as_str().unwrap();
    s.kr()
        .args(["decide", id, "combine", "--reason", "keep both types"])
        .assert()
        .code(1);
    s.git(&["checkout", "left"]);
    let merge = std::process::Command::new("git")
        .current_dir(&s.path)
        .args(["merge", "--no-commit", "right"])
        .output()
        .unwrap();
    assert_eq!(merge.status.code(), Some(1));
    s.write("Seed.swift", "struct Left {}\nstruct Right {}\n");
    s.commit("merge both requirements");
    let tree = s.git(&["rev-parse", "HEAD^{tree}"]);
    let run = kr_events::RunId::new();
    let log = kr_events::EventLog::new(s.home.path().join("state/events"));
    log.append(&kr_events::Event::now(run,Some("Fixture"),kr_events::EventKind::ItemEnd,json!({"item":"affected","passed":true,"tests":{"passed":if mode == "zero" {0} else {1},"failed":0},"identity":{"tree":if mode == "old_tree" {"an-old-tree"} else {&tree}}}))).unwrap();
    log.append(&kr_events::Event::now(
        run,
        Some("Fixture"),
        kr_events::EventKind::RunEnd,
        json!({"ok":mode != "failed_run","total":1,"passed":1}),
    ))
    .unwrap();
    let mut rec = quick_recordings(&s, "main");
    for args in [
        vec!["rev-parse", "HEAD^{tree}"],
        vec!["status", "--porcelain"],
        vec!["merge-base", "--is-ancestor", &left, "HEAD"],
        vec!["merge-base", "--is-ancestor", &right, "HEAD"],
    ] {
        let o = std::process::Command::new("git")
            .current_dir(&s.path)
            .args(&args)
            .output()
            .unwrap();
        rec.push(json!({"program":"git","args":args,"status":o.status.code(),"stdout":String::from_utf8(o.stdout).unwrap()}));
    }
    rec.push(
        serde_json::from_slice(
            &std::fs::read(root().join("tests/fixtures/run/gate-default/002-sysctl.json")).unwrap(),
        )
        .unwrap(),
    );
    rec.push(xcode_record("Fixture", "valid"));
    let out = s
        .kr()
        .env("KR_REPLAY", s.replay(rec))
        .env("KR_RESOURCES__BUILD", "1")
        .args([
            "merge",
            "check",
            "--base",
            "main",
            "--decision",
            id,
            "--verification-run",
            &run.to_string(),
            "--json",
        ])
        .assert()
        .code(if mode == "valid" { 0 } else { 1 })
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    if mode == "valid" || mode == "failed_run" {
        assert_eq!(v["reconciliation"]["supported"], mode == "valid");
        assert_eq!(v["reconciliation"]["tests"], 1);
    } else {
        assert!(v["reconciliation"].is_null());
    }
    let events = log
        .read_since("2026-01-01T00:00:00Z".parse().unwrap())
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == kr_events::EventKind::Reconciliation)
            .count(),
        if mode == "valid" || mode == "failed_run" {
            1
        } else {
            0
        }
    );
}

#[test]
fn xcstrings_same_key_delete_modify_and_metadata_disagreements_preserve_ours() {
    for kind in ["same_key", "delete_modify", "metadata"] {
        let s = Repo::new();
        let base =
            json!({"sourceLanguage":"en","version":"1.0","strings":{"A":{"comment":"base"}}});
        let mut ours = base.clone();
        let mut theirs = base.clone();
        match kind {
            "same_key" => {
                ours["strings"]["A"]["comment"] = json!("left");
                theirs["strings"]["A"]["comment"] = json!("right");
            }
            "delete_modify" => {
                ours["strings"] = json!({});
                theirs["strings"]["A"]["comment"] = json!("right");
            }
            _ => {
                ours["sourceLanguage"] = json!("fr");
                theirs["sourceLanguage"] = json!("de");
            }
        }
        for (n, d) in [("base", base), ("ours", ours), ("theirs", theirs)] {
            s.write(n, &d.to_string());
        }
        let before = std::fs::read(s.path.join("ours")).unwrap();
        s.kr()
            .env("KR_REPLAY", "")
            .args(["merge", "xcstrings", "base", "ours", "theirs", "--json"])
            .assert()
            .code(1);
        assert_eq!(
            before,
            std::fs::read(s.path.join("ours")).unwrap(),
            "{kind}"
        );
    }
}

#[test]
fn xcstrings_validation_failure_never_replaces_ours_or_leaves_a_temp_file() {
    let s = Repo::new();
    for n in ["base", "ours", "theirs"] {
        std::fs::copy(
            root().join(format!("tests/fixtures/merge/{n}.xcstrings")),
            s.path.join(n),
        )
        .unwrap();
    }
    let before = std::fs::read(s.path.join("ours")).unwrap();
    let replay=s.replay(vec![json!({"program":"xcrun","args":["xcstringstool","print","ours.kr-merge.xcstrings"],"status":1,"stdout":""})]);
    s.kr()
        .env("KR_REPLAY", replay)
        .args(["merge", "xcstrings", "base", "ours", "theirs", "--json"])
        .assert()
        .code(1);
    assert_eq!(before, std::fs::read(s.path.join("ours")).unwrap());
    assert!(!s.path.join("ours.kr-merge.xcstrings").exists());
}

#[test]
fn real_history_xcstrings_text_conflict_merges_without_dropping_entries() {
    let s = Repo::new();
    for n in ["base", "ours", "theirs"] {
        std::fs::copy(
            root().join(format!("tests/fixtures/merge/real-{n}.xcstrings")),
            s.path.join(format!("{n}.xcstrings")),
        )
        .unwrap();
    }
    let conflict = std::process::Command::new("git")
        .current_dir(&s.path)
        .args([
            "merge-file",
            "-p",
            "ours.xcstrings",
            "base.xcstrings",
            "theirs.xcstrings",
        ])
        .output()
        .unwrap();
    assert!(conflict.status.code().unwrap() > 0);
    let stdout =
        std::fs::read_to_string(root().join("tests/fixtures/merge/xcstringstool-print.txt"))
            .unwrap();
    let replay=s.replay(vec![json!({"program":"xcrun","args":["xcstringstool","print","ours.xcstrings.kr-merge.xcstrings"],"status":0,"stdout":stdout})]);
    s.kr()
        .env("KR_REPLAY", replay)
        .args([
            "merge",
            "xcstrings",
            "base.xcstrings",
            "ours.xcstrings",
            "theirs.xcstrings",
        ])
        .assert()
        .success();
    let merged: Value =
        serde_json::from_slice(&std::fs::read(s.path.join("ours.xcstrings")).unwrap()).unwrap();
    assert_eq!(merged["strings"].as_object().unwrap().len(), 8);
    for side in ["ours", "theirs"] {
        let original: Value = serde_json::from_slice(
            &std::fs::read(root().join(format!("tests/fixtures/merge/real-{side}.xcstrings")))
                .unwrap(),
        )
        .unwrap();
        for (key, entry) in original["strings"].as_object().unwrap() {
            assert_eq!(&merged["strings"][key], entry);
        }
    }
}

#[test]
fn quick_check_catches_the_original_batch6_duplicate_drop_in_upgrade_test() {
    let s = Repo::new();
    s.write(
        "LibraryGroupsTests.swift",
        &std::fs::read_to_string(root().join("tests/fixtures/merge/batch6-duplicate-drop.swift"))
            .unwrap(),
    );
    s.commit("batch6 recorded broken upgrade fixture");
    let out = s
        .kr()
        .args(["merge", "check", "--base", "HEAD~1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert!(
        v["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"] == "duplicate_drop"),
        "{v}"
    );
}

#[test]
fn quick_check_allows_recreated_tables_independent_fixtures_and_commented_sql() {
    let s = Repo::new();
    s.write(
        "Schema.swift",
        r#"
func upgrade() {
 db.execute(sql: "DROP TABLE old")
 db.execute(sql: "CREATE TABLE old (id INT)")
 db.execute(sql: "DROP TABLE old")
 // db.execute(sql: "DROP TABLE old")
 /* db.execute(sql: "DROP TABLE old") */
}
func anotherUpgrade() { db.execute(sql: "DROP TABLE old") }
"#,
    );
    s.commit("valid independent resets");
    let out = s
        .kr()
        .args(["merge", "check", "--base", "HEAD~1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert!(
        !v["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"] == "duplicate_drop"),
        "{v}"
    );
}

#[test]
fn quick_check_reports_invalid_json_and_credentials_without_printing_values() {
    let s = Repo::new();
    s.write("broken.json", "{\n");
    let value = ["only-a-synthetic-", "fixture-value"].concat();
    s.write(".env", &format!("PASSWORD={value}\n"));
    s.commit("synthetic scan fixture");
    let o = s
        .kr()
        .args(["merge", "check", "--base", "HEAD~1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(o).unwrap();
    assert!(!text.contains(&value));
    let v: Value = serde_json::from_str(&text).unwrap();
    let ps = v["problems"].as_array().unwrap();
    assert!(ps.iter().any(|p| p["kind"] == "invalid_json"));
    assert!(ps.iter().any(|p| p["kind"] == "credential"));
}

#[test]
fn predict_orders_stacked_branches_ancestor_first_without_asking_about_shared_commits() {
    let s = Repo::new();
    s.git(&["checkout", "-qb", "parent"]);
    s.write("seed", "parent\n");
    s.commit("#101 parent");
    s.git(&["checkout", "-qb", "child"]);
    s.write("seed", "child\n");
    s.commit("#102 child");
    let out = s
        .kr()
        .args([
            "merge", "predict", "--base", "main", "child", "parent", "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["suggested_order"], json!(["parent", "child"]));
    assert_eq!(v["decisions"], json!([]));
}

#[test]
fn resolution_is_reconciled_only_against_compiled_and_tested_merged_tree() {
    resolution_case("valid");
}
#[test]
fn resolution_rejects_tests_of_an_old_tree() {
    resolution_case("old_tree");
}
#[test]
fn resolution_rejects_a_zero_test_run() {
    resolution_case("zero");
}
#[test]
fn resolution_failed_tests_are_recorded_as_unsupported() {
    resolution_case("failed_run");
}

#[test]
fn quick_check_matches_registered_test_classes_without_requiring_a_name_suffix() {
    let s = Repo::new();
    s.write(".kr.toml","[project]\nname='Fixture'\n[simulators]\nruntime='iOS 27.1'\n[simulators.devices.default]\ndevice_type='iPhone 17'\n[scenarios.catalog]\nonly=['FixtureUITests/CatalogTests']\n");
    s.write(
        "FixtureUITests/CatalogTests.swift",
        "final class CatalogTests: XCTestCase {}\n",
    );
    s.commit("registered test class");
    let out = s
        .kr()
        .args(["merge", "check", "--base", "HEAD~1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert!(
        !v["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"].as_str().unwrap().starts_with("scenario_")),
        "{v}"
    );
}

#[test]
fn quick_check_detects_scenario_drift_and_migration_order() {
    let s = Repo::new();
    s.write(".kr.toml","[project]\nname='Fixture'\n[simulators]\nruntime='iOS 27.1'\n[simulators.devices.default]\ndevice_type='iPhone 17'\n[scenarios.missing]\nonly=['FixtureUITests/MissingTests']\n");
    s.write(
        "FixtureUITests/NewUITests.swift",
        "final class NewUITests: XCTestCase {}\n",
    );
    s.write("Schema.swift","m.registerMigration(\"Store.v3\") {}\nm.registerMigration(\"Store.v2\") {}\nm.registerMigration(\"Store.v2\") {}\n");
    s.commit("broken merge registry");
    let out = s
        .kr()
        .args(["merge", "check", "--base", "HEAD~1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    let ps = v["problems"].as_array().unwrap();
    for kind in [
        "scenario_missing_class",
        "scenario_unregistered",
        "migration_order",
        "duplicate_migration",
    ] {
        assert!(ps.iter().any(|p| p["kind"] == kind), "{kind}: {v}");
    }
}

#[test]
fn clean_json_and_credential_references_pass_without_a_native_build() {
    let s = Repo::new();
    s.write(
        "config.json",
        r#"{"password":"${CREDENTIAL_FILE}","api_key":"<configured-location>"}"#,
    );
    s.commit("credential locations only");
    let out = s
        .kr()
        .args(["merge", "check", "--base", "HEAD~1"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    insta::assert_snapshot!(String::from_utf8(out).unwrap(),@"Merge check: PASS (0 problem(s)).\n");
}

#[test]
fn clean_text_merge_of_same_file_still_needs_a_logical_decision() {
    let s = Repo::new();
    let lines = "first\n1\n2\n3\n4\n5\n6\n7\n8\nlast\n";
    s.write("shared", lines);
    s.commit("base lines");
    s.git(&["checkout", "-qb", "left"]);
    s.write("shared", &lines.replace("first", "LEFT"));
    s.commit("left");
    s.git(&["checkout", "-qb", "right", "main"]);
    s.write("shared", &lines.replace("last", "RIGHT"));
    s.commit("right");
    let out = s
        .kr()
        .args([
            "merge", "predict", "--base", "main", "left", "right", "--json",
        ])
        .assert()
        .code(4)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["pairs"][0]["text_conflict"], false);
    assert_eq!(
        v["decisions"][0]["evidence"]["left"]["issues_missing"],
        true
    );
}

#[test]
fn predict_suggests_clean_branch_before_a_conflicting_pair_and_rejects_invalid_refs() {
    let s = Repo::new();
    for (b, content) in [("left", "LEFT"), ("right", "RIGHT")] {
        s.git(&["checkout", "-qb", b, "main"]);
        s.write("seed", content);
        s.commit(b);
    }
    s.git(&["checkout", "-qb", "clean", "main"]);
    s.write("independent", "ok");
    s.commit("clean");
    let out = s
        .kr()
        .args([
            "merge", "predict", "--base", "main", "left", "right", "clean", "--json",
        ])
        .assert()
        .code(4)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["suggested_order"], json!(["clean", "left", "right"]));
    s.kr()
        .args(["merge", "predict", "main", "does-not-exist"])
        .assert()
        .code(1);
}

#[test]
fn git_driver_keeps_a_real_same_key_conflict_unmerged() {
    let s = Repo::new();
    s.kr().args(["merge", "install"]).assert().success();
    let base = json!({"sourceLanguage":"en","version":"1.0","strings":{"A":{"comment":"base"}}});
    s.write("Localizable.xcstrings", &base.to_string());
    s.commit("catalog and attributes");
    s.git(&["checkout", "-qb", "left"]);
    let mut left = base.clone();
    left["strings"]["A"]["comment"] = json!("left");
    s.write("Localizable.xcstrings", &left.to_string());
    s.commit("left");
    s.git(&["checkout", "-qb", "right", "main"]);
    let mut right = base;
    right["strings"]["A"]["comment"] = json!("right");
    s.write("Localizable.xcstrings", &right.to_string());
    s.commit("right");
    s.git(&["checkout", "left"]);
    let out = std::process::Command::new("git")
        .current_dir(&s.path)
        .env("HOME", s.home.path())
        .env("KR_STATE_DIR", s.home.path().join("state"))
        .env("KR_REPLAY", "")
        .args(["merge", "right"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        s.git(&["diff", "--name-only", "--diff-filter=U"]),
        "Localizable.xcstrings"
    );
    let actual: Value =
        serde_json::from_slice(&std::fs::read(s.path.join("Localizable.xcstrings")).unwrap())
            .unwrap();
    assert_eq!(actual, left);
}

#[test]
fn quick_check_allows_idempotent_drop_if_exists() {
    let s = Repo::new();
    s.write("Schema.swift","func cleanup() {\n db.execute(sql: \"DROP TABLE IF EXISTS old\")\n db.execute(sql: \"DROP TABLE IF EXISTS old\")\n}\n");
    s.commit("idempotent cleanup");
    let out = s
        .kr()
        .args(["merge", "check", "--base", "HEAD~1", "--json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert!(
        !v["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"] == "duplicate_drop"),
        "{v}"
    );
}

#[test]
fn xcstrings_rejects_duplicate_keys_instead_of_losing_a_translation() {
    let s = Repo::new();
    let bad = r#"{"sourceLanguage":"en","version":"1.0","strings":{"A":{"comment":"first"},"A":{"comment":"second"}}}"#;
    for name in ["base", "ours", "theirs"] {
        s.write(name, bad);
    }
    s.kr()
        .env("KR_REPLAY", "")
        .args(["merge", "xcstrings", "base", "ours", "theirs", "--json"])
        .assert()
        .code(1);
    assert_eq!(std::fs::read_to_string(s.path.join("ours")).unwrap(), bad);
    // Failure must be the malformed input, before asking for xcstringstool.
    let out = s
        .kr()
        .env("KR_REPLAY", "")
        .args(["merge", "xcstrings", "base", "ours", "theirs", "--json"])
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("duplicate"),
        "{v}"
    );
}

#[test]
fn quick_check_incrementally_builds_the_changed_module_with_recorded_xcode_output() {
    incremental_case("valid");
}
#[test]
fn incremental_build_failure_is_not_a_pass() {
    incremental_case("failed");
}
#[test]
fn incremental_build_timeout_is_not_a_pass() {
    incremental_case("timeout");
}
#[test]
fn changed_native_headers_also_require_incremental_compilation() {
    incremental_case("header");
}
#[test]
fn real_same_key_conflict_is_refused_without_changing_ours() {
    let s = Repo::new();
    for n in ["base", "ours", "theirs"] {
        std::fs::copy(
            root().join(format!("tests/fixtures/merge/same-key-{n}.xcstrings")),
            s.path.join(n),
        )
        .unwrap();
    }
    let before = std::fs::read(s.path.join("ours")).unwrap();
    s.kr()
        .env("KR_REPLAY", "")
        .args(["merge", "xcstrings", "base", "ours", "theirs", "--json"])
        .assert()
        .code(1);
    assert_eq!(before, std::fs::read(s.path.join("ours")).unwrap());
}
