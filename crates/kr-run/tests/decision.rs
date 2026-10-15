//! The rules of the first decision point (ADR 0011, ADR 0015): which failures kr asks
//! about, what it suggests, and how a later outcome reconciles the agent's choice.

use kr_run::decision::{FailureKind, Seen, suggest, unseen};
use kr_run::{Case, Category, Final};

fn case(id: &str, outcome: Final, signature: Option<&str>) -> Case {
    Case {
        id: id.to_owned(),
        outcome,
        duration_s: 1.0,
        signature: signature.map(str::to_owned),
        category: signature.map(|_| Category::Assertion),
    }
}

#[test]
fn only_unseen_failures_that_alone_fail_the_item_need_a_decision() {
    let cases = [
        case("T/a()", Final::Passed, None),
        case("T/b()", Final::Failed, Some("failed - asked to fail")),
    ];
    let mut seen = Seen::default();
    assert_eq!(unseen(&cases, false, &seen).len(), 1);
    // Something else fails the item (a timeout, a missing test): nothing to decide.
    assert!(unseen(&cases, true, &seen).is_empty());
    // The same failure of the same test was seen before: kr's rules say FAIL.
    seen.learn(&[case("T/b()", Final::Failed, Some("failed - asked to fail"))]);
    assert!(unseen(&cases, false, &seen).is_empty());
}

#[test]
fn a_seen_signature_of_another_test_or_another_signature_is_still_unseen() {
    let cases = [case("T/b()", Final::Failed, Some("failed - asked to fail"))];
    let mut seen = Seen::default();
    seen.learn(&[
        case("T/other()", Final::Failed, Some("failed - asked to fail")),
        case("T/b()", Final::Flaky, Some("XCTAssertTrue failed")),
        // Passed tests teach nothing.
        case("T/b()", Final::Passed, None),
    ]);
    assert_eq!(unseen(&cases, false, &seen).len(), 1);
}

#[test]
fn one_seen_failure_fails_the_item_so_its_other_failures_are_not_asked_about() {
    let cases = [
        case("T/a()", Final::Failed, Some("new")),
        case("T/b()", Final::Failed, Some("known")),
    ];
    let mut seen = Seen::default();
    seen.learn(&[case("T/b()", Final::Failed, Some("known"))]);
    assert!(unseen(&cases, false, &seen).is_empty());
}

#[test]
fn kr_suggests_environment_only_when_the_message_names_it() {
    assert_eq!(
        suggest(Some(Category::Infrastructure)).option,
        "environment"
    );
    assert_eq!(suggest(Some(Category::Launch)).option, "environment");
    for c in [
        Some(Category::Assertion),
        Some(Category::Crash),
        Some(Category::Timeout),
        Some(Category::Unknown),
        None,
    ] {
        assert_eq!(suggest(c).option, "product", "{c:?}");
    }
}

#[test]
fn a_later_outcome_on_the_same_inputs_supports_or_refutes_the_choice() {
    use FailureKind::*;
    assert_eq!(Product.supported_by(Final::Failed), Some(true));
    assert_eq!(Product.supported_by(Final::Passed), Some(false));
    assert_eq!(Flaky.supported_by(Final::Passed), Some(true));
    assert_eq!(Flaky.supported_by(Final::Flaky), Some(true));
    assert_eq!(Flaky.supported_by(Final::Failed), Some(false));
    assert_eq!(Environment.supported_by(Final::Failed), Some(false));
    assert_eq!(Environment.supported_by(Final::Skipped), None);
}

#[test]
fn options_are_named_as_kr_decide_takes_them() {
    let names: Vec<String> = FailureKind::options().into_iter().map(|o| o.name).collect();
    assert_eq!(names, ["product", "flaky", "environment"]);
    assert_eq!(FailureKind::parse("flaky"), Some(FailureKind::Flaky));
    assert_eq!(FailureKind::parse("Flaky"), None);
}
