//! The verdict for one scenario. Covers the four false passes seen in Krill
//! (pitfalls T5, R2): zero tests, a swallowed exit code, half a run, an ignored filter.

use kr_run::{Attempt, Final, judge};
use kr_xcode::{Enumeration, TestCase, TestOutcome};

fn enumeration(ids: &[&str]) -> Enumeration {
    Enumeration {
        enabled: ids.iter().map(|s| s.to_string()).collect(),
        ..Enumeration::default()
    }
}

fn case(id: &str, outcome: TestOutcome) -> TestCase {
    TestCase {
        id: id.into(),
        outcome,
        duration_s: 1.0,
        failure: (outcome == TestOutcome::Failed).then(|| format!("{id} broke")),
        filter: id.trim_end_matches("()").into(),
    }
}

const A: &str = "AppUITests/LoginUITests/testA()";
const B: &str = "AppUITests/LoginUITests/testB()";
const C: &str = "AppUITests/LoginUITests/testC()";

fn attempt(exit: i32, cases: Vec<TestCase>) -> Attempt {
    Attempt {
        exit: Some(exit),
        timed_out: false,
        results: Some(cases),
        bundle: None,
    }
}

#[test]
fn all_enumerated_tests_passing_is_a_pass() {
    let v = judge(
        &enumeration(&[A, B]),
        &attempt(
            0,
            vec![case(A, TestOutcome::Passed), case(B, TestOutcome::Passed)],
        ),
        &[],
    );
    assert!(v.passed(), "{v:?}");
    assert_eq!(v.count(Final::Passed), 2);
}

#[test]
fn zero_selected_tests_is_a_failure_even_with_exit_zero() {
    let v = judge(&enumeration(&[]), &attempt(0, vec![]), &[]);
    assert!(!v.passed());
    assert_eq!(v.reasons, ["the filters select 0 tests"]);
}

#[test]
fn a_swallowed_exit_code_does_not_hide_failed_tests() {
    // xcodebuild reported success, the result bundle has a failure.
    let v = judge(
        &enumeration(&[A, B]),
        &attempt(
            0,
            vec![case(A, TestOutcome::Passed), case(B, TestOutcome::Failed)],
        ),
        &[None],
    );
    assert!(!v.passed());
    assert_eq!(v.count(Final::Failed), 1);
}

#[test]
fn a_failing_exit_code_without_failed_tests_is_a_failure() {
    let v = judge(
        &enumeration(&[A]),
        &attempt(65, vec![case(A, TestOutcome::Passed)]),
        &[],
    );
    assert!(!v.passed());
    assert_eq!(v.reasons, ["xcodebuild exited 65 although no test failed"]);
}

#[test]
fn running_only_part_of_the_selection_is_a_failure() {
    let v = judge(
        &enumeration(&[A, B, C]),
        &attempt(0, vec![case(A, TestOutcome::Passed)]),
        &[],
    );
    assert!(!v.passed());
    assert_eq!(
        v.reasons,
        [
            "ran 1 of 3 selected tests; not run: AppUITests/LoginUITests/testB(), AppUITests/LoginUITests/testC()"
        ]
    );
}

#[test]
fn running_tests_outside_the_filter_is_a_failure() {
    // A single-test filter was ignored and the whole class ran.
    let v = judge(
        &enumeration(&[A]),
        &attempt(
            0,
            vec![case(A, TestOutcome::Passed), case(B, TestOutcome::Passed)],
        ),
        &[],
    );
    assert!(!v.passed());
    assert_eq!(
        v.reasons,
        ["ran 1 test(s) the filters did not select: AppUITests/LoginUITests/testB()"]
    );
}

#[test]
fn a_failure_that_passes_on_its_rerun_is_flaky_and_the_scenario_passes() {
    let first = attempt(
        65,
        vec![case(A, TestOutcome::Passed), case(B, TestOutcome::Failed)],
    );
    let rerun = attempt(0, vec![case(B, TestOutcome::Passed)]);
    let v = judge(&enumeration(&[A, B]), &first, &[Some(rerun)]);
    assert!(v.passed(), "{v:?}");
    assert_eq!(v.count(Final::Flaky), 1);
    assert_eq!(
        v.tests.iter().find(|t| t.id == B).unwrap().outcome,
        Final::Flaky
    );
}

#[test]
fn a_failure_that_fails_again_is_a_failure_with_its_message() {
    let first = attempt(65, vec![case(A, TestOutcome::Failed)]);
    let rerun = attempt(65, vec![case(A, TestOutcome::Failed)]);
    let v = judge(&enumeration(&[A]), &first, &[Some(rerun)]);
    assert!(!v.passed());
    assert_eq!(v.summary(), [format!("{A}: {A} broke")]);
}

#[test]
fn skipped_tests_are_reported_but_do_not_fail() {
    let v = judge(
        &enumeration(&[A, B]),
        &attempt(
            0,
            vec![case(A, TestOutcome::Passed), case(B, TestOutcome::Skipped)],
        ),
        &[],
    );
    assert!(v.passed());
    assert_eq!(v.count(Final::Skipped), 1);
}

#[test]
fn a_timeout_is_a_failure() {
    let a = Attempt {
        exit: None,
        timed_out: true,
        results: None,
        bundle: None,
    };
    let v = judge(&enumeration(&[A]), &a, &[]);
    assert!(!v.passed());
    assert_eq!(v.reasons[0], "timed out");
}

#[test]
fn a_missing_result_bundle_is_a_failure() {
    let a = Attempt {
        exit: Some(0),
        timed_out: false,
        results: None,
        bundle: None,
    };
    let v = judge(&enumeration(&[A]), &a, &[]);
    assert_eq!(
        v.reasons,
        ["no test results (the result bundle is missing or unreadable)"]
    );
}

#[test]
fn a_missing_result_bundle_is_named_when_known() {
    let a = Attempt {
        exit: Some(0),
        timed_out: false,
        results: None,
        bundle: Some("/repo/build/kr/results/login/run.xcresult".into()),
    };
    let v = judge(&enumeration(&[A]), &a, &[]);
    assert_eq!(
        v.reasons,
        ["no test results: /repo/build/kr/results/login/run.xcresult is missing or unreadable"]
    );
}
