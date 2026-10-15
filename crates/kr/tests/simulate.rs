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
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".local/state/kr/events");
        std::fs::create_dir_all(&events).unwrap();
        let mut text = String::new();
        for f in fixtures {
            text +=
                &std::fs::read_to_string(root().join("tests/fixtures/simulate").join(f)).unwrap();
        }
        std::fs::write(events.join("2026-10-14.ndjson"), text).unwrap();
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
