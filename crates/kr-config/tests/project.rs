use std::path::{Path, PathBuf};
use std::time::Duration;

use kr_config::{ConfigError, ProjectConfig, Sources, Suite, parse_timeout};

const MINIMAL: &str = r#"
[project]
name = "Demo"
scheme = "Demo"

[simulators]
runtime = "iOS 18.6"

[simulators.devices.default]
device_type = "iPhone 16 Pro"
count = 2

[simulators.devices.iPad]
device_type = "iPad Pro 13-inch (M4)"
runtime = "iOS 27.0"

[scenarios.login]
only = ["DemoUITests/LoginUITests"]

[scenarios.layout]
only = ["DemoUITests/LayoutUITests"]
device = "iPad"
timeout = "15m"

[suites.backend]
type = "command"
dir = "backend"
run = "npm test"

[gates.default]
suites = ["backend"]
scenarios = ["*"]

[gates.smoke]
scenarios = ["login"]
"#;

fn load(text: &str) -> Result<ProjectConfig, (ConfigError, PathBuf, tempfile::TempDir)> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".kr.toml");
    std::fs::write(&path, text).unwrap();
    match (Sources {
        project: Some(path.clone()),
        ..Sources::default()
    })
    .load()
    {
        Ok(config) => Ok(config.project.unwrap()),
        Err(e) => Err((e, path, dir)),
    }
}

fn render(err: &ConfigError, dir: &Path) -> String {
    let mut out = String::new();
    miette::GraphicalReportHandler::new_themed(miette::GraphicalTheme::unicode_nocolor())
        .with_width(300)
        .render_report(&mut out, err)
        .unwrap();
    out.replace(dir.to_str().unwrap(), "<tmp>")
}

/// Replaces `from` with `to` in MINIMAL and renders the resulting error.
fn error_for(from: &str, to: &str) -> String {
    assert!(MINIMAL.contains(from), "{from:?} not in MINIMAL");
    let (err, _, dir) = load(&MINIMAL.replacen(from, to, 1)).unwrap_err();
    render(&err, dir.path())
}

#[test]
fn a_valid_project_loads() {
    let config = load(MINIMAL).unwrap();
    assert_eq!(config.project.name, "Demo");
    assert_eq!(config.scenarios.len(), 2);
    assert!(matches!(&config.suites["backend"], Suite::Command { run, .. } if run == "npm test"));
}

#[test]
fn pool_devices_are_named_after_the_project_and_kind() {
    let config = load(MINIMAL).unwrap();
    assert_eq!(
        config.device_names("default"),
        ["Demo-Verify-1", "Demo-Verify-2"]
    );
    assert_eq!(config.device_names("iPad"), ["Demo-Verify-iPad-1"]);
}

#[test]
fn star_in_a_gate_means_every_scenario() {
    let config = load(MINIMAL).unwrap();
    assert_eq!(
        config.gate_scenarios(&config.gates["default"]),
        ["layout", "login"]
    );
    assert_eq!(config.gate_scenarios(&config.gates["smoke"]), ["login"]);
}

#[test]
fn timeouts_parse_like_15m() {
    assert_eq!(parse_timeout("90s"), Some(Duration::from_secs(90)));
    assert_eq!(parse_timeout("15m"), Some(Duration::from_secs(900)));
    assert_eq!(parse_timeout("1h30m"), Some(Duration::from_secs(5400)));
    for bad in ["", "15", "m", "15 minutes", "0m", "-1m"] {
        assert_eq!(parse_timeout(bad), None, "{bad}");
    }
}

#[test]
fn the_krill_example_is_valid() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/krill.kr.toml");
    let config = Sources {
        project: Some(path),
        ..Sources::default()
    }
    .load()
    .unwrap()
    .project
    .unwrap();
    assert_eq!(config.project.name, "Krill");
    assert_eq!(config.scenarios.len(), 33);
    assert_eq!(config.scenarios["smoke"].only.len(), 10);
    assert_eq!(config.gate_scenarios(&config.gates["default"]).len(), 31);
    assert_eq!(config.device_names("iPad"), ["Krill-Verify-iPad-1"]);
}

#[test]
fn a_gate_naming_an_unknown_scenario_points_at_it() {
    insta::assert_snapshot!(error_for(
        r#"scenarios = ["login"]"#,
        r#"scenarios = ["login", "logn"]"#
    ));
}

#[test]
fn a_gate_naming_an_unknown_suite_points_at_it() {
    insta::assert_snapshot!(error_for(
        r#"suites = ["backend"]"#,
        r#"suites = ["backnd"]"#
    ));
}

#[test]
fn a_scenario_on_an_unknown_device_kind_points_at_the_device() {
    insta::assert_snapshot!(error_for(r#"device = "iPad""#, r#"device = "iPhone SE""#));
}

#[test]
fn a_bad_timeout_points_at_the_value() {
    insta::assert_snapshot!(error_for(r#"timeout = "15m""#, r#"timeout = "15 minutes""#));
}

#[test]
fn a_device_kind_without_a_runtime_is_rejected() {
    insta::assert_snapshot!(error_for(
        "[simulators]\nruntime = \"iOS 18.6\"\n",
        "[simulators]\n"
    ));
}

#[test]
fn a_scenario_without_tests_is_rejected() {
    insta::assert_snapshot!(error_for(
        r#"only = ["DemoUITests/LoginUITests"]"#,
        "only = []"
    ));
}

#[test]
fn an_unknown_suite_type_is_rejected() {
    insta::assert_snapshot!(error_for(r#"type = "command""#, r#"type = "script""#));
}

#[test]
fn a_missing_project_section_is_rejected() {
    insta::assert_snapshot!(error_for(
        "[project]\nname = \"Demo\"\nscheme = \"Demo\"\n",
        ""
    ));
}

#[test]
fn several_problems_are_counted() {
    let text = MINIMAL
        .replacen(r#"suites = ["backend"]"#, r#"suites = ["x"]"#, 1)
        .replacen(r#"scenarios = ["login"]"#, r#"scenarios = ["y"]"#, 1);
    let (err, _, _dir) = load(&text).unwrap_err();
    assert!(err.to_string().contains("and 1 more problem"), "{err}");
}
