use std::time::{Duration, Instant};

use kr_exec::{Command, RealRunner, Recording, RecordingRunner, ReplayRunner, Runner, Sanitizer};

#[test]
fn a_logged_run_streams_stdout_and_stderr_into_the_log_and_keeps_a_tail() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("logs/build.log");
    let script = "for i in $(seq 1 12000); do echo line $i; done; echo oops >&2; exit 65";

    let out = RealRunner
        .run_logged(&Command::new("sh").args(["-c", script]), &log)
        .unwrap();

    assert_eq!(out.status, Some(65));
    assert!(!out.timed_out);
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.starts_with("line 1\n") && text.contains("line 12000\n") && text.contains("oops"));
    assert!(
        out.stdout.len() < text.len(),
        "only a tail is kept in memory"
    );
    assert!(
        out.stdout.ends_with("oops\n"),
        "{:?}",
        &out.stdout[out.stdout.len() - 20..]
    );
}

#[test]
fn a_logged_run_past_its_timeout_kills_the_whole_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("t.log");
    let marker = dir.path().join("grandchild-survived");
    // A grandchild that would write the marker if it outlived the timeout.
    let script = format!("(sleep 2; touch {}) & sleep 30", marker.display());
    let start = Instant::now();

    let out = RealRunner
        .run_logged(
            &Command::new("sh")
                .args(["-c", &script])
                .timeout(Duration::from_millis(300)),
            &log,
        )
        .unwrap();

    assert!(out.timed_out);
    assert!(!out.success());
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!marker.exists(), "the grandchild must be killed too");
}

#[test]
fn a_replayed_logged_run_writes_the_recorded_output_to_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("r.log");
    let runner = ReplayRunner::new(vec![Recording {
        program: "xcodebuild".into(),
        args: vec!["build".into()],
        status: Some(65),
        stdout: "A.swift:1:1: error: boom\n".into(),
        stderr: "** BUILD FAILED **\n".into(),
        timed_out: false,
        duration_ms: None,
    }]);

    let out = runner
        .run_logged(&Command::new("xcodebuild").arg("build"), &log)
        .unwrap();

    assert_eq!(out.status, Some(65));
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "A.swift:1:1: error: boom\n** BUILD FAILED **\n"
    );
}

#[test]
fn the_recording_runner_saves_each_call_sanitized_and_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let home = std::env::var("HOME").unwrap();
    let user = std::env::var("USER").unwrap_or_default();
    let recorder = RecordingRunner::new(RealRunner, dir.path(), Sanitizer::new(home.clone(), user));

    recorder
        .run(&Command::new("sh").args(["-c", "echo $HOME"]))
        .unwrap();
    recorder
        .run_logged(&Command::new("echo").arg("two"), &dir.path().join("x.log"))
        .unwrap();

    let first: Recording = kr_exec::read_recording(&dir.path().join("001-sh.json")).unwrap();
    assert_eq!(first.stdout, "~\n");
    let second: Recording = kr_exec::read_recording(&dir.path().join("002-echo.json")).unwrap();
    assert_eq!(second.stdout, "two\n");
}
