use std::process::{Command, Stdio};

use jiff::Timestamp;
use kr_events::{Event, EventLog, RunId};
use serde_json::{Value, json};

fn lines(dir: &std::path::Path) -> Vec<Value> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for line in text.lines() {
            all.push(
                serde_json::from_str(line).unwrap_or_else(|e| panic!("bad line {line:?}: {e}")),
            );
        }
    }
    all
}

#[test]
fn append_writes_one_json_line_into_the_utc_day_file() {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::new(dir.path());
    let run = RunId::new();
    let ts: Timestamp = "2026-10-14T23:30:00+08:00".parse().unwrap();

    log.append(&Event::at(
        ts,
        run,
        Some("krill"),
        "slot_acquired",
        json!({"resource": "build", "slot": 1}),
    ))
    .unwrap();

    let text = std::fs::read_to_string(dir.path().join("2026-10-14.ndjson")).unwrap();
    let line: Value = serde_json::from_str(text.trim_end()).unwrap();
    assert!(text.ends_with('\n'));
    assert_eq!(line["ts"], "2026-10-14T15:30:00Z");
    assert_eq!(line["run_id"], run.to_string());
    assert_eq!(line["project"], "krill");
    assert_eq!(line["type"], "slot_acquired");
    assert_eq!(line["fields"], json!({"resource": "build", "slot": 1}));
}

#[test]
fn run_ids_sort_by_creation_time() {
    let a = RunId::new();
    let b = RunId::new();
    assert!(a.to_string() < b.to_string());
}

const WRITERS: usize = 8;
const EVENTS_PER_WRITER: usize = 200;

/// Child half of the multi-process test: runs only when spawned with KR_EVENTS_TEST_DIR set.
#[test]
fn concurrent_writer_child() {
    let Ok(dir) = std::env::var("KR_EVENTS_TEST_DIR") else {
        return;
    };
    let log = EventLog::new(dir);
    let run = RunId::new();
    let padding = "x".repeat(8 * 1024);
    for i in 0..EVENTS_PER_WRITER {
        log.append(&Event::now(
            run,
            Some("krill"),
            "tick",
            json!({"i": i, "padding": padding}),
        ))
        .unwrap();
    }
}

#[test]
fn concurrent_processes_never_interleave_lines() {
    let dir = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let children: Vec<_> = (0..WRITERS)
        .map(|_| {
            Command::new(&exe)
                .args(["--exact", "concurrent_writer_child", "--test-threads=1"])
                .env("KR_EVENTS_TEST_DIR", dir.path())
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }

    let events = lines(dir.path());
    assert_eq!(events.len(), WRITERS * EVENTS_PER_WRITER);
    let runs: std::collections::HashSet<_> =
        events.iter().map(|e| e["run_id"].to_string()).collect();
    assert_eq!(runs.len(), WRITERS);
}
