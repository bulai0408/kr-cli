//! `kr doctor` against recorded outputs of this iMac (KR_REPLAY) and fake Xcode bundles.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

const PROJECT: &str = r#"
[project]
name = "Krill"
xcode = "27.1"

[simulators]
runtime = "iOS 18.6"

[simulators.devices.default]
device_type = "iPhone 16 Pro"
count = 3

[simulators.devices.iPad]
device_type = "iPad Pro 13-inch (M4) (16GB)"
"#;

fn doctor(project: &str, machine: Option<&str>, json: bool) -> (Option<i32>, String) {
    let home = tempfile::tempdir().unwrap();
    doctor_at(home.path(), project, machine, json)
}

fn doctor_at(
    home: &Path,
    project: &str,
    machine: Option<&str>,
    json: bool,
) -> (Option<i32>, String) {
    doctor_with_hardware(home, project, machine, json, "sysctl/m3-imac.json")
}

fn doctor_with_hardware(
    home: &Path,
    project: &str,
    machine: Option<&str>,
    json: bool,
    hardware: &str,
) -> (Option<i32>, String) {
    std::fs::create_dir_all(home.join("repo/.git")).unwrap();
    std::fs::write(home.join("repo/.kr.toml"), project).unwrap();
    if let Some(machine) = machine {
        std::fs::create_dir_all(home.join(".config/kr")).unwrap();
        std::fs::write(
            home.join(".config/kr/config.toml"),
            machine.replace("<home>", home.to_str().unwrap()),
        )
        .unwrap();
    }
    let f = fixtures();
    let replay = [
        hardware,
        "scutil/local-host-name.json",
        "df/root.json",
        "xcode-select/print-path.json",
        "simctl/list-runtimes.json",
        "simctl/list-devices.json",
    ]
    .map(|r| f.join(r).display().to_string())
    .join(":");
    let mut cmd = Command::cargo_bin("kr").unwrap();
    cmd.env_clear()
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .env("KR_REPLAY", replay)
        .env("KR_APPLICATIONS", f.join("xcode/apps"))
        .current_dir(home.join("repo"))
        .arg("doctor");
    if json {
        cmd.arg("--json");
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout)
        .replace(&f.join("xcode/apps").display().to_string(), "<apps>")
        .replace(home.to_str().unwrap(), "<home>");
    (out.status.code(), text)
}

#[test]
fn doctor_calibrates_the_work_mac_to_two_builds_without_hiding_config_overrides() {
    let home = tempfile::tempdir().unwrap();
    let (code, text) =
        doctor_with_hardware(home.path(), PROJECT, None, true, "sysctl/m4pro-work.json");
    assert_eq!(code, Some(0), "{text}");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["capacity"]["build"]["value"], 2);
    assert_eq!(json["capacity"]["build"]["source"], "suggested");
    let (code, text) = doctor_with_hardware(
        home.path(),
        PROJECT,
        Some("[resources]\nbuild = 1\nsim = 2\n"),
        true,
        "sysctl/m4pro-work.json",
    );
    assert_eq!(code, Some(0), "{text}");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["capacity"]["build"]["value"], 1);
    assert_eq!(json["capacity"]["sim"]["value"], 2);
    assert_eq!(json["suggested_capacity"]["build"]["value"], 2);
    let (_, text) =
        doctor_with_hardware(home.path(), PROJECT, None, false, "sysctl/m4pro-work.json");
    assert!(text.contains("hardware suggestion"), "{text}");
}

#[test]
fn doctor_remembers_non_kr_legacy_owners_after_they_release_the_lock() {
    let home = tempfile::tempdir().unwrap();
    let lock = home.path().join("native.lock");
    std::fs::create_dir(&lock).unwrap();
    std::fs::write(
        lock.join("owner"),
        format!(
            "krill-e2e-smoke orca {} 2026-10-16T00:00:00Z\n",
            std::process::id()
        ),
    )
    .unwrap();
    let machine = "[legacy]\nmachine_lock = '<home>/native.lock'\nqueue = '<home>/native.queue'\n";
    let (code, text) = doctor_at(home.path(), PROJECT, Some(machine), true);
    assert_eq!(code, Some(0), "{text}");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        json["legacy"]["recent_non_kr_owners"][0]["owner"]["task"],
        "krill-e2e-smoke"
    );
    assert_eq!(json["legacy"]["can_disable"], false);
    std::fs::remove_file(lock.join("owner")).unwrap();
    std::fs::remove_dir(&lock).unwrap();
    let (_, text) = doctor_at(home.path(), PROJECT, Some(machine), false);
    assert!(text.contains("krill-e2e-smoke"), "{text}");
    assert!(text.contains("still in use"), "{text}");
    // A recently exited old script leaves evidence too; ignoring its stale owner
    // would incorrectly make an unobserved legacy entry point disappear.
    let mut exited = std::process::Command::new("true").spawn().unwrap();
    exited.wait().unwrap();
    std::fs::create_dir(&lock).unwrap();
    std::fs::write(
        lock.join("owner"),
        format!("conflux-build shell {} now\n", exited.id()),
    )
    .unwrap();
    let (_, text) = doctor_at(home.path(), PROJECT, Some(machine), true);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        json["legacy"]["recent_non_kr_owners"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["owner"]["task"] == "conflux-build"),
        "{text}"
    );
}

#[test]
fn doctor_excludes_kr_owned_locks_and_explains_the_observation_limit() {
    let home = tempfile::tempdir().unwrap();
    let lock = home.path().join("native.lock");
    std::fs::create_dir(&lock).unwrap();
    std::fs::write(
        lock.join("owner"),
        format!(
            "kr-build kr:orca {} 2026-10-16T00:00:00Z\n",
            std::process::id()
        ),
    )
    .unwrap();
    let machine = "[legacy]\nmachine_lock = '<home>/native.lock'\nqueue = '<home>/native.queue'\n";
    let (_, text) = doctor_at(home.path(), PROJECT, Some(machine), true);
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        json["legacy"]["recent_non_kr_owners"],
        serde_json::json!([])
    );
    assert_eq!(json["legacy"]["can_disable"], true);
    assert!(
        json["legacy"]["next"]
            .as_str()
            .unwrap()
            .contains("migration checklist")
    );
    let (_, text) = doctor_at(home.path(), PROJECT, Some(machine), false);
    assert!(text.contains("can remove [legacy]"), "{text}");
    assert!(text.contains("observations only"), "{text}");
}

#[test]
fn doctor_shows_profile_capacity_and_pool_problems() {
    let (code, text) = doctor(PROJECT, None, false);
    // Krill-Verify-3 lives on iOS 27.0, so the iOS 18.6 pool misses it: a warning only.
    assert_eq!(code, Some(0), "{text}");
    insta::assert_snapshot!(text);
}

#[test]
fn doctor_fails_when_the_required_xcode_is_missing() {
    let (code, text) = doctor(&PROJECT.replace("27.1", "28"), None, false);
    assert_eq!(code, Some(1));
    assert!(text.contains("Xcode 28 is required"), "{text}");
}

#[test]
fn doctor_json_says_where_each_capacity_comes_from() {
    let machine = "[resources]\nbuild = 1\n\n[machines.iMac.resources]\nsim = 3\n";
    let (code, text) = doctor(PROJECT, Some(machine), true);
    assert_eq!(code, Some(0));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let cap = &json["capacity"];
    assert_eq!(cap["build"]["value"], 1);
    assert_eq!(cap["build"]["source"], "config: [resources]");
    assert_eq!(cap["sim"]["value"], 3);
    assert_eq!(cap["sim"]["source"], "config: [machines.iMac]");
    assert_eq!(cap["light"]["value"], 1);
    assert_eq!(cap["light"]["source"], "suggested");
    assert_eq!(json["problems"][0]["code"], "pool_incomplete");
    assert_eq!(json["xcode"]["selected"]["version"], "27.1");
    assert_eq!(json["suggested_capacity"]["build"]["value"], 1);
    assert_eq!(json["suggested_capacity"]["sim"]["value"], 2);
    // Every --json document is versioned and has the decision points (none here).
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["decisions"], serde_json::json!([]));
    insta::assert_json_snapshot!(json["capacity"]);
}
