//! What each test leaves in the events (ADR 0010): its time, and for a failure a
//! signature without numbers and addresses and a category. Messages are from Krill's
//! v0-evidence and kr's own sample runs.

use kr_run::{Category, Final, TestVerdict, Verdict, cases, failure_category, signature};

#[test]
fn messages_fall_into_the_five_categories_and_unknown() {
    let category = |m: &str| failure_category(m, false);
    assert_eq!(category("XCTAssertTrue failed"), Category::Assertion);
    assert_eq!(
        category("Expectation failed: await startupCalls(startup) == 2"),
        Category::Assertion
    );
    assert_eq!(category("failed - asked to fail"), Category::Assertion);
    assert_eq!(
        category("Failed to tap \"Edit\" Button: No matches found for Elements"),
        Category::Assertion
    );
    assert_eq!(
        category("Test exceeded execution time allowance of 10 minutes"),
        Category::Timeout
    );
    assert_eq!(
        category("Failed to launch app with identifier: dev.kr.sample.e2e"),
        Category::Launch
    );
    assert_eq!(
        category("KrSample (1234) crashed in -[Foo bar]"),
        Category::Crash
    );
    assert_eq!(
        category("Lost connection to the test runner"),
        Category::Infrastructure
    );
    assert_eq!(category("something kr has not seen"), Category::Unknown);
    // The attempt hit kr's watchdog: whatever the message says.
    assert_eq!(
        failure_category("XCTAssertTrue failed", true),
        Category::Timeout
    );
}

#[test]
fn signatures_drop_numbers_addresses_and_identifiers() {
    assert_eq!(
        signature(r#"XCTAssertEqual failed: ("Optional("3")") is not equal to ("Optional("4")")"#),
        signature(r#"XCTAssertEqual failed: ("Optional("15")") is not equal to ("Optional("2")")"#),
    );
    assert_eq!(
        signature("object at 0x16b2f3a10 on 6BFDF46B-1778-4C7E-B901-4E825CB78C4D"),
        "object at <addr> on <id>"
    );
    assert_eq!(
        signature("failed - asked to fail"),
        "failed - asked to fail"
    );
}

#[test]
fn each_test_has_its_time_and_failed_or_flaky_ones_their_failure() {
    let verdict = Verdict {
        reasons: Vec::new(),
        tests: vec![
            TestVerdict {
                id: "A/B/testOk()".into(),
                outcome: Final::Passed,
                failure: None,
                duration_s: 2.5,
            },
            TestVerdict {
                id: "A/B/testFlaky()".into(),
                outcome: Final::Flaky,
                failure: Some("XCTAssertEqual failed: (\"1\") is not equal to (\"2\")".into()),
                duration_s: 4.0,
            },
        ],
    };
    let json = serde_json::to_value(cases(&verdict, false)).unwrap();
    assert_eq!(
        json,
        serde_json::json!([
            { "id": "A/B/testOk()", "outcome": "passed", "duration_s": 2.5 },
            {
                "id": "A/B/testFlaky()",
                "outcome": "flaky",
                "duration_s": 4.0,
                "signature": "XCTAssertEqual failed: (\"N\") is not equal to (\"N\")",
                "category": "assertion",
            },
        ])
    );
}
