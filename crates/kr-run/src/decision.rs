//! Decision points (ADR 0011, ADR 0015): where kr's rules end and the calling agent
//! judges. kr marks them with evidence and a default, records what the agent chose and
//! later sets the choice against what happened. The rules here touch nothing outside.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Case, Category, Final};

/// The kind of the first decision point: a failure kr has not seen, of a test that
/// failed again when run on its own.
pub const FAILURE_KIND: &str = "failure_kind";

/// What a decision point changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Affects {
    /// The verdict: the run ends without one (exit 4) until the agent decides.
    Conclusion,
    /// Only how fast the answer comes: kr goes on with its default and records the
    /// decision as pending review.
    Efficiency,
}

/// One option of a decision point and what choosing it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub name: String,
    pub means: String,
}

/// kr's suggestion and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub option: String,
    pub reason: String,
}

/// A decision point as `--json` shows it in `decisions` and the events keep it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// `<run id>-<n>`, the argument of `kr decide`.
    pub id: String,
    pub kind: String,
    pub affects: Affects,
    pub question: String,
    pub item: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<String>,
    pub options: Vec<Choice>,
    /// Paths and statistics the choice can rest on.
    pub evidence: serde_json::Value,
    pub default: Suggestion,
}

impl Decision {
    pub fn option(&self, name: &str) -> Option<&Choice> {
        self.options.iter().find(|o| o.name == name)
    }
}

/// How the agent classified an unseen failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailureKind {
    /// The code or the test is wrong: the test stays failed.
    Product,
    /// It fails now and then: the test counts as flaky, as after a passing rerun.
    Flaky,
    /// The simulator, the machine or a service caused it: the test is unverified.
    Environment,
}

impl FailureKind {
    pub const ALL: [FailureKind; 3] = [Self::Product, Self::Flaky, Self::Environment];

    pub fn name(self) -> &'static str {
        match self {
            Self::Product => "product",
            Self::Flaky => "flaky",
            Self::Environment => "environment",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    fn means(self) -> &'static str {
        match self {
            Self::Product => "a real failure of the code or the test: the test stays failed",
            Self::Flaky => {
                "it fails now and then: the test counts as flaky and does not fail the item"
            }
            Self::Environment => {
                "the simulator, the machine or a service caused it: the test is unverified, which is not a pass"
            }
        }
    }

    pub fn options() -> Vec<Choice> {
        Self::ALL
            .into_iter()
            .map(|k| Choice {
                name: k.name().to_owned(),
                means: k.means().to_owned(),
            })
            .collect()
    }

    /// Whether a later outcome of the test on the same build inputs supports this
    /// classification; `None` if the outcome says nothing (skipped).
    pub fn supported_by(self, later: Final) -> Option<bool> {
        let broken = match later {
            Final::Failed => true,
            Final::Passed | Final::Flaky => false,
            Final::Skipped => return None,
        };
        Some(match self {
            Self::Product => broken,
            Self::Flaky | Self::Environment => !broken,
        })
    }
}

/// kr's default for an unseen failure: environment when the message names the
/// infrastructure or the launch, product otherwise. A failure that repeats on its own
/// rerun is a real failure until shown otherwise.
pub fn suggest(category: Option<Category>) -> Suggestion {
    match category {
        Some(c @ (Category::Infrastructure | Category::Launch)) => Suggestion {
            option: FailureKind::Environment.name().to_owned(),
            reason: format!(
                "the failure message matches kr's {} phrases",
                serde_json::to_value(c)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            ),
        },
        _ => Suggestion {
            option: FailureKind::Product.name().to_owned(),
            reason:
                "it failed again when run on its own, and its message names no environment cause"
                    .to_owned(),
        },
    }
}

/// Failures kr has seen: (test, signature) of failed and flaky tests of earlier runs,
/// the failure signature library of ADR 0011.
#[derive(Debug, Clone, Default)]
pub struct Seen(BTreeSet<(String, String)>);

impl Seen {
    pub fn add(&mut self, test: &str, signature: Option<&str>) {
        self.0
            .insert((test.to_owned(), signature.unwrap_or_default().to_owned()));
    }

    pub fn contains(&self, test: &str, signature: Option<&str>) -> bool {
        self.0
            .contains(&(test.to_owned(), signature.unwrap_or_default().to_owned()))
    }

    /// Learns the failed and flaky tests of `cases`.
    pub fn learn(&mut self, cases: &[Case]) {
        for c in cases
            .iter()
            .filter(|c| matches!(c.outcome, Final::Failed | Final::Flaky))
        {
            self.add(&c.id, c.signature.as_deref());
        }
    }
}

/// The failed tests of an item that need classifying. Only when the classification can
/// change the item's verdict: nothing else fails it (`other_reasons` is false: no
/// timeout, no missing tests, no invalid rerun filter) and none of its failures has
/// been seen. Otherwise the item fails by kr's rules and there is nothing to decide.
pub fn unseen<'a>(cases: &'a [Case], other_reasons: bool, seen: &Seen) -> Vec<&'a Case> {
    let failed: Vec<&Case> = cases
        .iter()
        .filter(|c| c.outcome == Final::Failed)
        .collect();
    if other_reasons
        || failed.is_empty()
        || failed
            .iter()
            .any(|c| seen.contains(&c.id, c.signature.as_deref()))
    {
        return Vec::new();
    }
    failed
}
