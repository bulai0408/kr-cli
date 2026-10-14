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
