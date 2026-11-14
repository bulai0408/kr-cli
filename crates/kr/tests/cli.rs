use assert_cmd::Command;

/// A temp home with an empty git repo at `<home>/repo`.
struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("repo/.git")).unwrap();
        Self { home }
    }

    fn repo(&self) -> std::path::PathBuf {
        self.home.path().join("repo")
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.home.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn kr(&self) -> Command {
        let mut cmd = Command::cargo_bin("kr").unwrap();
        cmd.env_clear()
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .current_dir(self.repo());
        cmd
    }

    fn clean(&self, text: &[u8]) -> String {
        let home = self.home.path().canonicalize().unwrap();
        String::from_utf8_lossy(text)
            .replace(home.to_str().unwrap(), "<home>")
            .replace(self.home.path().to_str().unwrap(), "<home>")
    }
}

#[test]
fn help_lists_commands_and_global_flags() {
    let out = Sandbox::new().kr().arg("--help").output().unwrap();
    assert!(out.status.success());
    insta::assert_snapshot!(String::from_utf8_lossy(&out.stdout));
}

#[test]
fn version_prints_the_crate_version() {
    let out = Sandbox::new().kr().arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("kr {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn config_json_shows_sources_and_merged_values() {
    let sb = Sandbox::new();
    sb.write(
        ".config/kr/config.toml",
        "[resources]\nbuild = 1\nsim = 2\n",
    );
    sb.write("repo/.kr.toml", "[project]\nname = \"krill\"\n");

    let out = sb
        .kr()
        .args(["config", "--json"])
        .env("KR_RESOURCES__SIM", "3")
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    insta::assert_json_snapshot!(
        serde_json::from_str::<serde_json::Value>(&sb.clean(json.to_string().as_bytes())).unwrap()
    );
}

#[test]
fn config_human_output_starts_with_where_the_config_came_from() {
    let sb = Sandbox::new();
    sb.write("repo/.kr.toml", "[project]\nname = \"krill\"\n");

    let out = sb.kr().arg("config").output().unwrap();

    assert!(out.status.success());
    insta::assert_snapshot!(sb.clean(&out.stdout));
}

#[test]
fn config_error_points_at_the_file_and_line() {
    let sb = Sandbox::new();
    sb.write("repo/.kr.toml", "[project]\nname = krill\n");

    let out = sb.kr().arg("config").output().unwrap();

    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    insta::assert_snapshot!(sb.clean(&out.stderr));
}

#[test]
fn config_error_with_json_is_structured_on_stdout() {
    let sb = Sandbox::new();
    sb.write("repo/.kr.toml", "[project]\nname = krill\n");

    let out = sb.kr().args(["config", "--json"]).output().unwrap();

    assert_eq!(out.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    insta::assert_json_snapshot!(
        serde_json::from_str::<serde_json::Value>(&sb.clean(json.to_string().as_bytes())).unwrap()
    );
}

/// `kr <args>` with fixed capacities, so `kr queue` does not read this Mac's hardware.
fn queue_like(s: &Sandbox, args: &[&str]) -> (String, String) {
    let out = s
        .kr()
        .args(args)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("KR_STATE_DIR", s.home.path().join("state"))
        .env("KR_RESOURCES__BUILD", "1")
        .env("KR_RESOURCES__SIM", "1")
        .env("KR_RESOURCES__LIGHT", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn guessed_status_commands_print_the_queue_and_name_the_real_command() {
    let s = Sandbox::new();
    for json in [false, true] {
        let flag: &[&str] = if json { &["--json"] } else { &[] };
        let (queue, _) = queue_like(&s, &[&["queue"], flag].concat());
        for guess in [&["status"][..], &["lock", "status"]] {
            let (out, err) = queue_like(&s, &[guess, flag].concat());
            assert_eq!(out, queue, "{guess:?} json={json}");
            let hint = format!("`kr {}` is `kr queue`", guess.join(" "));
            assert!(err.contains(&hint), "{guess:?} json={json}: {err}");
            assert_eq!(err.lines().count(), 1, "{err}");
            if json {
                let line: serde_json::Value = serde_json::from_str(err.trim()).unwrap();
                assert!(line["hint"].as_str().unwrap().contains(&hint), "{err}");
            }
        }
    }
    // A global flag before the command still finds the guess.
    let (out, _) = queue_like(&s, &["--json", "lock", "status"]);
    assert_eq!(out, queue_like(&s, &["--json", "queue"]).0);
}

#[test]
fn guessed_names_stay_out_of_help_and_the_skill() {
    let s = Sandbox::new();
    let help = s.kr().arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(
        !help.lines().any(|l| l.trim_start().starts_with("status")),
        "{help}"
    );
    let skill = s.kr().arg("skill").output().unwrap();
    let skill = String::from_utf8_lossy(&skill.stdout);
    for guess in ["kr status", "kr lock status", "kr pool list"] {
        assert!(!skill.contains(guess), "{guess} in kr skill");
    }
}

#[test]
fn a_real_lock_of_a_resource_is_not_taken_for_a_guess() {
    let s = Sandbox::new();
    let out = s.kr().args(["lock", "statu"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
