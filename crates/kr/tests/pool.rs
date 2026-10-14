//! `kr pool` against recorded simctl outputs (KR_REPLAY).

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use kr_exec::Recording;

const PROJECT: &str = r#"
[project]
name = "Krill"

[simulators]
runtime = "iOS 18.6"

[simulators.devices.default]
device_type = "iPhone 16 Pro"
count = 3

[simulators.devices.iPad]
device_type = "iPad Pro 13-inch (M4) (16GB)"
"#;

const RECORDED_UDID: &str = "2A3B86EB-E206-4C43-9C35-0F8124258FB6";
const KRILL_1: &str = "6BFDF46B-1778-4C7E-B901-4E825CB78C4D";

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(rel)
}

struct Repo {
    home: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("repo/.git")).unwrap();
        std::fs::write(home.path().join("repo/.kr.toml"), PROJECT).unwrap();
        Self { home }
    }

    /// Writes recordings (fixture files, optionally with a UDID or name swapped) for KR_REPLAY.
    fn replay(&self, items: &[(&str, &str, &str)]) -> String {
        let dir = self.home.path().join("replay");
        std::fs::create_dir_all(&dir).unwrap();
        items
            .iter()
            .enumerate()
            .map(|(i, (rel, from, to))| {
                let mut r: Recording = kr_exec::read_recording(&fixture(rel)).unwrap();
                if !from.is_empty() {
                    for a in &mut r.args {
                        *a = a.replace(from, to);
                    }
                }
                let path = dir.join(format!("{i}.json"));
                std::fs::write(&path, serde_json::to_string(&r).unwrap()).unwrap();
                path.display().to_string()
            })
            .collect::<Vec<_>>()
            .join(":")
    }

    fn kr(&self, replay: String) -> Command {
        let mut cmd = Command::cargo_bin("kr").unwrap();
        cmd.env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var("PATH").unwrap())
            .env("NO_COLOR", "1")
            .env("KR_REPLAY", replay)
            .env("KR_APPLICATIONS", fixture("xcode/apps"))
            .env("KR_RESOURCES__SIM", "1")
            .current_dir(self.home.path().join("repo"));
        cmd
    }
}

#[test]
fn pool_status_lists_pool_devices_only() {
    let repo = Repo::new();
    let replay = repo.replay(&[
        ("simctl/list-runtimes.json", "", ""),
        ("simctl/list-devices.json", "", ""),
    ]);
    let out = repo.kr(replay).args(["pool", "--json"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    insta::assert_json_snapshot!(json);
}

#[test]
fn pool_sync_creates_the_missing_device() {
    let repo = Repo::new();
    let replay = repo.replay(&[
        ("simctl/list-runtimes.json", "", ""),
        ("simctl/list-devices.json", "", ""),
        ("simctl/create.json", "KrTest-Verify-2", "Krill-Verify-3"),
    ]);
    let out = repo.kr(replay).args(["pool", "sync"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    insta::assert_snapshot!(String::from_utf8_lossy(&out.stdout));
}

#[test]
fn pool_lease_runs_the_command_on_a_booted_pool_device() {
    let repo = Repo::new();
    let replay = repo.replay(&[
        ("simctl/list-runtimes.json", "", ""),
        ("simctl/list-devices.json", "", ""),
        (
            "simctl/shutdown-already-shutdown.json",
            RECORDED_UDID,
            KRILL_1,
        ),
        ("simctl/bootstatus.json", RECORDED_UDID, KRILL_1),
        ("simctl/shutdown.json", RECORDED_UDID, KRILL_1),
    ]);
    let out = repo
        .kr(replay)
        .args([
            "pool",
            "lease",
            "--",
            "sh",
            "-c",
            "echo $KR_SIM_NAME $KR_SIM_UDID; exit 3",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("Krill-Verify-1 {KRILL_1}\n")
    );

    let events = std::fs::read_dir(repo.home.path().join(".local/state/kr/events")).unwrap();
    let text: String = events
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect();
    let types: Vec<String> = text
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        types,
        [
            "queue_start",
            "slot_acquired",
            "sim_leased",
            "sim_released",
            "slot_released"
        ]
    );
}

#[test]
fn pool_lease_of_an_unknown_kind_explains_the_kinds() {
    let repo = Repo::new();
    let replay = repo.replay(&[]);
    let out = repo
        .kr(replay)
        .args(["pool", "lease", "--kind", "watch", "--", "true"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no simulator kind `watch`") && err.contains("default, iPad"),
        "{err}"
    );
}
