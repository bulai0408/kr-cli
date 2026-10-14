use std::path::{Path, PathBuf};

use kr_xcode::{TestSymbols, XcTestRun, Xcodebuild, error_summary, parse_entitlements_dump};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(rel)
}

fn recorded_stdout(rel: &str) -> String {
    kr_exec::read_recording(&fixture(rel)).unwrap().stdout
}

#[test]
fn xcodebuild_commands_carry_kr_defaults_in_a_stable_order() {
    let cmd = Xcodebuild::new("build-for-testing")
        .project(Some("Krill.xcodeproj".into()), None)
        .scheme("KrillPlayerControls")
        .configuration(Some("DebugUIAcceptance"))
        .destination("generic/platform=iOS Simulator")
        .derived_data("build/kr/DerivedData")
        .settings([("KRILL_ACCEPTANCE_BUNDLE_ID", "com.kami.krill.e2e148")])
        .command();
    assert_eq!(
        cmd.to_string(),
        "xcodebuild build-for-testing -quiet -skipMacroValidation -project Krill.xcodeproj -scheme KrillPlayerControls -configuration DebugUIAcceptance -destination generic/platform=iOS Simulator -derivedDataPath build/kr/DerivedData KRILL_ACCEPTANCE_BUNDLE_ID=com.kami.krill.e2e148"
    );
}

#[test]
fn test_actions_never_collect_diagnostics_or_clone_simulators() {
    let cmd = Xcodebuild::new("test-without-building")
        .scheme("S")
        .destination("id=X")
        .command();
    assert!(
        cmd.args
            .windows(2)
            .any(|w| w == ["-collect-test-diagnostics", "never"])
    );
    assert!(
        cmd.args
            .windows(2)
            .any(|w| w == ["-parallel-testing-enabled", "NO"])
    );
}

#[test]
fn error_summary_takes_the_first_three_distinct_compiler_errors() {
    let log = "\
/r/A.swift:3:5: error: cannot find 'Foo' in scope
/r/A.swift:3:5: error: cannot find 'Foo' in scope
note: something
/r/B.swift:9:1: error: missing import of defining module 'Modules'
/r/C.swift:1:1: error: third
/r/D.swift:1:1: error: fourth
** TEST BUILD FAILED **
";
    assert_eq!(
        error_summary(log),
        [
            "/r/A.swift:3:5: error: cannot find 'Foo' in scope",
            "/r/B.swift:9:1: error: missing import of defining module 'Modules'",
            "/r/C.swift:1:1: error: third",
        ]
    );
}

#[test]
fn error_summary_prefers_located_compiler_errors_over_driver_noise() {
    let log = "\
error: the following command failed with exit code 1 but produced no further output
/r/App.swift:14:52: error: cannot find 'missingHelper' in scope
   |                                                    `- error: cannot find 'missingHelper' in scope
** TEST BUILD FAILED **
";
    assert_eq!(
        error_summary(log),
        ["/r/App.swift:14:52: error: cannot find 'missingHelper' in scope"]
    );
}

#[test]
fn error_summary_falls_back_to_the_last_lines() {
    assert_eq!(error_summary("a\n\nb\nc\nd\n"), ["b", "c", "d"]);
}

#[test]
fn xctestrun_lists_test_targets_with_resolved_paths() {
    let run = XcTestRun::read(&fixture(
        "xcode/products/KrSample_iphonesimulator27.1-arm64-x86_64.xctestrun",
    ))
    .unwrap();
    let ui = run.target("KrSampleUITests").unwrap();
    assert_eq!(ui.module, "KrSampleUITests");
    assert_eq!(
        ui.bundle,
        PathBuf::from(
            "Debug-iphonesimulator/KrSampleUITests-Runner.app/PlugIns/KrSampleUITests.xctest"
        )
    );
    assert_eq!(
        ui.app.as_deref(),
        Some(Path::new("Debug-iphonesimulator/KrSample.app"))
    );
    let unit = run.target("KrSampleTests").unwrap();
    assert_eq!(
        unit.bundle,
        PathBuf::from("Debug-iphonesimulator/KrSample.app/PlugIns/KrSampleTests.xctest")
    );
    assert_eq!(
        unit.app.as_deref(),
        Some(Path::new("Debug-iphonesimulator/KrSample.app"))
    );
}

#[test]
fn symbols_answer_whether_a_filter_names_real_tests() {
    let mut symbols = TestSymbols::default();
    symbols.add(
        "KrSampleUITests",
        &recorded_stdout("nm/sample-uitests.json"),
    );
    symbols.add(
        "KrSampleTests",
        &recorded_stdout("nm/sample-unittests.json"),
    );

    for ok in [
        "KrSampleUITests",
        "KrSampleUITests/LaunchUITests",
        "KrSampleUITests/LaunchUITests/testShowsGreeting",
        "KrSampleUITests/SettingsUITests/testLaunchesTwice",
        "KrSampleTests/GreetingTests/greetsByName()",
        "KrSampleTests/GreetingTests",
    ] {
        assert!(symbols.matches(ok), "{ok}");
    }
    for bad in [
        "KrSampleUITests/NoSuchTests",
        "KrSampleUITests/LaunchUITests/testMissing",
        "KrSampleTests/GreetingTests/greetsByNickname()",
        "OtherTests",
        "KrSampleUITests/LaunchUITests/init",
    ] {
        assert!(!symbols.matches(bad), "{bad}");
    }
}

#[test]
fn simulator_entitlements_are_read_from_the_otool_dump() {
    let ent = parse_entitlements_dump(&recorded_stdout("otool/sample-entitlements.json")).unwrap();
    assert_eq!(
        ent.get("com.apple.developer.icloud-container-environment")
            .and_then(|v| v.as_string()),
        Some("Development")
    );
}

#[test]
fn bundle_identifier_comes_from_the_built_info_plist() {
    let id = kr_xcode::bundle_identifier(&fixture(
        "xcode/products/Debug-iphonesimulator/KrSample.app",
    ))
    .unwrap();
    assert_eq!(id, "dev.kr.sample.e2e");
}
