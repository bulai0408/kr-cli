use std::time::{Duration, Instant};

use kr_exec::{Command, RealRunner, Runner};

#[test]
fn dropping_a_background_service_stops_its_descendants_even_after_the_leader_exits() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("survived");
    let script = format!("(sleep 2; touch '{}') & exit 0", marker.display());
    let mut process = RealRunner
        .start(
            &Command::new("sh").args(["-c", &script]),
            &dir.path().join("service.log"),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(process.exited().unwrap());
    drop(process);
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !marker.exists(),
        "the descendant must not outlive its scope"
    );
}

#[test]
fn a_background_service_ignoring_term_is_killed_after_the_grace_period() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = format!(
        "trap '' TERM; echo $$ > '{}'; while :; do sleep 1; done",
        pid_file.display()
    );
    let mut process = RealRunner
        .start(
            &Command::new("sh").args(["-c", &script]),
            &dir.path().join("service.log"),
        )
        .unwrap();
    let started = Instant::now();
    while !pid_file.exists() && started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let started = Instant::now();
    process.stop().unwrap();
    assert!(started.elapsed() < Duration::from_secs(8));
    // SAFETY: probe only the child PID read from the command this test launched.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[test]
fn a_cleanup_command_has_one_hard_deadline_even_if_it_ignores_term() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new("sh")
        .args(["-c", "trap '' TERM; sleep 30"])
        .timeout(Duration::from_millis(150));
    command.cleanup = true;
    let started = Instant::now();
    let out = RealRunner
        .run_logged(&command, &dir.path().join("cleanup.log"))
        .unwrap();
    assert!(out.timed_out);
    assert!(!out.success());
    assert!(started.elapsed() < Duration::from_secs(2));
}
