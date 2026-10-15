//! Judging saved results again (ADR 0010, layer 3): the same rules change nothing; a
//! changed rule shows exactly the verdicts it changes.

use std::path::PathBuf;

use kr_run::rejudge::{Evidence, SCHEMA_VERSION, Saved, TestChange, parse_enabled_tests};
use kr_run::{Attempt, Final, judge};
use kr_xcode::{Enumeration, TestCase, TestOutcome};

const FLAKY: &str = "AppUITests/LoginUITests/testFlaky()";
const STEADY: &str = "AppUITests/LoginUITests/testSteady()";

fn case(id: &str, outcome: TestOutcome) -> TestCase {
    TestCase {
        id: id.into(),
        outcome,
        duration_s: 1.0,
        failure: (outcome == TestOutcome::Failed).then(|| "XCTAssertTrue failed".to_owned()),
        filter: id.trim_end_matches("()").into(),
    }
}

fn attempt(exit: i32, cases: Vec<TestCase>) -> Attempt {
    Attempt {
        exit: Some(exit),
        timed_out: false,
        results: Some(cases),
        bundle: None,
    }
}

/// A pass where FLAKY failed, then passed when run again on its own.
fn saved_with_a_flaky_test() -> Saved {
    let enumeration = Enumeration {
        enabled: vec![FLAKY.into(), STEADY.into()],
        ..Enumeration::default()
    };
    let first = attempt(
        65,
        vec![
            case(FLAKY, TestOutcome::Failed),
            case(STEADY, TestOutcome::Passed),
        ],
    );
    let reruns = vec![Some(attempt(0, vec![case(FLAKY, TestOutcome::Passed)]))];
    Saved {
        schema_version: SCHEMA_VERSION,
        item: "login".into(),
        verdict: judge(&enumeration, &first, &reruns),
        enumeration,
        first,
        reruns,
    }
}

#[test]
fn the_rules_a_run_was_judged_with_change_nothing() {
    let saved = saved_with_a_flaky_test();
    assert!(saved.verdict.passed());
    assert_eq!(saved.rejudge(&judge), None);
}

#[test]
fn a_rule_that_ignores_reruns_turns_the_flaky_test_into_a_failure() {
    let saved = saved_with_a_flaky_test();
    let change = saved
        .rejudge(&|e, first, _| judge(e, first, &[]))
        .expect("a change");
    assert!(change.before.passed);
    assert!(!change.after.passed);
    assert_eq!(
        change.tests,
        [TestChange {
            id: FLAKY.into(),
            before: Some(Final::Flaky),
            after: Some(Final::Failed),
        }]
    );
}

#[test]
fn a_saved_verdict_survives_a_round_trip_through_json() {
    let saved = saved_with_a_flaky_test();
    let back: Saved = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
    assert_eq!(back.rejudge(&judge), None);
    assert_eq!(back.first.results.unwrap()[0].outcome, TestOutcome::Failed);
}

#[test]
fn enabled_tests_of_the_old_e2e_script_are_its_enumeration() {
    let text = "Plan KrillPlayerControls\n\tKrillTests/A/one()\n\tKrillTests/A/two(position:)\n";
    assert_eq!(
        parse_enabled_tests(text).enabled,
        ["KrillTests/A/one()", "KrillTests/A/two(position:)"]
    );
}

fn sample_results() -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/xcresult");
    std::fs::read_to_string(root.join("sample-r1-tests.json")).unwrap()
}

fn evidence(enumeration: Option<Enumeration>, exit: Option<i32>) -> Evidence {
    let mut e = Evidence {
        schema_version: SCHEMA_VERSION,
        bundle: "batch6-gate/e2e/1/results.xcresult".into(),
        exit,
        enumeration,
        results: Some(sample_results()),
        verdict: Default::default(),
    };
    e.verdict = e.judge(&judge);
    e
}

#[test]
fn evidence_without_an_enumeration_counts_the_tests_that_ran_as_selected() {
    let e = evidence(None, None);
    // sample-r1: the flaky UI test failed on its first run; nothing ran it again.
    assert_eq!(e.verdict.count(Final::Failed), 1);
    assert_eq!(e.verdict.count(Final::Passed), 3);
    assert!(e.verdict.reasons.is_empty(), "{:?}", e.verdict.reasons);
    assert_eq!(e.rejudge(&judge), None);
}

#[test]
fn evidence_with_an_enumeration_shows_selected_tests_that_did_not_run() {
    let enabled = Enumeration {
        enabled: vec!["KrSampleTests/GreetingTests/greetsByName()".into()],
        ..Enumeration::default()
    };
    let e = evidence(Some(enabled), Some(65));
    assert!(
        e.verdict.reasons[0].starts_with("ran 3 test(s) the filters did not select"),
        "{:?}",
        e.verdict.reasons
    );
}
