use std::path::{Path, PathBuf};

use kr_config::ProjectConfig;
use kr_exec::ReplayRunner;
use kr_machine::{Profile, Severity, SimDevices, detect, diagnose};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(rel)
}

fn profile() -> Profile {
    let rec = |r: &str| kr_exec::read_recording(&fixture(r)).unwrap();
    let runner = ReplayRunner::new(vec![
        rec("sysctl/m3-imac.json"),
        rec("scutil/local-host-name.json"),
        rec("df/root.json"),
        rec("xcode-select/print-path.json"),
        rec("simctl/list-runtimes.json"),
    ]);
    detect(&runner, &fixture("xcode/apps"), None).unwrap()
}

fn devices() -> SimDevices {
    let rec = kr_exec::read_recording(&fixture("simctl/list-devices.json")).unwrap();
    SimDevices::parse(&rec.stdout).unwrap()
}

fn project(text: &str) -> ProjectConfig {
    toml::from_str(text).unwrap()
}

const KRILL: &str = r#"
[project]
name = "Krill"
xcode = "27.1"

[simulators]
runtime = "iOS 18.6"

[simulators.devices.default]
device_type = "iPhone 16 Pro"
count = 2

[simulators.devices.iPad]
device_type = "iPad Pro 13-inch (M4) (16GB)"
"#;

fn codes(problems: &[kr_machine::Problem]) -> Vec<String> {
    problems.iter().map(|p| p.code.clone()).collect()
}

#[test]
fn the_krill_pool_on_this_imac_has_no_problems() {
    let problems = diagnose(&profile(), Some(&project(KRILL)), Some(&devices()));
    assert!(problems.is_empty(), "{problems:#?}");
}

#[test]
fn a_required_xcode_that_is_not_installed_is_an_error_with_the_installed_list() {
    let problems = diagnose(
        &profile(),
        Some(&project(&KRILL.replace("27.1", "28"))),
        Some(&devices()),
    );
    assert_eq!(codes(&problems), ["xcode_missing"]);
    let p = &problems[0];
    assert_eq!(p.severity, Severity::Error);
    assert!(p.message.contains("Xcode 28"), "{}", p.message);
    assert!(
        p.fix.contains("27.1") && p.fix.contains("26.6"),
        "{}",
        p.fix
    );
}

#[test]
fn a_missing_runtime_is_an_error_with_install_instructions() {
    let text = KRILL.replace("runtime = \"iOS 18.6\"", "runtime = \"iOS 17.5\"");
    let problems = diagnose(&profile(), Some(&project(&text)), Some(&devices()));
    assert_eq!(codes(&problems), ["runtime_missing", "runtime_missing"]);
    assert!(
        problems[0]
            .fix
            .contains("xcodebuild -downloadPlatform iOS -buildVersion 17.5"),
        "{}",
        problems[0].fix
    );
}

#[test]
fn a_device_type_the_runtime_does_not_support_is_an_error() {
    let text = KRILL.replace("iPhone 16 Pro", "iPhone 3G");
    let problems = diagnose(&profile(), Some(&project(&text)), Some(&devices()));
    assert_eq!(codes(&problems), ["device_type_unsupported"]);
}

#[test]
fn missing_pool_devices_are_listed() {
    let text = KRILL.replace("count = 2", "count = 4");
    let problems = diagnose(&profile(), Some(&project(&text)), Some(&devices()));
    assert_eq!(codes(&problems), ["pool_incomplete"]);
    assert!(
        problems[0].message.contains("Krill-Verify-3")
            && problems[0].message.contains("Krill-Verify-4")
    );
    assert_eq!(problems[0].severity, Severity::Warning);
}

#[test]
fn low_disk_is_a_warning_then_an_error() {
    let mut p = profile();
    p.disk_free_bytes = 15 << 30;
    assert_eq!(codes(&diagnose(&p, None, None)), ["disk_low"]);
    assert_eq!(diagnose(&p, None, None)[0].severity, Severity::Warning);
    p.disk_free_bytes = 5 << 30;
    assert_eq!(diagnose(&p, None, None)[0].severity, Severity::Error);
}

#[test]
fn high_load_now_is_a_warning() {
    let mut p = profile();
    p.hardware.load.one = 40.0;
    assert_eq!(codes(&diagnose(&p, None, None)), ["load_high"]);
}
