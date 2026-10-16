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
        Command::from_std(self.process(replay))
    }

    fn process(&self, replay: String) -> std::process::Command {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("kr"));
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

#[test]
fn pool_lease_selects_a_free_device_by_name_or_udid() {
    const SECOND: &str = "DF280713-8C80-4056-8737-4E3B90B6B933";
    for (flag, value) in [("--name", "Krill-Verify-2"), ("--udid", SECOND)] {
        let repo = Repo::new();
        let replay = repo.replay(&[
            ("simctl/list-runtimes.json", "", ""),
            ("simctl/list-devices.json", "", ""),
            ("simctl/shutdown.json", RECORDED_UDID, SECOND),
            ("simctl/bootstatus.json", RECORDED_UDID, SECOND),
            ("simctl/shutdown.json", RECORDED_UDID, SECOND),
        ]);
        let out = repo
            .kr(replay)
            .args([
                "pool",
                "lease",
                flag,
                value,
                "--",
                "sh",
                "-c",
                "echo $KR_SIM_NAME $KR_SIM_UDID",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("Krill-Verify-2 {SECOND}\n")
        );
    }
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !ready() {
        assert!(
            start.elapsed().as_secs() < 15,
            "timed out waiting for lease"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_some() {
            return;
        }
        // Let kr forward termination to its command group before forcing exit.
        unsafe {
            libc::kill(self.0.id() as i32, libc::SIGTERM);
        }
        for _ in 0..100 {
            if self.0.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn pool_lease_waits_for_the_selected_device_and_queue_names_it() {
    for (flag, value, capacity) in [("--name", "Krill-Verify-1", "1"), ("--udid", KRILL_1, "2")] {
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
        let root = repo.home.path().join("repo");
        let mut owner = Child(
            repo.process(replay.clone()).env("KR_RESOURCES__SIM", capacity)
                .args(["pool", "lease", "--task", "current-owner", "--", "sh", "-c",
                    "touch ready; n=0; while [ ! -f release ] && [ $n -lt 400 ]; do sleep 0.05; n=$((n+1)); done"])
                .spawn().unwrap(),
        );
        wait_until(|| root.join("ready").exists());
        let stderr_path = root.join("waiting.log");
        let mut command = repo.process(replay.clone());
        command.env("KR_RESOURCES__SIM", capacity);
        if flag == "--udid" {
            command.arg("--json");
        }
        let mut waiter = Child(
            command
                .args([
                    "pool",
                    "lease",
                    flag,
                    value,
                    "--task",
                    "target-waiter",
                    "--",
                    "sh",
                    "-c",
                    "echo $KR_SIM_NAME $KR_SIM_UDID > selected",
                ])
                .stderr(std::fs::File::create(&stderr_path).unwrap())
                .spawn()
                .unwrap(),
        );
        wait_until(|| {
            std::fs::read_to_string(&stderr_path)
                .unwrap()
                .contains("current-owner")
        });
        assert!(
            !root.join("selected").exists(),
            "must not take another free device"
        );
        assert!(waiter.0.try_wait().unwrap().is_none());
        let stderr = std::fs::read_to_string(&stderr_path).unwrap();
        assert!(
            stderr.contains("Krill-Verify-1") && stderr.contains(KRILL_1),
            "{stderr}"
        );
        if flag == "--udid" {
            let notice: serde_json::Value = stderr
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find(|v| v["waiting"] == "pool_device")
                .unwrap();
            assert_eq!(notice["holder"]["task"], "current-owner");
            assert_eq!(
                notice["selection"],
                serde_json::json!({"by": "udid", "value": KRILL_1})
            );
        }
        let out = repo
            .kr(replay.clone())
            .env("KR_RESOURCES__SIM", capacity)
            .args(["queue", "sim", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let queue: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        wait_until(|| {
            let out = repo
                .kr(replay.clone())
                .env("KR_RESOURCES__SIM", capacity)
                .args(["queue", "sim", "--json"])
                .output()
                .unwrap();
            let queue: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            ["holders", "waiters"]
                .into_iter()
                .flat_map(|key| queue["resources"][0][key].as_array().unwrap())
                .any(|h| {
                    let task = h["task"].as_str().unwrap();
                    task.contains("target-waiter")
                        && task.contains("Krill-Verify-1")
                        && task.contains(KRILL_1)
                })
        });
        assert_eq!(
            queue["resources"][0]["capacity"],
            capacity.parse::<u32>().unwrap()
        );
        std::fs::write(root.join("release"), "").unwrap();
        wait_until(|| owner.0.try_wait().unwrap().is_some());
        wait_until(|| waiter.0.try_wait().unwrap().is_some());
        assert!(owner.0.wait().unwrap().success());
        assert!(waiter.0.wait().unwrap().success(), "{stderr}");
        assert_eq!(
            std::fs::read_to_string(root.join("selected")).unwrap(),
            format!("Krill-Verify-1 {KRILL_1}\n")
        );
    }
}

#[test]
fn pool_lease_help_explains_exact_device_selection() {
    let repo = Repo::new();
    let out = repo
        .kr(repo.replay(&[]))
        .args(["pool", "lease", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    insta::assert_snapshot!(String::from_utf8_lossy(&out.stdout));
}

#[test]
fn pool_lease_rejects_devices_outside_the_project_pool_before_queueing() {
    for (flag, value) in [
        ("--name", "iPhone 16 Pro"),
        ("--name", "Other-Verify-1"),
        ("--udid", "D4B96787-C994-476B-9379-6D1BFD205DAC"),
        ("--udid", "unknown"),
    ] {
        let repo = Repo::new();
        let replay = repo.replay(&[
            ("simctl/list-runtimes.json", "", ""),
            ("simctl/list-devices.json", "", ""),
        ]);
        let out = repo
            .kr(replay)
            .args(["pool", "lease", flag, value, "--", "true"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains(value) && err.contains("kr pool status"),
            "{err}"
        );
        assert!(!repo.home.path().join(".local/state/kr/resources").exists());
    }
}

#[test]
fn pool_lease_selectors_are_mutually_exclusive() {
    for args in [
        vec!["--name", "Krill-Verify-1", "--udid", KRILL_1],
        vec!["--name", "Krill-Verify-1", "--kind", "default"],
        vec!["--udid", KRILL_1, "--kind", "default"],
    ] {
        let repo = Repo::new();
        let out = repo
            .kr(repo.replay(&[]))
            .args(["pool", "lease"])
            .args(args)
            .args(["--", "true"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"));
    }
}

#[test]
fn pool_lease_json_notices_and_events_record_the_selection() {
    for (flag, value, by) in [
        ("--name", "Krill-Verify-1", "name"),
        ("--udid", KRILL_1, "udid"),
        ("--kind", "default", "kind"),
    ] {
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
                "--json",
                flag,
                value,
                "--",
                "sh",
                "-c",
                "echo child-output; exit 3",
            ])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(out.stdout, b"child-output\n");
        let notices: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|v: &serde_json::Value| v["type"] == "sim_leased")
            .collect();
        assert_eq!(notices.len(), 1, "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            notices[0]["selection"],
            serde_json::json!({"by": by, "value": value})
        );
        assert_eq!(notices[0]["device"], "Krill-Verify-1");
        assert_eq!(notices[0]["udid"], KRILL_1);
        let text: String = std::fs::read_dir(repo.home.path().join(".local/state/kr/events"))
            .unwrap()
            .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
            .collect();
        let events: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .filter(|v: &serde_json::Value| {
                matches!(v["type"].as_str(), Some("sim_leased" | "sim_released"))
            })
            .collect();
        assert_eq!(events.len(), 2);
        for event in events {
            assert_eq!(
                event["fields"]["selection"],
                serde_json::json!({"by": by, "value": value})
            );
        }
    }
}
