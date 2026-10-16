//! Decisions of `kr run` that do not touch the machine (ADR 0005): whether a scenario
//! passed, what the build inputs hash to, and in which order work goes to simulators.

pub mod decision;
mod failure;
pub mod rejudge;
pub mod schedule;

pub use failure::{Case, Category, cases, failure_category, signature};

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use kr_xcode::{Enumeration, TestCase, TestOutcome};
use serde::{Deserialize, Serialize};

/// One `test-without-building` run and what it left behind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub exit: Option<i32>,
    pub timed_out: bool,
    /// Test cases from the result bundle; `None` if it is missing or unreadable.
    pub results: Option<Vec<TestCase>>,
    /// Where the result bundle should be, to name it when it is missing.
    pub bundle: Option<PathBuf>,
}

/// The four results a test can end with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Final {
    Passed,
    Failed,
    /// Failed, then passed when run again on its own.
    Flaky,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestVerdict {
    pub id: String,
    pub outcome: Final,
    pub failure: Option<String>,
    pub duration_s: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Verdict {
    /// Why the run as a whole cannot be trusted, if it cannot.
    pub reasons: Vec<String>,
    pub tests: Vec<TestVerdict>,
}

impl Verdict {
    pub fn passed(&self) -> bool {
        self.reasons.is_empty() && self.count(Final::Failed) == 0
    }

    pub fn count(&self, outcome: Final) -> usize {
        self.tests.iter().filter(|t| t.outcome == outcome).count()
    }

    /// At most three lines: reasons first, then failed tests with their message.
    pub fn summary(&self) -> Vec<String> {
        self.reasons
            .iter()
            .cloned()
            .chain(
                self.tests
                    .iter()
                    .filter(|t| t.outcome == Final::Failed)
                    .map(|t| match &t.failure {
                        Some(f) => format!("{}: {f}", t.id),
                        None => t.id.clone(),
                    }),
            )
            .take(3)
            .collect()
    }

    /// Failed tests of the first attempt, in order: the ones to run again.
    pub fn to_rerun(first: &Attempt) -> Vec<String> {
        first
            .results
            .iter()
            .flatten()
            .filter(|t| t.outcome == TestOutcome::Failed)
            .map(|t| t.id.clone())
            .collect()
    }
}

/// Judges one scenario. `reruns[i]` is the solo rerun of the i-th failed test of
/// `first` (see [`Verdict::to_rerun`]); `None` if it was not run again.
pub fn judge(enumeration: &Enumeration, first: &Attempt, reruns: &[Option<Attempt>]) -> Verdict {
    let mut v = Verdict::default();
    if first.timed_out {
        v.reasons.push("timed out".to_owned());
    }
    if let Some(error) = enumeration.errors.first() {
        // e.g. the test runner could not be installed: nothing was listed or run.
        v.reasons
            .push(format!("xcodebuild could not list the tests: {error}"));
        return v;
    }
    if enumeration.enabled.is_empty() {
        v.reasons.push("the filters select 0 tests".to_owned());
    }
    let Some(results) = &first.results else {
        v.reasons.push(match &first.bundle {
            Some(bundle) => format!(
                "no test results: {} is missing or unreadable",
                bundle.display()
            ),
            None => "no test results (the result bundle is missing or unreadable)".to_owned(),
        });
        return v;
    };

    let ran: Vec<&str> = results.iter().map(|t| t.id.as_str()).collect();
    let not_run: Vec<&str> = enumeration
        .enabled
        .iter()
        .map(String::as_str)
        .filter(|id| !ran.contains(id))
        .collect();
    if !not_run.is_empty() && !first.timed_out {
        v.reasons.push(format!(
            "ran {} of {} selected tests; not run: {}",
            enumeration.enabled.len() - not_run.len(),
            enumeration.enabled.len(),
            not_run.join(", ")
        ));
    }
    let extra: Vec<&str> = ran
        .iter()
        .copied()
        .filter(|id| !enumeration.enabled.iter().any(|e| e == id))
        .collect();
    if !extra.is_empty() {
        v.reasons.push(format!(
            "ran {} test(s) the filters did not select: {}",
            extra.len(),
            extra.join(", ")
        ));
    }
    let failed_first = results
        .iter()
        .filter(|t| t.outcome == TestOutcome::Failed)
        .count();
    if failed_first == 0 && !first.timed_out && first.exit.is_some_and(|c| c != 0) {
        v.reasons.push(format!(
            "xcodebuild exited {} although no test failed",
            first.exit.unwrap_or_default()
        ));
    }

    let mut rerun_index = 0;
    for t in results {
        let (outcome, failure) = match t.outcome {
            TestOutcome::Passed | TestOutcome::Other => (Final::Passed, None),
            TestOutcome::Skipped => (Final::Skipped, None),
            TestOutcome::Failed => {
                let rerun = reruns.get(rerun_index).and_then(Option::as_ref);
                rerun_index += 1;
                let again = rerun
                    .and_then(|a| a.results.as_ref())
                    .and_then(|r| r.iter().find(|c| c.id == t.id));
                match again {
                    Some(c) if matches!(c.outcome, TestOutcome::Passed | TestOutcome::Other) => {
                        (Final::Flaky, t.failure.clone())
                    }
                    Some(c) => (
                        Final::Failed,
                        c.failure.clone().or_else(|| t.failure.clone()),
                    ),
                    None => (Final::Failed, t.failure.clone()),
                }
            }
        };
        v.tests.push(TestVerdict {
            id: t.id.clone(),
            outcome,
            failure,
            duration_s: t.duration_s,
        });
    }
    v
}

/// The inputs of a build: listed files (by content) and the command line.
pub fn input_hash(root: &Path, files: &[String], command: &[&str]) -> Result<String> {
    let mut sorted: Vec<&String> = files.iter().collect();
    sorted.sort();
    let mut hasher = blake3::Hasher::new();
    for file in sorted {
        hasher.update(file.as_bytes());
        hasher.update(&[0]);
        match std::fs::read(root.join(file)) {
            Ok(bytes) => {
                hasher.update(&(bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
            Err(_) => {
                hasher.update(b"<missing>");
            }
        }
    }
    for part in command {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Longest work first, so the slowest scenarios start early on the simulators;
/// work without history goes first of all.
pub fn longest_first<T>(mut items: Vec<T>, duration: impl Fn(&T) -> Option<Duration>) -> Vec<T> {
    items.sort_by_key(|item| std::cmp::Reverse(duration(item).unwrap_or(Duration::MAX)));
    items
}
