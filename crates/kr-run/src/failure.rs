//! What the events keep of each test (ADR 0010): its time, and for a failure a
//! signature and a category, so failures can be grouped and counted across runs.

use serde::{Deserialize, Serialize};

use crate::{Final, Verdict};

/// Where a failure comes from, by its message. `Unknown` when no rule matches: kr does
/// not guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Assertion,
    Timeout,
    Crash,
    Launch,
    /// The test runner, the simulator or the connection to them.
    Infrastructure,
    Unknown,
}

/// Rules in order: the first category with a matching phrase wins.
const RULES: &[(Category, &[&str])] = &[
    (
        Category::Timeout,
        &["timed out", "exceeded execution time", "exceeded timeout"],
    ),
    (
        Category::Infrastructure,
        &[
            "lost connection",
            "test runner",
            "coresimulator",
            "failed to install",
            "unable to boot",
            "never finished bootstrapping",
            "failed to establish communication",
        ],
    ),
    (
        Category::Launch,
        &[
            "failed to launch",
            "unable to launch",
            "failed to activate",
            "failed to terminate",
            "is not running",
        ],
    ),
    (
        Category::Crash,
        &[
            "crashed",
            "crash:",
            "unexpected exit",
            "sigabrt",
            "sigsegv",
            "exc_",
        ],
    ),
    (
        Category::Assertion,
        &[
            "xctassert",
            "xctfail",
            "expectation failed",
            "failed - ",
            "caught error",
            "no matches found",
            "failed to tap",
            "failed to synthesize event",
        ],
    ),
];

/// The category of a failure message; `timed_out` when kr's watchdog ended the attempt.
pub fn failure_category(message: &str, timed_out: bool) -> Category {
    if timed_out {
        return Category::Timeout;
    }
    let message = message.to_lowercase();
    RULES
        .iter()
        .find(|(_, phrases)| phrases.iter().any(|p| message.contains(p)))
        .map_or(Category::Unknown, |(category, _)| *category)
}

/// The message without what differs between runs of the same failure: numbers become
/// `N`, `0x…` addresses `<addr>`, and hex identifiers (UUIDs, hashes) `<id>`.
pub fn signature(message: &str) -> String {
    let chars: Vec<char> = message.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let hex_run = |from: usize| {
            chars[from..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit() || **c == '-')
                .count()
        };
        if chars[i] == '0' && chars.get(i + 1) == Some(&'x') && hex_run(i + 2) > 0 {
            out.push_str("<addr>");
            i += 2 + hex_run(i + 2);
            continue;
        }
        let run = hex_run(i);
        if run > 0 {
            let word = &chars[i..i + run];
            if run >= 8 && word.iter().any(char::is_ascii_digit) {
                out.push_str("<id>");
            } else {
                let mut digits = false;
                for c in word {
                    if c.is_ascii_digit() {
                        if !digits {
                            out.push('N');
                        }
                        digits = true;
                    } else {
                        out.push(*c);
                        digits = false;
                    }
                }
            }
            i += run;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One test of a verdict as the events keep it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub outcome: Final,
    pub duration_s: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<Category>,
}

/// Every test of `verdict`; failed and flaky tests with their first failure.
/// `timed_out`: the attempt hit kr's watchdog.
pub fn cases(verdict: &Verdict, timed_out: bool) -> Vec<Case> {
    verdict
        .tests
        .iter()
        .map(|t| {
            let failure = t
                .failure
                .as_deref()
                .filter(|_| matches!(t.outcome, Final::Failed | Final::Flaky));
            Case {
                id: t.id.clone(),
                outcome: t.outcome,
                duration_s: t.duration_s,
                signature: failure.map(signature),
                category: failure.map(|f| failure_category(f, timed_out)),
            }
        })
        .collect()
}
