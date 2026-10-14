//! What xcodebuild says about tests: `-enumerate-tests` JSON and
//! `xcresulttool get test-results tests` JSON.

use std::path::Path;

use anyhow::{Context, Result};
use kr_exec::Command;
use serde::{Deserialize, Serialize};

/// The tests a filter selects, from `-test-enumeration-format json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Enumeration {
    /// Full identifiers, e.g. "KrSampleUITests/LaunchUITests/testShowsGreeting()".
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
    pub errors: Vec<String>,
}

impl Enumeration {
    pub fn parse(json: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Id {
            identifier: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Value {
            #[serde(default)]
            enabled_tests: Vec<Id>,
            #[serde(default)]
            disabled_tests: Vec<Id>,
        }
        #[derive(Deserialize)]
        struct Root {
            #[serde(default)]
            errors: Vec<String>,
            #[serde(default)]
            values: Vec<Value>,
        }
        let root: Root = serde_json::from_str(json).context("cannot read the test enumeration")?;
        let mut out = Self {
            errors: root.errors,
            ..Self::default()
        };
        for v in root.values {
            out.enabled
                .extend(v.enabled_tests.into_iter().map(|i| i.identifier));
            out.disabled
                .extend(v.disabled_tests.into_iter().map(|i| i.identifier));
        }
        Ok(out)
    }

    /// True if the `-only-testing` style filter selects at least one enabled test.
    pub fn hits(&self, filter: &str) -> bool {
        let filter = filter.trim_end_matches("()");
        self.enabled.iter().any(|id| {
            let id = id.trim_end_matches("()");
            id == filter || id.starts_with(&format!("{filter}/"))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestOutcome {
    Passed,
    Failed,
    Skipped,
    /// Anything else xcresulttool reports (e.g. "Expected Failure"); counts as passed.
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TestCase {
    /// "Target/Class/method()", the same form as the enumeration.
    pub id: String,
    pub outcome: TestOutcome,
    pub duration_s: f64,
    /// The first failure message, if it failed.
    pub failure: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    #[serde(default)]
    name: String,
    #[serde(default)]
    node_type: String,
    node_identifier: Option<String>,
    result: Option<String>,
    duration_in_seconds: Option<f64>,
    #[serde(default)]
    children: Vec<Node>,
}

/// Every test case in `xcresulttool get test-results tests --compact` output.
pub fn parse_test_results(json: &str) -> Result<Vec<TestCase>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Root {
        test_nodes: Vec<Node>,
    }
    let root: Root = serde_json::from_str(json).context("cannot read the xcresult test results")?;
    let mut out = Vec::new();
    for node in &root.test_nodes {
        walk(node, None, &mut out);
    }
    Ok(out)
}

fn walk(node: &Node, target: Option<&str>, out: &mut Vec<TestCase>) {
    let target = if node.node_type.ends_with("bundle") {
        Some(node.name.as_str())
    } else {
        target
    };
    if node.node_type == "Test Case" {
        let local = node.node_identifier.as_deref().unwrap_or(&node.name);
        let outcome = match node.result.as_deref() {
            Some("Passed") => TestOutcome::Passed,
            Some("Failed") => TestOutcome::Failed,
            Some("Skipped") => TestOutcome::Skipped,
            _ => TestOutcome::Other,
        };
        let failure = node
            .children
            .iter()
            .find(|c| c.node_type == "Failure Message")
            .map(|c| c.name.clone());
        out.push(TestCase {
            id: match target {
                Some(t) => format!("{t}/{local}"),
                None => local.to_owned(),
            },
            outcome,
            duration_s: node.duration_in_seconds.unwrap_or(0.0),
            failure,
        });
        return;
    }
    for child in &node.children {
        walk(child, target, out);
    }
}

/// The identifier to pass to `-only-testing` for one test. XCTest methods (named
/// `test…`) are given without "()"; Swift Testing functions keep it.
pub fn only_testing_id(id: &str) -> String {
    let method = id.rsplit('/').next().unwrap_or(id);
    if method.starts_with("test") {
        id.trim_end_matches("()").to_owned()
    } else {
        id.to_owned()
    }
}

pub fn results_command(xcresult: &Path) -> Command {
    Command::new("xcrun").args([
        "xcresulttool".to_owned(),
        "get".to_owned(),
        "test-results".to_owned(),
        "tests".to_owned(),
        "--path".to_owned(),
        xcresult.display().to_string(),
        "--compact".to_owned(),
    ])
}
