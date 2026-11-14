//! `kr simulate` on the two recorded runs of #9 (tests/fixtures/simulate): the kr gate
//! on Krill and the old e2e.sh run of batch 6 on the same code tree.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

const KR_GATE: &str = "01a13972-e967-759e-b387-d49fbf89cf37";
const BATCH6: &str = "01a13900-0000-7000-8000-00000000b606";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    /// A home whose event log holds the given fixtures.
    fn with(fixtures: &[&str]) -> Self {
        let mut text = String::new();
        for f in fixtures {
            text +=
                &std::fs::read_to_string(root().join("tests/fixtures/simulate").join(f)).unwrap();
        }
        Self::with_events("2026-10-14", &text)
    }

    /// A home whose event log for `day` holds `text`.
    fn with_events(day: &str, text: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".local/state/kr/events");
        std::fs::create_dir_all(&events).unwrap();
        std::fs::write(events.join(format!("{day}.ndjson")), text).unwrap();
        Self { dir }
    }

    fn kr(&self, args: &[&str]) -> (Option<i32>, String) {
        let out = Command::cargo_bin("kr")
            .unwrap()
            .env_clear()
            .env("HOME", self.dir.path())
            .env("NO_COLOR", "1")
            .current_dir(self.dir.path())
            .args(args)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.code(), text)
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut all = vec!["--json"];
        all.extend(args);
        let (code, out) = self.kr(&all);
        assert_eq!(code, Some(0), "{out}");
        serde_json::from_str(&out).unwrap()
    }
}

fn both() -> Home {
    Home::with(&["krill-batch6-e2e.ndjson", "krill-p0-kr-gate.ndjson"])
}

fn minutes(ms: &serde_json::Value) -> f64 {
    ms.as_f64().unwrap() / 60_000.0
}

#[test]
fn the_kr_gate_replays_within_ten_percent_of_its_measured_time() {
    let v = both().json(&["simulate", KR_GATE]);
    assert_eq!(v["schema_version"], 1);
    let run = &v["calibration"]["runs"][0];
    assert_eq!(run["run_id"], KR_GATE);
    // First simulator item start to last end, 08:35:54 → 09:21:52.
    assert!((minutes(&run["measured_ms"]) - 46.0).abs() < 0.1, "{run}");
    assert_eq!(run["capacity"], 2);
    assert_eq!(
        run["lanes"],
        serde_json::json!(["default", "iPad", "default"])
    );
    assert!(run["error"].as_f64().unwrap().abs() <= 0.10, "{run}");
}

#[test]
fn the_batch6_script_run_replays_within_ten_percent_of_its_measured_time() {
    let v = both().json(&["simulate", BATCH6]);
    let run = &v["calibration"]["runs"][0];
    assert!((minutes(&run["measured_ms"]) - 44.4).abs() < 0.1, "{run}");
    assert_eq!(
        run["lanes"],
        serde_json::json!(["default", "default", "iPad"])
    );
    assert!(run["error"].as_f64().unwrap().abs() <= 0.10, "{run}");
}

#[test]
fn both_runs_together_rank_like_the_measurements() {
    // Measured: batch 6 (44.4 min) was faster than the kr gate (46.0 min).
    let v = both().json(&["simulate", BATCH6, KR_GATE]);
    let c = &v["calibration"];
    assert_eq!(c["runs"].as_array().unwrap().len(), 2);
    assert!(c["mape"].as_f64().unwrap() <= 0.10, "{c}");
    assert_eq!(c["ranking_matches"], true, "{c}");
}

#[test]
fn ipad_first_beats_the_old_lane_order_on_batch6() {
    let v = both().json(&[
        "simulate",
        BATCH6,
        "--order",
        "ipad-first",
        "--order",
        "name",
    ]);
    let settings = v["settings"].as_array().unwrap();
    assert_eq!(settings.len(), 2);
    let p50 = |order: &str| {
        let s = settings.iter().find(|s| s["order"] == order).unwrap();
        assert!(s["lower_bound_ms"].as_u64() <= s["p50_ms"].as_u64(), "{s}");
        assert!(s["p50_ms"].as_u64() <= s["p90_ms"].as_u64(), "{s}");
        minutes(&s["p50_ms"])
    };
    let (ipad, name) = (p50("ipad-first"), p50("name"));
    assert!(ipad < name - 3.0, "ipad-first {ipad:.1} vs name {name:.1}");
}

#[test]
fn a_capacity_the_runs_did_not_use_is_outside_calibration() {
    let v = both().json(&["simulate", KR_GATE, "--sim", "2", "--sim", "3"]);
    let settings = v["settings"].as_array().unwrap();
    assert!(
        settings[0]["outside_calibration"].is_null(),
        "{}",
        settings[0]
    );
    let note = settings[1]["outside_calibration"].as_str().unwrap();
    assert!(note.contains("sim=3") && note.contains("sim=2"), "{note}");
}

#[test]
fn samples_from_several_runs_spread_p50_and_p90() {
    let v = both().json(&["simulate", BATCH6, KR_GATE]);
    let s = &v["settings"][0];
    assert!(s["p50_ms"].as_u64() < s["p90_ms"].as_u64(), "{s}");
}

#[test]
fn the_human_report_gives_calibration_then_one_line_per_setting() {
    let (code, out) = both().kr(&["simulate", KR_GATE, "--sim", "2", "--sim", "3"]);
    assert_eq!(code, Some(0), "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn no_recorded_run_says_what_to_do() {
    let home = Home::with(&[]);
    let (code, out) = home.kr(&["simulate"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("no run with simulator items"), "{out}");
    assert!(out.contains("kr gate"), "{out}");
}

#[test]
fn a_cache_miss_needs_a_recorded_build_time() {
    let (code, out) = both().kr(&["simulate", KR_GATE, "--cache", "miss"]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("no build step"), "{out}");
}

#[test]
fn a_cache_miss_adds_the_recorded_build_time() {
    let home = both();
    // A build of ten minutes in the kr gate's run.
    let build = serde_json::json!({
        "ts": "2026-10-14T08:06:30Z", "run_id": KR_GATE, "project": "Krill", "type": "step_end",
        "fields": { "command": "run", "step": "build build-for-testing Krill (generic/platform=iOS Simulator)", "duration_ms": 600_000, "ok": true }
    });
    let events = home
        .dir
        .path()
        .join(".local/state/kr/events/2026-10-14.ndjson");
    let text = std::fs::read_to_string(&events).unwrap() + &build.to_string() + "\n";
    std::fs::write(&events, text).unwrap();
    let v = home.json(&["simulate", KR_GATE, "--cache", "hit", "--cache", "miss"]);
    let s = v["settings"].as_array().unwrap();
    let diff = s[1]["p50_ms"].as_u64().unwrap() - s[0]["p50_ms"].as_u64().unwrap();
    assert_eq!(diff, 600_000, "{v}");
}

// `kr simulate --build N` (#65): the build requests of the event log, replayed under
// kr's FIFO slot rule.

/// Three hours of Conflux build and sim requests, 2026-11-13 04:00–07:00 UTC (README).
fn conflux() -> Home {
    let text =
        std::fs::read_to_string(root().join("tests/fixtures/simulate/conflux-build-queue.ndjson"))
            .unwrap();
    Home::with_events("2026-11-13", &text)
}

const CONFLUX_SINCE: &str = "2026-11-13T00:00:00Z";

fn assert_ms(v: &serde_json::Value, seconds: f64) {
    let ms = v.as_f64().unwrap();
    assert!(
        (ms - seconds * 1000.0).abs() < 100.0,
        "{ms} ms, expected {seconds} s"
    );
}

fn assert_share(v: &serde_json::Value, share: f64) {
    assert!(
        (v.as_f64().unwrap() - share).abs() < 0.0005,
        "{v}, expected {share}"
    );
}

/// One request of `task` on `resource` at minute `at`: it waited `waited` and held the
/// slot `held` minutes. With `lock`, it took the old machine-wide lock right at arrival.
fn request(resource: &str, task: &str, at: u64, waited: u64, held: u64, lock: bool) -> String {
    let ts = |minute: u64| format!("2026-11-01T{:02}:{:02}:00Z", minute / 60, minute % 60);
    let run = format!(
        "01a1d000-0000-7000-8000-{:012}",
        at * 1000 + task.len() as u64
    );
    let event = |minute: u64, kind: &str, fields: serde_json::Value| {
        serde_json::json!({ "ts": ts(minute), "run_id": run, "project": "Conflux", "type": kind, "fields": fields })
            .to_string()
            + "\n"
    };
    let mut text = event(
        at,
        "queue_start",
        serde_json::json!({ "resource": resource, "task": task, "capacity": 1 }),
    );
    if lock {
        text += &event(
            at + waited,
            "machine_lock",
            serde_json::json!({ "waited_ms": waited * 60_000, "held_by_ancestor": false, "shared_in_process": false }),
        );
    }
    text += &event(
        at + waited,
        "slot_acquired",
        serde_json::json!({ "resource": resource, "task": task, "waited_ms": waited * 60_000 }),
    );
    text += &event(
        at + waited + held,
        "slot_released",
        serde_json::json!({ "resource": resource, "task": task, "held_ms": held * 60_000 }),
    );
    text
}

const NOV1: &str = "2026-11-01T00:00:00Z";

#[test]
fn the_recorded_capacity_replays_the_measured_waits_of_a_single_queue() {
    // A, B, C, D arrive at minutes 0, 1, 2, 3 and hold 10, 10, 5, 1 minutes. With one
    // slot they start at 0, 10, 20, 25: waits 0, 9, 18, 22 = 49 minutes.
    let text = request("build", "a", 0, 0, 10, false)
        + &request("build", "b", 1, 9, 10, false)
        + &request("build", "c", 2, 18, 5, false)
        + &request("build", "d", 3, 22, 1, false);
    let v = Home::with_events("2026-11-01", &text)
        .json(&["simulate", "--build", "1", "--build", "2", "--since", NOV1]);
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["requests"]["build"], 4);
    let c = &v["calibration"];
    assert_ms(&c["measured"]["queued_ms"], 49.0 * 60.0);
    assert_ms(&c["replayed"]["queued_ms"], 49.0 * 60.0);
    assert_eq!(c["error"], 0.0, "{c}");
    assert_eq!(c["capacities"], serde_json::json!([1]));
    // Held 26 minutes in all: 49 ÷ (49 + 26).
    assert_share(&c["measured"]["queue_share"], 49.0 / 75.0);
    let s = v["settings"].as_array().unwrap();
    assert_eq!(s[0]["build"], 1);
    assert_ms(&s[0]["queued_ms"], 49.0 * 60.0);
    assert!(s[0]["outside_calibration"].is_null(), "{}", s[0]);
    // Two slots: A [0, 10], B [1, 11], C waits for A [10, 15], D for B [11, 12]: 8 + 8.
    assert_eq!(s[1]["build"], 2);
    assert_ms(&s[1]["queued_ms"], 16.0 * 60.0);
    // Nearest rank of 0, 0, 8, 8 minutes.
    assert_ms(&s[1]["p50_wait_ms"], 0.0);
    assert_ms(&s[1]["p90_wait_ms"], 8.0 * 60.0);
    assert!(
        s[1]["outside_calibration"]
            .as_str()
            .unwrap()
            .contains("build=1"),
        "{}",
        s[1]
    );
}

#[test]
fn a_slowdown_stretches_the_builds_that_start_beside_another() {
    // Two slots, 1.5×: B starts beside A and takes 15 minutes [1, 16]; C starts at 10
    // beside B and takes 7.5 [10, 17.5]; D waits for B until 16: 8 + 13 = 21 minutes.
    let text = request("build", "a", 0, 0, 10, false)
        + &request("build", "b", 1, 9, 10, false)
        + &request("build", "c", 2, 18, 5, false)
        + &request("build", "d", 3, 22, 1, false);
    let home = Home::with_events("2026-11-01", &text);
    let v = home.json(&[
        "simulate",
        "--build",
        "1",
        "--build",
        "2",
        "--since",
        NOV1,
        "--slowdown",
        "1.5",
    ]);
    assert_eq!(v["slowdown"], 1.5);
    let s = v["settings"].as_array().unwrap();
    // One slot: no build runs beside another, so nothing slows down.
    assert_ms(&s[0]["queued_ms"], 49.0 * 60.0);
    assert_ms(&s[1]["queued_ms"], 21.0 * 60.0);
    // The recorded capacity is replayed as recorded, without the slowdown.
    assert_ms(&v["calibration"]["replayed"]["queued_ms"], 49.0 * 60.0);
}

#[test]
fn the_calibration_replays_the_old_machine_lock_that_sims_shared() {
    // A sim lease takes the machine lock at minute 0 for 10 minutes; a build arrives at
    // minute 1, waits for the lock 9 minutes, and builds 5.
    let text =
        request("sim", "run Conflux lane", 0, 0, 10, true) + &request("build", "b", 1, 9, 5, true);
    let v =
        Home::with_events("2026-11-01", &text).json(&["simulate", "--build", "1", "--since", NOV1]);
    assert_eq!(v["requests"]["build"], 1);
    assert_eq!(v["requests"]["bridged"], 1);
    let c = &v["calibration"];
    assert_ms(&c["measured"]["queued_ms"], 9.0 * 60.0);
    assert_ms(&c["replayed"]["queued_ms"], 9.0 * 60.0);
    // Without the machine lock, the one build has the build slot to itself.
    assert_ms(&v["settings"][0]["queued_ms"], 0.0);
}

#[test]
fn requests_before_since_and_without_an_end_are_left_out() {
    let mut text =
        request("build", "early", 0, 0, 10, false) + &request("build", "late", 20, 0, 10, false);
    // Queued at minute 40 and never got a slot (killed while waiting).
    text += &serde_json::json!({
        "ts": "2026-11-01T00:40:00Z", "run_id": "01a1d000-0000-7000-8000-00000000dead",
        "project": "Conflux", "type": "queue_start",
        "fields": { "resource": "build", "task": "killed", "capacity": 1 }
    })
    .to_string();
    text += "\n";
    let v = Home::with_events("2026-11-01", &text).json(&[
        "simulate",
        "--build",
        "1",
        "--since",
        "2026-11-01T00:15:00Z",
    ]);
    assert_eq!(v["requests"]["build"], 1);
    assert_eq!(v["requests"]["unfinished"], 1);
}

#[test]
fn conflux_build_queue_replays_like_an_independent_count() {
    // Expected values from a separate script over the same fixture (see its README).
    let v = conflux().json(&[
        "simulate",
        "--build",
        "1",
        "--build",
        "2",
        "--build",
        "3",
        "--since",
        CONFLUX_SINCE,
    ]);
    assert_eq!(v["requests"]["build"], 62);
    assert_eq!(v["requests"]["unfinished"], 1);
    assert_eq!(v["requests"]["bridged"], 62);
    let c = &v["calibration"];
    assert_ms(&c["measured"]["queued_ms"], 5612.1);
    assert_share(&c["measured"]["queue_share"], 0.5135);
    assert_ms(&c["measured"]["p90_wait_ms"], 330.1);
    assert_ms(&c["replayed"]["queued_ms"], 3837.7);
    assert!(
        (c["error"].as_f64().unwrap() - (3837.7 / 5612.1 - 1.0)).abs() < 0.001,
        "{c}"
    );
    let s = v["settings"].as_array().unwrap();
    assert_ms(&s[0]["queued_ms"], 3361.2);
    assert_ms(&s[0]["p90_wait_ms"], 205.0);
    assert_ms(&s[1]["queued_ms"], 223.0);
    assert_ms(&s[2]["queued_ms"], 0.0);
}

#[test]
fn conflux_build_queue_with_a_slowdown() {
    let v = conflux().json(&[
        "simulate",
        "--build",
        "2",
        "--since",
        CONFLUX_SINCE,
        "--slowdown",
        "1.5",
    ]);
    assert_ms(&v["settings"][0]["queued_ms"], 448.7);
    assert_share(&v["settings"][0]["queue_share"], 0.0673);
}

#[test]
fn the_human_build_report_says_the_default_slowdown_is_a_lower_bound() {
    let (code, out) = conflux().kr(&[
        "simulate",
        "--build",
        "1",
        "--build",
        "2",
        "--since",
        CONFLUX_SINCE,
    ]);
    assert_eq!(code, Some(0), "{out}");
    insta::assert_snapshot!(out);
}

#[test]
fn build_replay_does_not_mix_with_the_sim_settings() {
    let (code, out) = conflux().kr(&["simulate", "--build", "2", "--sim", "2"]);
    assert_eq!(code, Some(2), "{out}");
    assert!(out.contains("--sim"), "{out}");
}
