//! Stop signals (SIGINT, SIGTERM, SIGHUP) end the command that is running and every
//! later one. Its own test binary: the signal goes to the whole process.

use std::time::{Duration, Instant};

use kr_exec::{Command, RealRunner, Recording, ReplayRunner, Runner};

#[test]
fn a_sigterm_stops_a_replayed_command_and_every_later_one() {
    kr_exec::catch_stop_signals().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let runner = ReplayRunner::new(vec![Recording {
        program: "xcodebuild".into(),
        args: vec!["test-without-building".into()],
        status: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        duration_ms: Some(10_000),
    }]);
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(200));
        // SAFETY: plain kill(2) to this process.
        unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
    });

    let started = Instant::now();
    let err = runner
        .run_logged(
            &Command::new("xcodebuild").arg("test-without-building"),
            &dir.path().join("run.log"),
        )
        .unwrap_err();

    assert!(started.elapsed() < Duration::from_secs(5), "{err:#}");
    assert!(format!("{err:#}").contains("signal 15"), "{err:#}");
    assert_eq!(kr_exec::stop_signal(), Some(libc::SIGTERM));
    // A later command does not start at all.
    let marker = dir.path().join("ran");
    let later = RealRunner.run_logged(
        &Command::new("touch").arg(marker.display().to_string()),
        &dir.path().join("later.log"),
    );
    assert!(later.is_err());
    assert!(!marker.exists());
    let mut cleanup = Command::new("touch").arg(marker.display().to_string());
    cleanup.cleanup = true;
    assert!(
        RealRunner
            .run_logged(&cleanup, &dir.path().join("cleanup.log"))
            .unwrap()
            .success()
    );
    assert!(marker.exists(), "cleanup still runs after SIGTERM");
}
