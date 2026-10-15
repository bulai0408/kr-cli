//! `kr run --remote` / `kr gate --remote` on Krill's configuration, replaying real
//! recordings (tests/fixtures/remote): git on a clean, pushed clone; ssh to localhost
//! (a Mac), to `vps` and `fnos` (Linux) and to a name that does not resolve.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use kr_exec::Recording;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> Vec<Recording> {
    let path = root().join("tests/fixtures/remote").join(name);
    let files: Vec<PathBuf> = if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        files
    } else {
        vec![path.with_extension("json")]
    };
    files
        .iter()
        .map(|f| {
            let mut r = kr_exec::read_recording(f).unwrap();
            // Recorded with kr 0.1.0 on both sides; keep "the same version" true.
            r.stdout = r.stdout.replace(
                "kr=kr 0.1.0",
                &format!("kr=kr {}", env!("CARGO_PKG_VERSION")),
            );
            r
        })
        .collect()
}

/// The recordings of a clean, pushed checkout, with `change` applied to the git call
/// whose arguments start with `args`.
fn git_with(args: &[&str], change: impl Fn(&mut Recording)) -> Vec<Recording> {
    let mut recs = fixture("git-pushed");
    let r = recs
        .iter_mut()
        .find(|r| {
            r.args
                .iter()
                .map(String::as_str)
                .take(args.len())
                .eq(args.iter().copied())
        })
        .expect("a recorded git call");
    change(r);
    recs
}

const CONFIG: &str = r#"
[remotes.localhost]
ssh = "localhost"
dir = "/tmp/kr-remote"
kr = "/tmp/kr-remote/bin/kr"

[remotes.vps]
ssh = "vps"

[remotes.fnos]
ssh = "fnos"

[remotes.nowhere]
ssh = "nowhere.invalid"
"#;

struct Repo {
    home: tempfile::TempDir,
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Repo {
    fn new() -> Self {
        Self::with_project(&std::fs::read_to_string(root().join("examples/krill.kr.toml")).unwrap())
    }

    fn with_project(kr_toml: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".kr.toml"), kr_toml).unwrap();
        let config = home.path().join(".config/kr/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(config, CONFIG).unwrap();
        Self { home }
    }

    fn replay_files(&self, recordings: &[Recording]) -> String {
        let dir = self.home.path().join(format!("replay-{}", nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        recordings
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let p = dir.join(format!("{i:03}.json"));
                std::fs::write(&p, serde_json::to_string(r).unwrap()).unwrap();
                p.display().to_string()
            })
            .collect::<Vec<_>>()
            .join(":")
    }

    fn command(&self, recordings: &[Recording], args: &[&str]) -> std::process::Command {
        let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("kr"));
        cmd.env_clear()
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .env("KR_REPLAY", self.replay_files(recordings))
            .env("KR_APPLICATIONS", root().join("tests/fixtures/xcode/apps"))
            .current_dir(self.home.path().join("repo"))
            .args(args);
        cmd
    }

    fn kr(&self, recordings: &[Recording], args: &[&str]) -> Out {
        self.kr_env(recordings, args, &[])
    }

    fn kr_env(&self, recordings: &[Recording], args: &[&str], env: &[(&str, &str)]) -> Out {
        let mut cmd = Command::from_std(self.command(recordings, args));
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        Out {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn events(&self) -> Vec<serde_json::Value> {
        let dir = self.home.path().join(".local/state/kr/events");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flat_map(|e| {
                std::fs::read_to_string(e.unwrap().path())
                    .unwrap()
                    .lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect::<Vec<serde_json::Value>>()
            })
            .collect()
    }

    fn events_of(&self, kind: &str) -> Vec<serde_json::Value> {
        self.events()
            .into_iter()
            .filter(|e| e["type"] == kind)
            .collect()
    }
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

/// A recorded host run, as asked for with other kr arguments (only those differ).
fn asked_as(mut run: Recording, kr_args: &str) -> Recording {
    let script = run.args.last_mut().unwrap();
    *script = script.replace(
        "--json run source-checks kit-build",
        &format!("--json {kr_args}"),
    );
    run
}

fn with(mut recs: Vec<Recording>, more: &[&str]) -> Vec<Recording> {
    for name in more {
        recs.extend(fixture(name));
    }
    recs
}

fn json(out: &Out) -> serde_json::Value {
    serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", out.stdout))
}

// --- The result comes back ---

#[test]
fn a_remote_run_reports_the_hosts_result_as_its_own() {
    let r = Repo::new();
    let out = r.kr(
        &with(fixture("git-pushed"), &["probe-localhost", "run-localhost"]),
        &["run", "--remote", "localhost", "source-checks", "kit-build"],
    );
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert!(
        lines[0].starts_with("PASS: source-checks ("),
        "{}",
        out.stdout
    );
    assert!(lines[1].starts_with("PASS: kit-build ("), "{}", out.stdout);
    assert!(
        lines[2].starts_with("run on localhost: 2/2 PASS · "),
        "{}",
        out.stdout
    );
    assert!(
        out.stderr
            .contains("run on localhost: v0/integration at 4ea68bc on localhost; log: "),
        "{}",
        out.stderr
    );
}

#[test]
fn the_json_result_is_the_hosts_with_where_it_ran() {
    let r = Repo::new();
    let out = r.kr(
        &with(fixture("git-pushed"), &["probe-localhost", "run-localhost"]),
        &[
            "--json",
            "run",
            "--remote",
            "localhost",
            "source-checks",
            "kit-build",
        ],
    );
    assert_eq!(out.code, Some(0), "{}", out.stdout);
    let v = json(&out);
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["ok"], true);
    assert_eq!(v["label"], "run on localhost");
    assert_eq!(v["host"], "localhost");
    assert_eq!(v["remote"]["branch"], "v0/integration");
    assert_eq!(
        v["remote"]["commit"],
        "4ea68bc7212949937b6cb0714fdfd8a59cc6855b"
    );
    assert_eq!(
        v["remote"]["workdir"],
        "/tmp/kr-remote/Krill@v0-integration"
    );
    assert_eq!(v["remote"]["exit_code"], 0);
    let names: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["source-checks", "kit-build"]);
}

#[test]
fn events_of_a_remote_run_are_written_here_too() {
    let r = Repo::new();
    r.kr(
        &with(fixture("git-pushed"), &["probe-localhost", "run-localhost"]),
        &["run", "--remote", "localhost", "source-checks", "kit-build"],
    );
    let started = r.events_of("item_start");
    assert_eq!(started.len(), 2);
    let ended = r.events_of("item_end");
    assert_eq!(ended.len(), 2);
    assert!(ended.iter().all(|e| e["fields"]["host"] == "localhost"
        && e["fields"]["passed"] == true
        && e["fields"]["duration_ms"].is_u64()));
    let end = &r.events_of("run_end")[0];
    assert_eq!(end["fields"]["ok"], true);
    assert_eq!(end["fields"]["host"], "localhost");
    assert_eq!(end["fields"]["branch"], "v0/integration");
    assert_eq!(end["fields"]["remote_exit"], 0);
    assert_eq!(end["project"], "Krill");
    assert_eq!(end["run_id"], ended[0]["run_id"]);
}

#[test]
fn a_gate_runs_remotely_by_name() {
    let r = Repo::new();
    let mut recs = with(fixture("git-pushed"), &["probe-localhost"]);
    // The same host output, asked for as `gate smoke`.
    recs.push(asked_as(fixture("run-localhost").remove(0), "gate smoke"));
    let out = r.kr(&recs, &["gate", "smoke", "--remote", "localhost"]);
    assert_eq!(out.code, Some(0), "{}{}", out.stdout, out.stderr);
    assert!(
        out.stdout.contains("gate smoke on localhost: 2/2 PASS"),
        "{}",
        out.stdout
    );
}

// --- Refused before the run ---

#[test]
fn a_linux_host_is_refused_as_not_macos() {
    for host in ["vps", "fnos"] {
        let r = Repo::new();
        let out = r.kr(
            &with(fixture("git-pushed"), &[&format!("probe-{host}")]),
            &["run", "--remote", host, "source-checks"],
        );
        assert_eq!(out.code, Some(1), "{}", out.stderr);
        insta::assert_snapshot!(format!("linux-{host}"), out.stderr);
        assert!(r.events_of("item_start").is_empty());
    }
}

#[test]
fn a_refusal_in_json_lists_each_problem_with_its_next_step() {
    let r = Repo::new();
    let out = r.kr(
        &with(fixture("git-pushed"), &["probe-vps"]),
        &["--json", "run", "--remote", "vps", "source-checks"],
    );
    assert_eq!(out.code, Some(1));
    let v = json(&out);
    assert_eq!(v["error"]["code"], "kr::remote::preflight");
    assert_eq!(v["error"]["problems"][0]["code"], "not_macos");
    assert!(
        v["error"]["problems"][0]["next"]
            .as_str()
            .unwrap()
            .contains("[remotes]")
    );
    let end = &r.events_of("run_end")[0];
    assert_eq!(end["fields"]["ok"], false);
    assert_eq!(end["fields"]["error"], "refused");
    assert_eq!(end["fields"]["problems"][0]["code"], "not_macos");
}

#[test]
fn an_unreachable_host_is_refused_with_how_to_reach_it() {
    let r = Repo::new();
    let out = r.kr(
        &with(fixture("git-pushed"), &["probe-nowhere"]),
        &["--json", "run", "--remote", "nowhere", "source-checks"],
    );
    assert_eq!(out.code, Some(1));
    let p = &json(&out)["error"]["problems"][0];
    assert_eq!(p["code"], "unreachable");
    assert!(
        p["message"].as_str().unwrap().contains("Connection closed"),
        "{p}"
    );
    assert!(
        p["next"]
            .as_str()
            .unwrap()
            .contains("ssh nowhere.invalid true"),
        "{p}"
    );
}

/// The localhost probe with its output changed.
fn mac_probe(stdout: impl Fn(&str) -> String) -> Recording {
    let mut probe = fixture("probe-localhost").remove(0);
    probe.stdout = stdout(&probe.stdout);
    probe
}

fn problem_codes(out: &Out) -> Vec<String> {
    json(out)["error"]["problems"]
        .as_array()
        .unwrap_or_else(|| panic!("{}", out.stdout))
        .iter()
        .map(|p| p["code"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_mac_without_xcode_or_with_another_kr_is_refused() {
    let cases: [(&str, Recording); 3] = [
        (
            "no_xcode",
            mac_probe(|s| {
                s.lines()
                    .map(|l| if l.starts_with("xcode=") { "xcode=" } else { l })
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        ),
        (
            "kr_missing",
            mac_probe(|s| {
                s.lines()
                    .map(|l| if l.starts_with("kr=") { "kr=" } else { l })
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        ),
        (
            "kr_version",
            mac_probe(|s| {
                s.lines()
                    .map(|l| {
                        if l.starts_with("kr=") {
                            "kr=kr 0.0.1"
                        } else {
                            l
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        ),
    ];
    for (code, probe) in cases {
        let r = Repo::new();
        let mut recs = fixture("git-pushed");
        recs.push(probe);
        let out = r.kr(
            &recs,
            &["--json", "run", "--remote", "localhost", "source-checks"],
        );
        assert_eq!(out.code, Some(1), "{code}: {}", out.stdout);
        assert_eq!(problem_codes(&out), [code]);
    }
}

#[test]
fn a_branch_the_host_cannot_fetch_is_refused() {
    let cases: Vec<(&str, Vec<Recording>)> = vec![
        (
            "not_pushed",
            git_with(&["ls-remote"], |r| r.stdout = String::new()),
        ),
        (
            "not_pushed",
            git_with(&["ls-remote"], |r| {
                r.stdout = r.stdout.replace("4ea68bc7", "0000000a");
            }),
        ),
        (
            "uncommitted",
            git_with(&["status"], |r| {
                r.stdout = " M Krill/App.swift\n".to_owned()
            }),
        ),
        (
            "detached_head",
            git_with(&["rev-parse", "--abbrev-ref"], |r| {
                r.stdout = "HEAD\n".to_owned()
            }),
        ),
        (
            "no_origin",
            git_with(&["remote"], |r| {
                r.status = Some(2);
                r.stdout = String::new();
                r.stderr = "error: No such remote 'origin'\n".to_owned();
            }),
        ),
    ];
    for (code, git) in cases {
        let r = Repo::new();
        let out = r.kr(
            &with(git, &["probe-localhost"]),
            &["--json", "run", "--remote", "localhost", "source-checks"],
        );
        assert_eq!(out.code, Some(1), "{code}: {}", out.stdout);
        assert_eq!(problem_codes(&out), [code], "{}", out.stdout);
    }
}

#[test]
fn an_unpushed_branch_says_how_to_push_it() {
    let r = Repo::new();
    let out = r.kr(
        &with(
            git_with(&["ls-remote"], |r| r.stdout = String::new()),
            &["probe-localhost"],
        ),
        &["run", "--remote", "localhost", "source-checks"],
    );
    insta::assert_snapshot!(out.stderr);
}

#[test]
fn every_problem_is_reported_at_once() {
    let r = Repo::new();
    let mut git = git_with(&["ls-remote"], |r| r.stdout = String::new());
    git.retain(|r| r.args[0] != "status");
    git.extend(
        git_with(&["status"], |r| r.stdout = " M a.swift\n".to_owned())
            .into_iter()
            .filter(|r| r.args[0] == "status"),
    );
    let out = r.kr(
        &with(git, &["probe-vps"]),
        &["--json", "run", "--remote", "vps", "source-checks"],
    );
    assert_eq!(
        problem_codes(&out),
        ["uncommitted", "not_pushed", "not_macos"]
    );
}

#[test]
fn an_undeclared_remote_names_the_declared_ones_and_the_file() {
    let r = Repo::new();
    let out = r.kr(&[], &["run", "--remote", "studio", "source-checks"]);
    assert_eq!(out.code, Some(1));
    assert!(
        out.stderr.contains(
            "no remote machine `studio` is declared; declared: fnos, localhost, nowhere, vps"
        ),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("[remotes.studio]"), "{}", out.stderr);
}

// --- Signing over SSH ---

/// Krill plus a macOS test suite at the repo root: Xcode signs that build with a
/// developer certificate.
fn with_mac_suite() -> Repo {
    let mut toml = std::fs::read_to_string(root().join("examples/krill.kr.toml")).unwrap();
    toml += "\n[suites.mac-tests]\ntype = \"test\"\nscheme = \"Krill\"\ndestination = \"macos\"\n";
    Repo::with_project(&toml)
}

#[test]
fn a_remote_run_refuses_work_that_needs_a_developer_certificate() {
    let r = with_mac_suite();
    let out = r.kr(
        &with(fixture("git-pushed"), &["probe-localhost"]),
        &[
            "--json",
            "run",
            "--remote",
            "localhost",
            "mac-tests",
            "kit-build",
        ],
    );
    assert_eq!(out.code, Some(1));
    let v = json(&out);
    let problems = v["error"]["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{v}");
    assert_eq!(problems[0]["code"], "needs_signing");
    assert!(
        problems[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("`mac-tests` builds the app for generic/platform=macOS"),
        "{v}"
    );
}

#[test]
fn in_an_ssh_session_work_that_needs_a_certificate_is_refused_at_once() {
    let r = with_mac_suite();
    // No recordings: refused before any command runs.
    let out = r.kr_env(
        &[],
        &["run", "mac-tests"],
        &[("SSH_CONNECTION", "192.0.2.1 50000 192.0.2.2 22")],
    );
    assert_eq!(out.code, Some(1));
    insta::assert_snapshot!(out.stderr);
}

#[test]
fn in_an_ssh_session_simulator_and_package_work_is_not_refused() {
    let r = with_mac_suite();
    // Gets past the signing rule and fails later, on the missing recording.
    let out = r.kr_env(
        &[],
        &["run", "kit-build", "smoke"],
        &[("SSH_CONNECTION", "192.0.2.1 50000 192.0.2.2 22")],
    );
    assert!(
        !out.stderr.contains("developer certificate"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("no recording for"), "{}", out.stderr);
}

// --- Host lost during the run ---

/// The host run, lost after `duration_ms`, as asked for with `kr_args`.
fn lost_run(duration_ms: u64, kr_args: &str) -> Recording {
    let mut run = asked_as(fixture("run-localhost-lost").remove(0), kr_args);
    run.duration_ms = Some(duration_ms);
    run
}

#[test]
fn a_host_lost_during_the_run_leaves_every_item_unverified() {
    let r = Repo::new();
    let mut recs = with(fixture("git-pushed"), &["probe-localhost"]);
    recs.push(lost_run(200, "run source-checks kit-build"));
    let out = r.kr(
        &recs,
        &["run", "--remote", "localhost", "source-checks", "kit-build"],
    );
    assert_eq!(out.code, Some(1), "{}{}", out.stdout, out.stderr);
    insta::assert_snapshot!(out.stdout.lines().take(2).collect::<Vec<_>>().join("\n"));
    let ended = r.events_of("item_end");
    assert_eq!(ended.len(), 2);
    for e in &ended {
        assert_eq!(e["fields"]["passed"], false);
        assert_eq!(e["fields"]["tests"]["unverified"], 1);
        assert!(
            e["fields"]["unverified_reason"]
                .as_str()
                .unwrap()
                .starts_with("lost the connection to localhost during the run: "),
            "{e}"
        );
    }
    let end = &r.events_of("run_end")[0];
    assert_eq!(end["fields"]["ok"], false);
    assert!(end["fields"]["host_lost"].is_string());
}

#[test]
fn deliver_lists_items_of_a_lost_host_as_unverified() {
    let r = Repo::new();
    let mut recs = with(fixture("git-pushed"), &["probe-localhost"]);
    recs.push(lost_run(50, "run source-checks"));
    r.kr(&recs, &["run", "--remote", "localhost", "source-checks"]);
    let out = r.kr(&[], &["--json", "deliver"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let v = json(&out);
    assert_eq!(v["machine"]["items"][0]["result"], "unverified");
    assert!(
        v["unverified"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["kind"] == "unverified" && t["item"] == "source-checks"),
        "{v}"
    );
}

#[test]
fn a_host_lost_in_json_says_why_and_what_next() {
    let r = Repo::new();
    let mut recs = with(fixture("git-pushed"), &["probe-localhost"]);
    recs.push(lost_run(50, "run source-checks"));
    let out = r.kr(
        &recs,
        &["--json", "run", "--remote", "localhost", "source-checks"],
    );
    let v = json(&out);
    assert_eq!(v["ok"], false, "{v}\n{}", out.stderr);
    assert_eq!(v["items"][0]["tests"]["unverified"], 1);
    assert!(
        v["host_lost"]["next"]
            .as_str()
            .unwrap()
            .contains("ssh localhost true")
    );
}

#[test]
fn a_stop_signal_during_a_remote_run_aborts_it() {
    let r = Repo::new();
    let mut recs = with(fixture("git-pushed"), &["probe-localhost"]);
    let mut run = asked_as(fixture("run-localhost").remove(0), "run source-checks");
    run.duration_ms = Some(10_000);
    recs.push(run);
    let child = r
        .command(&recs, &["run", "--remote", "localhost", "source-checks"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while r.events_of("item_start").is_empty() {
        assert!(started.elapsed().as_secs() < 10, "the run did not start");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    // SAFETY: plain kill(2) to the kr child.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let out = child.wait_with_output().unwrap();
    assert!(started.elapsed().as_secs() < 8);
    assert_eq!(out.status.code(), Some(143));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ABORTED: run on localhost (SIGTERM) · interrupted: source-checks"),
        "{stdout}"
    );
    let aborted = &r.events_of("run_aborted")[0];
    assert_eq!(aborted["fields"]["host"], "localhost");
    assert!(r.events_of("item_end").is_empty());
}

// --- Real hosts (opt-in) ---

/// `KR_INTEGRATION=1` runs these against real hosts: `ssh localhost` (this Mac, with kr
/// and Xcode) runs a whole remote run of `KR_IT_REMOTE_REPO` (a clean checkout of a
/// pushed branch with a .kr.toml) and `KR_IT_REMOTE_TARGETS` (space-separated);
/// `KR_IT_LINUX_HOSTS` (e.g. "vps fnos") must be refused as not macOS. The host's kr
/// queues for the machine itself, so heavy work waits for the machine lock there.
fn real_hosts() -> bool {
    std::env::var("KR_INTEGRATION").is_ok_and(|v| v == "1")
}

fn real_kr(repo: &Path, config: &Path, args: &[&str]) -> Out {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("kr"));
    // kr reads every other KR_* variable as configuration, the test's own ones too.
    for (key, _) in std::env::vars().filter(|(k, _)| k.starts_with("KR_")) {
        cmd.env_remove(key);
    }
    let out = cmd
        .env("KR_CONFIG", config)
        .current_dir(repo)
        .args(args)
        .output()
        .unwrap();
    Out {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn real_config(dir: &Path) -> PathBuf {
    let mut text = String::new();
    if let Ok(machine) = std::fs::read_to_string(
        Path::new(&std::env::var("HOME").unwrap()).join(".config/kr/config.toml"),
    ) {
        text += &machine;
    }
    text += &format!(
        "\n[remotes.it-localhost]\nssh = \"localhost\"\ndir = \"/tmp/kr-remote\"\nkr = \"{}\"\n",
        assert_cmd::cargo::cargo_bin("kr").display()
    );
    for host in std::env::var("KR_IT_LINUX_HOSTS")
        .unwrap_or_default()
        .split_whitespace()
    {
        text += &format!("\n[remotes.it-{host}]\nssh = \"{host}\"\n");
    }
    let path = dir.join("config.toml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn real_localhost_runs_the_whole_flow() {
    if !real_hosts() {
        return;
    }
    let repo = PathBuf::from(std::env::var("KR_IT_REMOTE_REPO").expect("KR_IT_REMOTE_REPO"));
    let targets = std::env::var("KR_IT_REMOTE_TARGETS").expect("KR_IT_REMOTE_TARGETS");
    let dir = tempfile::tempdir().unwrap();
    let config = real_config(dir.path());
    let mut args = vec!["--json", "run", "--remote", "it-localhost"];
    args.extend(targets.split_whitespace());
    let out = real_kr(&repo, &config, &args);
    let v: serde_json::Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}\n{}\n{}", out.stdout, out.stderr));
    assert_eq!(v["host"], "it-localhost", "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(out.code, Some(0));
}

#[test]
fn real_linux_hosts_are_refused_as_not_macos() {
    if !real_hosts() {
        return;
    }
    let repo = PathBuf::from(std::env::var("KR_IT_REMOTE_REPO").expect("KR_IT_REMOTE_REPO"));
    let targets = std::env::var("KR_IT_REMOTE_TARGETS").expect("KR_IT_REMOTE_TARGETS");
    let dir = tempfile::tempdir().unwrap();
    let config = real_config(dir.path());
    for host in std::env::var("KR_IT_LINUX_HOSTS")
        .unwrap_or_default()
        .split_whitespace()
    {
        let remote = format!("it-{host}");
        let mut args = vec!["--json", "run", "--remote", remote.as_str()];
        args.extend(targets.split_whitespace());
        let out = real_kr(&repo, &config, &args);
        assert_eq!(out.code, Some(1), "{host}: {}", out.stdout);
        let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
        let codes: Vec<&str> = v["error"]["problems"]
            .as_array()
            .unwrap_or_else(|| panic!("{host}: {v}"))
            .iter()
            .map(|p| p["code"].as_str().unwrap())
            .collect();
        assert!(codes.contains(&"not_macos"), "{host}: {v}");
    }
}

#[test]
fn a_build_that_fails_on_the_host_fails_the_run_and_says_why() {
    // Real output of the first run on the LAN Mac `work`: xcodegen was not on the PATH
    // of the SSH command, so `generate` failed and nothing ran.
    let r = Repo::new();
    let out = r.kr(
        &with(
            fixture("git-pushed"),
            &["probe-localhost", "run-work-build-failed"],
        ),
        &["run", "--remote", "localhost", "source-checks", "kit-build"],
    );
    assert_eq!(out.code, Some(1), "{}{}", out.stdout, out.stderr);
    insta::assert_snapshot!(out.stdout);
    let end = &r.events_of("run_end")[0];
    assert_eq!(end["fields"]["ok"], false);
    assert_eq!(end["fields"]["build_failed"], true);
}
