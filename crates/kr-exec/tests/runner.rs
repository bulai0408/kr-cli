use kr_exec::{Command, RealRunner, Recording, ReplayRunner, Runner, Sanitizer};

fn recording(program: &str, args: &[&str], status: i32, stdout: &str) -> Recording {
    Recording {
        program: program.into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        status: Some(status),
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

#[test]
fn replay_returns_the_recorded_output_for_a_matching_command() {
    let runner = ReplayRunner::new(vec![recording(
        "xcode-select",
        &["-p"],
        0,
        "/Applications/Xcode.app/Contents/Developer\n",
    )]);

    let out = runner.run(&Command::new("xcode-select").arg("-p")).unwrap();

    assert!(out.success());
    assert_eq!(out.stdout, "/Applications/Xcode.app/Contents/Developer\n");
}

#[test]
fn replay_returns_repeated_calls_in_recorded_order() {
    let runner = ReplayRunner::new(vec![
        recording("xcrun", &["simctl", "boot", "A"], 149, "first"),
        recording("xcrun", &["simctl", "boot", "A"], 0, "second"),
    ]);
    let cmd = Command::new("xcrun").args(["simctl", "boot", "A"]);

    assert_eq!(runner.run(&cmd).unwrap().stdout, "first");
    assert_eq!(runner.run(&cmd).unwrap().stdout, "second");
    let err = runner.run(&cmd).unwrap_err().to_string();
    assert!(err.contains("xcrun simctl boot A"), "{err}");
    assert!(err.contains("2 time"), "{err}");
}

#[test]
fn replay_rejects_an_unrecorded_command_and_names_it() {
    let runner = ReplayRunner::new(vec![recording("xcode-select", &["-p"], 0, "")]);

    let err = runner
        .run(&Command::new("sysctl").args(["-n", "hw.ncpu"]))
        .unwrap_err()
        .to_string();

    assert!(err.contains("sysctl -n hw.ncpu"), "{err}");
    assert!(err.contains("no recording"), "{err}");
}

#[test]
fn replay_loads_recordings_from_fixture_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("p.json");
    std::fs::write(
        &path,
        serde_json::to_string(&recording("xcode-select", &["-p"], 0, "/X\n")).unwrap(),
    )
    .unwrap();

    let runner = ReplayRunner::from_files([&path]).unwrap();

    assert_eq!(
        runner
            .run(&Command::new("xcode-select").arg("-p"))
            .unwrap()
            .stdout,
        "/X\n"
    );
}

#[test]
fn real_runner_captures_stdout_and_exit_status() {
    let out = RealRunner
        .run(&Command::new("sh").args(["-c", "echo hi; exit 3"]))
        .unwrap();

    assert_eq!(out.stdout, "hi\n");
    assert_eq!(out.status, Some(3));
    assert!(!out.success());
}

#[test]
fn sanitizer_removes_home_user_and_secrets_from_a_recording() {
    let sanitizer = Sanitizer::new("/Users/alice", "alice").with_secrets(["s3cr3t-token"]);
    let rec = recording(
        "sh",
        &["-c", "cat /Users/alice/a"],
        0,
        "owner alice at /Users/alice/Library token=s3cr3t-token",
    );

    let clean = sanitizer.sanitize(rec);

    assert_eq!(clean.args, vec!["-c", "cat ~/a"]);
    assert_eq!(clean.stdout, "owner <user> at ~/Library token=<redacted>");
}
