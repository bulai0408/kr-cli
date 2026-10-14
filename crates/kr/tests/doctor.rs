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
    std::fs::create_dir_all(home.path().join("repo/.git")).unwrap();
    std::fs::write(home.path().join("repo/.kr.toml"), project).unwrap();
    if let Some(machine) = machine {
        std::fs::create_dir_all(home.path().join(".config/kr")).unwrap();
        std::fs::write(home.path().join(".config/kr/config.toml"), machine).unwrap();
    }
    let f = fixtures();
    let replay = [
        "sysctl/m3-imac.json",
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
        .env("HOME", home.path())
        .env("NO_COLOR", "1")
        .env("KR_REPLAY", replay)
        .env("KR_APPLICATIONS", f.join("xcode/apps"))
        .current_dir(home.path().join("repo"))
        .arg("doctor");
    if json {
        cmd.arg("--json");
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout)
        .replace(&f.join("xcode/apps").display().to_string(), "<apps>")
        .replace(home.path().to_str().unwrap(), "<home>");
    (out.status.code(), text)
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
    insta::assert_json_snapshot!(json["capacity"]);
}
