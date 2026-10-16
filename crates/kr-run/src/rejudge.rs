//! Judging saved results again with the current rules (ADR 0010, layer 3): only the
//! conclusions that change are of interest.

use std::path::PathBuf;

use kr_xcode::{Enumeration, parse_test_results};
use serde::{Deserialize, Serialize};

use crate::{Attempt, Final, Verdict};

/// Version of [`Saved`] and [`Evidence`] files and of `kr rejudge --json`.
pub const SCHEMA_VERSION: u32 = 1;

/// The rules a verdict comes from: [`crate::judge`], or another version of it.
pub type Rules<'a> = &'a dyn Fn(&Enumeration, &Attempt, &[Option<Attempt>]) -> Verdict;

/// What [`crate::judge`] was given for one pass of an item, and what it said. `kr run`
/// saves it as `verdict.json` next to the item's logs, so later rules can judge the run
/// again without its logs or result bundles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    pub schema_version: u32,
    /// The item, with "-again" for a whole-item rerun.
    pub item: String,
    pub enumeration: Enumeration,
    pub first: Attempt,
    pub reruns: Vec<Option<Attempt>>,
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine: Option<QuarantineState>,
}

/// The complete-run guard and gate status when the repository policy was observed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineState {
    pub item: String,
    pub eligible: bool,
    pub blocking: bool,
    pub tests: Vec<String>,
}

impl Saved {
    pub fn rejudge(&self, rules: Rules<'_>) -> Option<Change> {
        compare(
            &self.verdict,
            &rules(&self.enumeration, &self.first, &self.reruns),
        )
    }
}

/// A result bundle from before kr (e.g. Krill's `v0-evidence`), read once with
/// xcresulttool. The raw output is kept, so changes to parsing count too.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub schema_version: u32,
    pub bundle: PathBuf,
    /// From a `status` file next to the bundle.
    pub exit: Option<i32>,
    /// From an `enabled-tests.txt` next to the bundle; without it the tests that ran
    /// count as selected, so "selected but not run" cannot show.
    pub enumeration: Option<Enumeration>,
    /// `xcresulttool get test-results tests --compact` output; `None` if it failed.
    pub results: Option<String>,
    /// The verdict when the bundle was first read.
    pub verdict: Verdict,
}

impl Evidence {
    pub fn judge(&self, rules: Rules<'_>) -> Verdict {
        let results = self
            .results
            .as_deref()
            .and_then(|r| parse_test_results(r).ok());
        let enumeration = self.enumeration.clone().unwrap_or_else(|| Enumeration {
            enabled: results.iter().flatten().map(|t| t.id.clone()).collect(),
            ..Enumeration::default()
        });
        let first = Attempt {
            exit: self.exit,
            timed_out: false,
            results,
            bundle: Some(self.bundle.clone()),
        };
        rules(&enumeration, &first, &[])
    }

    pub fn rejudge(&self, rules: Rules<'_>) -> Option<Change> {
        compare(&self.verdict, &self.judge(rules))
    }
}

/// The tests of an `enabled-tests.txt` written by Krill's `e2e.sh`: a "Plan …" line,
/// then one indented identifier per line.
pub fn parse_enabled_tests(text: &str) -> Enumeration {
    let enabled: Vec<String> = text
        .lines()
        .filter(|l| l.starts_with(['\t', ' ']))
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    Enumeration {
        enabled,
        ..Enumeration::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conclusion {
    pub passed: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TestChange {
    pub id: String,
    /// `None`: the test was not in that verdict.
    pub before: Option<Final>,
    pub after: Option<Final>,
}

/// A verdict that changed: whether the item passed or any test's result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    pub before: Conclusion,
    pub after: Conclusion,
    pub tests: Vec<TestChange>,
}

/// `None` if `after` reaches the same conclusions as `before`; wording of the reasons
/// does not count.
pub fn compare(before: &Verdict, after: &Verdict) -> Option<Change> {
    let outcome = |v: &Verdict, id: &str| v.tests.iter().find(|t| t.id == id).map(|t| t.outcome);
    let mut ids: Vec<&str> = before.tests.iter().map(|t| t.id.as_str()).collect();
    for t in &after.tests {
        if !ids.contains(&t.id.as_str()) {
            ids.push(&t.id);
        }
    }
    let tests: Vec<TestChange> = ids
        .into_iter()
        .filter_map(|id| {
            let (b, a) = (outcome(before, id), outcome(after, id));
            (b != a).then(|| TestChange {
                id: id.to_owned(),
                before: b,
                after: a,
            })
        })
        .collect();
    if tests.is_empty() && before.passed() == after.passed() {
        return None;
    }
    let conclusion = |v: &Verdict| Conclusion {
        passed: v.passed(),
        reasons: v.reasons.clone(),
    };
    Some(Change {
        before: conclusion(before),
        after: conclusion(after),
        tests,
    })
}
