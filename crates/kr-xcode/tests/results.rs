use std::path::{Path, PathBuf};

use kr_xcode::{Enumeration, TestOutcome, only_testing_id, parse_test_results};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(rel)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(fixture(rel)).unwrap()
}

#[test]
fn enumeration_lists_enabled_tests_in_full_form() {
    let e = Enumeration::parse(&read("xcresult/sample-enumeration.json")).unwrap();
    assert_eq!(
        e.enabled,
        [
            "KrSampleTests/GreetingTests/greetsByName()",
            "KrSampleTests/GreetingTests/greetsEmptyName()",
            "KrSampleUITests/LaunchUITests/testFlakyOnce()",
            "KrSampleUITests/LaunchUITests/testShowsGreeting()",
        ]
    );
    assert!(e.errors.is_empty());
}

#[test]
fn enumeration_tells_which_filters_hit_nothing() {
    let e = Enumeration::parse(&read("xcresult/sample-enumeration.json")).unwrap();
    assert!(e.hits("KrSampleUITests/LaunchUITests"));
    assert!(e.hits("KrSampleTests"));
    assert!(e.hits("KrSampleUITests/LaunchUITests/testShowsGreeting"));
    assert!(e.hits("KrSampleTests/GreetingTests/greetsByName()"));
    assert!(
        !e.hits("KrSampleUITests/SettingsUITests"),
        "disabled by the filters, not enabled"
    );
    assert!(
        !e.hits("KrSampleUITests/Launch"),
        "no prefix matches inside a name"
    );
}

#[test]
fn test_results_give_every_case_with_target_and_outcome() {
    let results = parse_test_results(&read("xcresult/sample-r1-tests.json")).unwrap();
    let got: Vec<(&str, TestOutcome)> =
        results.iter().map(|t| (t.id.as_str(), t.outcome)).collect();
    assert_eq!(
        got,
        [
            (
                "KrSampleTests/GreetingTests/greetsByName()",
                TestOutcome::Passed
            ),
            (
                "KrSampleTests/GreetingTests/greetsEmptyName()",
                TestOutcome::Passed
            ),
            (
                "KrSampleUITests/LaunchUITests/testFlakyOnce()",
                TestOutcome::Failed
            ),
            (
                "KrSampleUITests/LaunchUITests/testShowsGreeting()",
                TestOutcome::Passed
            ),
        ]
    );
    let flaky = &results[2];
    assert_eq!(
        flaky.failure.as_deref(),
        Some("failed - first run fails on purpose")
    );
    assert!(flaky.duration_s > 1.0);
}

#[test]
fn a_rerun_result_has_just_the_rerun_test() {
    let results = parse_test_results(&read("xcresult/sample-r2-tests.json")).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, TestOutcome::Passed);
}

#[test]
fn only_testing_ids_drop_the_parentheses_of_xctest_methods() {
    assert_eq!(
        only_testing_id("KrSampleUITests/LaunchUITests/testFlakyOnce()"),
        "KrSampleUITests/LaunchUITests/testFlakyOnce"
    );
    assert_eq!(
        only_testing_id("KrSampleTests/GreetingTests/greetsByName()"),
        "KrSampleTests/GreetingTests/greetsByName()"
    );
}
