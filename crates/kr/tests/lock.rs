//! Multi-process tests of `kr lock` / `kr queue`: real processes compete for slots.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Machine {
    dir: tempfile::TempDir,
}

impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        Self { dir }
    }

    fn log(&self) -> PathBuf {
        self.dir.path().join("log")
    }

    fn kr(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kr"));
        cmd.env_clear()
            .env("HOME", self.dir.path())
            .env("PATH", std::env::var("PATH").unwrap())
            .env("KR_STATE_DIR", self.dir.path().join("state"))
            .env("NO_COLOR", "1")
            .current_dir(self.dir.path().join("work"));
        cmd
    }

    /// `kr lock <res> -- sh -c <script>`; the script sees $LOG.
    fn lock(&self, res: &str, capacity: u32, script: &str) -> Child {
        self.kr()
            .args(["lock", res, "--", "sh", "-c", script])
            .env(
                format!("KR_RESOURCES__{}", res.to_uppercase()),
                capacity.to_string(),
            )
            .env("LOG", self.log())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn wait_for_line(&self, line: &str) {
        wait_until(Duration::from_secs(10), || {
            self.lines().iter().any(|l| l == line)
        });
    }

    fn queue_json(&self) -> serde_json::Value {
        let out = self.kr().args(["queue", "--json"]).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn events(&self) -> Vec<serde_json::Value> {
        let dir = self.dir.path().join("state/events");
        let mut all = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            all.extend(text.lines().map(|l| serde_json::from_str(l).unwrap()));
        }
        all
    }
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "timed out after {limit:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn finish(mut child: Child, limit: Duration) -> i32 {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code().unwrap_or(-1);
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            panic!("kr did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn step(id: &str, secs: &str) -> String {
    format!("echo start {id} >> \"$LOG\"; sleep {secs}; echo end {id} >> \"$LOG\"")
}

/// Highest number of steps running at the same time, from the start/end log.
fn max_concurrency(lines: &[String]) -> usize {
    let (mut now, mut max) = (0usize, 0usize);
    for line in lines {
        if line.starts_with("start") {
            now += 1;
            max = max.max(now);
        } else if line.starts_with("end") {
            now -= 1;
        }
    }
    max
}

fn kill9(mut child: Child) {
    child.kill().unwrap();
    child.wait().unwrap();
}

fn kill_group(pgid: i64) {
    let _ = Command::new("kill")
        .args(["-9", &format!("-{pgid}")])
        .status();
}

#[test]
fn capacity_one_runs_strictly_one_at_a_time() {
    let m = Machine::new();
    let children: Vec<_> = (0..4)
        .map(|i| m.lock("build", 1, &step(&i.to_string(), "0.2")))
        .collect();
    for c in children {
        assert_eq!(finish(c, Duration::from_secs(20)), 0);
    }
    let lines = m.lines();
    assert_eq!(lines.len(), 8);
    assert_eq!(max_concurrency(&lines), 1, "{lines:?}");
}

#[test]
fn capacity_two_runs_at_most_two_at_a_time() {
    let m = Machine::new();
    let children: Vec<_> = (0..5)
        .map(|i| m.lock("sim", 2, &step(&i.to_string(), "0.4")))
        .collect();
    for c in children {
        assert_eq!(finish(c, Duration::from_secs(20)), 0);
    }
    let lines = m.lines();
    assert_eq!(lines.len(), 10);
    assert_eq!(max_concurrency(&lines), 2, "{lines:?}");
}

#[test]
fn waiters_get_the_slot_in_arrival_order() {
    let m = Machine::new();
    let first = m.lock("build", 1, &step("first", "1"));
    m.wait_for_line("start first");
    let mut rest = Vec::new();
    for id in ["b", "c", "d"] {
        rest.push(m.lock("build", 1, &step(id, "0.1")));
        // Make sure each one is queued before the next one arrives.
        let n = rest.len();
        wait_until(Duration::from_secs(5), || {
            m.queue_json()["resources"][0]["waiters"]
                .as_array()
                .unwrap()
                .len()
                == n
        });
    }
    for c in std::iter::once(first).chain(rest) {
        assert_eq!(finish(c, Duration::from_secs(20)), 0);
    }
    let starts: Vec<_> = m
        .lines()
        .into_iter()
        .filter(|l| l.starts_with("start"))
        .collect();
    assert_eq!(starts, ["start first", "start b", "start c", "start d"]);
}

#[test]
fn exit_code_of_the_command_is_passed_through() {
    let m = Machine::new();
    assert_eq!(
        finish(m.lock("light", 1, "exit 7"), Duration::from_secs(10)),
        7
    );
}

#[test]
fn slot_is_free_once_a_killed_holder_and_its_command_are_gone() {
    let m = Machine::new();
    let holder = m.lock("build", 1, "echo started >> \"$LOG\"; sleep 30");
    m.wait_for_line("started");
    let pgid = m.queue_json()["resources"][0]["holders"][0]["child_pid"]
        .as_i64()
        .unwrap();

    kill9(holder);
    kill_group(pgid);

    let start = Instant::now();
    assert_eq!(
        finish(m.lock("build", 1, "true"), Duration::from_secs(5)),
        0
    );
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "took {:?}",
        start.elapsed()
    );
}

#[test]
fn slot_stays_held_while_a_killed_holders_command_still_runs() {
    let m = Machine::new();
    let holder = m.lock("build", 1, "echo started >> \"$LOG\"; sleep 30");
    m.wait_for_line("started");
    let pgid = m.queue_json()["resources"][0]["holders"][0]["child_pid"]
        .as_i64()
        .unwrap();
    kill9(holder);

    let mut next = m.lock("build", 1, "echo next >> \"$LOG\"");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        next.try_wait().unwrap().is_none(),
        "must wait for the orphaned command"
    );
    let held = &m.queue_json()["resources"][0]["holders"][0];
    assert_eq!(held["orphan"], true, "{held}");

    kill_group(pgid);
    assert_eq!(finish(next, Duration::from_secs(5)), 0);
    assert!(m.lines().contains(&"next".to_owned()));
}

#[test]
fn nested_lock_in_the_holders_command_runs_without_queueing() {
    let m = Machine::new();
    let kr = env!("CARGO_BIN_EXE_kr");
    let inner = format!("{kr} lock build -- sh -c 'echo inner >> \"$LOG\"'");
    let outer = m.lock("build", 1, &inner);
    assert_eq!(finish(outer, Duration::from_secs(10)), 0);
    assert_eq!(m.lines(), ["inner"]);
}

#[test]
fn a_held_variable_from_a_non_ancestor_is_not_trusted() {
    let m = Machine::new();
    let holder = m.lock("build", 1, &step("holder", "1"));
    m.wait_for_line("start holder");

    let intruder = m
        .kr()
        .args([
            "lock",
            "build",
            "--",
            "sh",
            "-c",
            "echo start intruder >> \"$LOG\"",
        ])
        .env("KR_RESOURCES__BUILD", "1")
        .env("KR_HELD", "build:1")
        .env("LOG", m.log())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert_eq!(finish(intruder, Duration::from_secs(10)), 0);
    assert_eq!(finish(holder, Duration::from_secs(10)), 0);

    assert_eq!(m.lines(), ["start holder", "end holder", "start intruder"]);
    assert!(m.events().iter().any(|e| e["type"] == "reentry_denied"));
}

#[test]
fn events_record_queueing_acquiring_and_releasing() {
    let m = Machine::new();
    assert_eq!(
        finish(m.lock("light", 1, "true"), Duration::from_secs(10)),
        0
    );
    let types: Vec<_> = m
        .events()
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(types, ["queue_start", "slot_acquired", "slot_released"]);
    let released = &m.events()[2]["fields"];
    assert_eq!(released["resource"], "light");
    assert_eq!(released["exit_code"], 0);
    assert!(released["held_ms"].is_u64());
}

#[test]
fn sigterm_to_kr_is_forwarded_to_the_command_and_the_slot_is_returned() {
    let m = Machine::new();
    let holder = m.lock(
        "build",
        1,
        "trap 'echo got-term >> \"$LOG\"; exit 0' TERM; echo started >> \"$LOG\"; while true; do sleep 0.1; done",
    );
    m.wait_for_line("started");
    assert!(
        Command::new("kill")
            .args(["-TERM", &holder.id().to_string()])
            .status()
            .unwrap()
            .success()
    );

    assert_eq!(finish(holder, Duration::from_secs(5)), 0);
    assert!(m.lines().contains(&"got-term".to_owned()));
    assert!(
        m.queue_json()["resources"][0]["holders"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn queue_json_lists_holders_and_waiters_with_estimates() {
    let m = Machine::new();
    // History: one earlier build held the slot for about a second.
    assert_eq!(
        finish(m.lock("build", 1, "sleep 1"), Duration::from_secs(10)),
        0
    );
    let holder = m.lock("build", 1, &step("holder", "3"));
    m.wait_for_line("start holder");
    let waiter = m.lock("build", 1, "true");
    wait_until(Duration::from_secs(5), || {
        m.queue_json()["resources"][0]["waiters"]
            .as_array()
            .unwrap()
            .len()
            == 1
    });

    let mut json = m.queue_json();
    let build = &mut json["resources"][0];
    assert!(build["avg_hold_seconds"].as_u64().is_some());
    assert!(build["waiters"][0]["eta_seconds"].as_u64().is_some());
    for key in ["holders", "waiters"] {
        for item in build[key].as_array_mut().unwrap() {
            for field in [
                "pid",
                "child_pid",
                "started",
                "enqueued",
                "held_seconds",
                "waited_seconds",
                "eta_seconds",
            ] {
                if item.get(field).is_some_and(|v| !v.is_null()) {
                    item[field] = serde_json::json!(format!("<{field}>"));
                }
            }
        }
    }
    build["avg_hold_seconds"] = serde_json::json!("<avg>");
    insta::assert_json_snapshot!(json);

    for c in [holder, waiter] {
        assert_eq!(finish(c, Duration::from_secs(10)), 0);
    }
}
