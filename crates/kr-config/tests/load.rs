use std::path::{Path, PathBuf};

use kr_config::{Config, ConfigError, Sources};

fn write(path: &Path, text: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    path.to_owned()
}

fn render(err: &ConfigError) -> String {
    let mut out = String::new();
    miette::GraphicalReportHandler::new_themed(miette::GraphicalTheme::unicode_nocolor())
        .with_width(300)
        .render_report(&mut out, err)
        .unwrap();
    out
}

#[test]
fn later_layers_override_earlier_ones() {
    let dir = tempfile::tempdir().unwrap();
    let machine = write(
        &dir.path().join("config.toml"),
        "[resources]\nbuild = 1\nsim = 2\nlight = 1\n",
    );
    let project = write(
        &dir.path().join("repo/.kr.toml"),
        "[project]\nname = \"krill\"\n\n[resources]\nsim = 3\n",
    );

    let config: Config = Sources {
        machine: Some(machine),
        project: Some(project),
        env: vec![("KR_RESOURCES__LIGHT".into(), "4".into())],
        overrides: vec![("resources.build".into(), "5".into())],
    }
    .load()
    .unwrap();

    assert_eq!(config.project.unwrap().name, "krill");
    assert_eq!(config.resources.build, Some(5));
    assert_eq!(config.resources.sim, Some(3));
    assert_eq!(config.resources.light, Some(4));
}

#[test]
fn missing_files_are_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let config = Sources {
        machine: Some(dir.path().join("nope.toml")),
        ..Sources::default()
    }
    .load()
    .unwrap();
    assert_eq!(config, Config::default());
}

#[test]
fn discovery_finds_the_project_file_up_to_the_repo_root() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let project = write(&repo.join(".kr.toml"), "");
    let nested = repo.join("Sources/App");
    std::fs::create_dir_all(&nested).unwrap();

    assert_eq!(kr_config::find_project_file(&nested), Some(project));
}

#[test]
fn discovery_stops_at_the_repo_root() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join(".kr.toml"), "");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();

    assert_eq!(kr_config::find_project_file(&repo), None);
}

#[test]
fn machine_file_defaults_to_xdg_config_home_then_home() {
    let env = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        kr_config::machine_file(&env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/x")])),
        Some(PathBuf::from("/x/kr/config.toml"))
    );
    assert_eq!(
        kr_config::machine_file(&env(&[("HOME", "/h")])),
        Some(PathBuf::from("/h/.config/kr/config.toml"))
    );
    assert_eq!(
        kr_config::machine_file(&env(&[("HOME", "/h"), ("KR_CONFIG", "/c.toml")])),
        Some(PathBuf::from("/c.toml"))
    );
}

#[test]
fn state_dir_defaults_to_xdg_state_home_then_home() {
    let config = Config::default();
    assert_eq!(
        config.state_dir(&[("HOME".into(), "/h".into())]),
        Some(PathBuf::from("/h/.local/state/kr"))
    );
    assert_eq!(
        config.state_dir(&[
            ("HOME".into(), "/h".into()),
            ("XDG_STATE_HOME".into(), "/s".into())
        ]),
        Some(PathBuf::from("/s/kr"))
    );
}

#[test]
fn a_toml_syntax_error_points_at_the_file_and_line() {
    let dir = tempfile::tempdir().unwrap();
    let project = write(&dir.path().join(".kr.toml"), "[project]\nname = krill\n");

    let err = Sources {
        project: Some(project),
        ..Sources::default()
    }
    .load()
    .unwrap_err();

    insta::assert_snapshot!(render(&err).replace(dir.path().to_str().unwrap(), "<tmp>"));
}

#[test]
fn an_unknown_key_points_at_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let machine = write(
        &dir.path().join("config.toml"),
        "[resources]\nbuild = 1\nsimulators = 2\n",
    );

    let err = Sources {
        machine: Some(machine),
        ..Sources::default()
    }
    .load()
    .unwrap_err();

    insta::assert_snapshot!(render(&err).replace(dir.path().to_str().unwrap(), "<tmp>"));
}

#[test]
fn a_bad_environment_value_names_the_variable() {
    let err = Sources {
        env: vec![("KR_RESOURCES__SIM".into(), "two".into())],
        ..Sources::default()
    }
    .load()
    .unwrap_err();

    insta::assert_snapshot!(render(&err));
}

#[test]
fn non_config_kr_variables_are_ignored() {
    let config = Sources {
        env: vec![
            ("KR_LOG".into(), "debug".into()),
            ("KR_CONFIG".into(), "/c.toml".into()),
        ],
        ..Sources::default()
    }
    .load()
    .unwrap();
    assert_eq!(config, Config::default());
}
