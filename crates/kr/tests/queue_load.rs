//! Load wait visibility through the public CLI, with isolated events and processes.

use assert_cmd::prelude::*;
use jiff::{SignedDuration, Timestamp};
use kr_events::{Event, EventKind, EventLog, RunId};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Machine {
    dir: tempfile::TempDir,
}

impl Machine {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn kr(&self) -> Command {
        let mut cmd = Command::cargo_bin("kr").unwrap();
        cmd.env_clear()
            .env("HOME", self.dir.path())
            .env("PATH", std::env::var("PATH").unwrap())
            .env("KR_STATE_DIR", self.dir.path().join("state"))
            .env("KR_RESOURCES__BUILD", "1")
            .env("KR_RESOURCES__SIM", "1")
            .env("KR_RESOURCES__LIGHT", "1")
            .env("KR_REPLAY", fixture("m3-imac-overloaded"))
            .current_dir(self.dir.path());
        cmd
    }

    fn log(&self) -> EventLog {
        EventLog::new(self.dir.path().join("state/events"))
    }

    fn queue(&self) -> Value {
        let out = self.kr().args(["queue", "--json"]).assert().success();
        serde_json::from_slice(&out.get_output().stdout).unwrap()
    }

    fn human(&self) -> String {
        let out = self.kr().arg("queue").assert().success();
        String::from_utf8(out.get_output().stdout.clone()).unwrap()
    }

    fn wait_event(&self, run: RunId, kind: EventKind, resource: &str, pid: u32) {
        self.log()
            .append(&Event::at(
                Timestamp::now() - SignedDuration::from_secs(60),
                run,
                Some("example"),
                kind,
                json!({
                    "resource": resource, "task": "compile", "pid": pid,
                    "load": { "one": 80.0, "five": 60.0, "fifteen": 40.0 },
                    "pressure": "normal", "threshold": 32.0, "max_wait_seconds": 120,
                }),
            ))
            .unwrap();
    }
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/sysctl")
        .join(format!("{name}.json"))
}

#[test]
fn queue_shows_load_waiters_and_removes_acquired_or_dead_tasks() {
    let m = Machine::new();
    let run = RunId::new();
    let mut process = WaitingProcess(Command::new("sleep").arg("30").spawn().unwrap());
    let pid = process.0.id();
    m.wait_event(run, EventKind::LoadWait, "build", pid);
    let mut queue = m.queue();
    let waiters = queue["resources"][0]["load_waiters"]
        .as_array_mut()
        .expect("each resource includes load_waiters");
    assert_eq!(waiters.len(), 1);
    let w = &mut waiters[0];
    assert_eq!(w["task"], "compile");
    assert_eq!(w["pid"], pid);
    assert_eq!(w["load"]["one"], 99.0, "show current, not old load");
    assert_eq!(w["threshold"], 32.0);
    assert!((60..65).contains(&w["waited_seconds"].as_u64().unwrap()));
    assert!((55..=60).contains(&w["max_wait_remaining_seconds"].as_u64().unwrap()));
    for key in ["pid", "waited_seconds", "max_wait_remaining_seconds"] {
        w[key] = json!(format!("<{key}>"));
    }
    insta::assert_json_snapshot!(queue);
    insta::with_settings!({filters => vec![
        (r"pid \d+", "pid <pid>"),
        (r"waited [^·]+", "waited <duration> "),
        (r"max_wait remaining .+", "max_wait remaining <duration>"),
    ]}, {
        insta::assert_snapshot!(m.human());
    });

    m.wait_event(run, EventKind::SlotAcquired, "build", pid);
    assert_eq!(m.queue()["resources"][0]["load_waiters"], json!([]));
    assert!(!m.human().contains("waiting for load"));

    m.wait_event(run, EventKind::LoadWait, "build", pid);
    process.0.kill().unwrap();
    process.0.wait().unwrap();
    assert_eq!(m.queue()["resources"][0]["load_waiters"], json!([]));
    assert!(!m.human().contains("waiting for load"));
}

#[test]
fn load_waits_are_scoped_to_run_and_resource_and_end_when_load_is_ok() {
    let m = Machine::new();
    let run = RunId::new();
    let pid = std::process::id();
    m.wait_event(run, EventKind::LoadWait, "build", pid);
    m.wait_event(run, EventKind::LoadWait, "sim", pid);
    m.wait_event(RunId::new(), EventKind::SlotAcquired, "build", pid);
    assert_eq!(
        m.queue()["resources"][0]["load_waiters"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    m.wait_event(run, EventKind::LoadOk, "build", pid);
    let q = m.queue();
    assert_eq!(q["resources"][0]["load_waiters"], json!([]));
    assert_eq!(
        q["resources"][1]["load_waiters"].as_array().unwrap().len(),
        1
    );
}

struct WaitingProcess(Child);

impl Drop for WaitingProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn lock_records_both_load_wait_paths_and_only_suggests_configured_remotes() {
    for after_slot in [false, true] {
        for remote in [false, true] {
            let m = Machine::new();
            if remote {
                std::fs::create_dir_all(m.dir.path().join(".config/kr")).unwrap();
                std::fs::write(
                    m.dir.path().join(".config/kr/config.toml"),
                    "[remotes.work]\nhost = \"work-mac\"\n",
                )
                .unwrap();
            }
            let replay = if after_slot {
                format!(
                    "{}:{}:{}",
                    fixture("m3-imac").display(),
                    fixture("m3-imac-overloaded").display(),
                    fixture("m3-imac-overloaded").display()
                )
            } else {
                fixture("m3-imac-overloaded").display().to_string()
            };
            let stderr = std::fs::File::create(m.dir.path().join("stderr")).unwrap();
            let mut cmd = m.kr();
            let process = WaitingProcess(
                cmd.args(["lock", "build", "--task", "compile", "--", "true"])
                    .env("KR_LOAD__BLOCK", "true")
                    .env("KR_LOAD__MAX_WAIT", "1m")
                    .env("KR_REPLAY", replay)
                    .stdout(Stdio::null())
                    .stderr(stderr)
                    .spawn()
                    .unwrap(),
            );
            let start = Instant::now();
            loop {
                let events = m.log().read_since(Timestamp::UNIX_EPOCH).unwrap();
                let hint = std::fs::read_to_string(m.dir.path().join("stderr")).unwrap();
                if let Some(event) = events
                    .iter()
                    .find(|e| e.kind == EventKind::LoadWait && e.fields["after_slot"] == after_slot)
                {
                    if hint.contains("waiting before") || hint.contains("giving the slot back") {
                        assert_eq!(event.fields["task"], "compile");
                        assert_eq!(event.fields["pid"], process.0.id());
                        assert_eq!(event.fields["threshold"], 32.0);
                        assert!(event.fields["max_wait_seconds"].as_u64().unwrap() <= 60);
                        assert_eq!(hint.contains("kr run --remote work"), remote, "{hint}");
                        assert_eq!(
                            m.queue()["resources"][0]["load_waiters"][0]["pid"],
                            process.0.id()
                        );
                        break;
                    }
                }
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "{events:?}\n{hint}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
