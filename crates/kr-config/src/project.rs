//! The project file `.kr.toml` (spec §4).

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Everything kr needs to know about one Apple client repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub project: ProjectInfo,
    #[serde(default)]
    pub simulators: Simulators,
    #[serde(default)]
    pub suites: BTreeMap<String, Suite>,
    #[serde(default)]
    pub scenarios: BTreeMap<String, Scenario>,
    #[serde(default)]
    pub gates: BTreeMap<String, Gate>,
    #[serde(default)]
    pub isolation: Isolation,
    #[serde(default)]
    pub signing: Signing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectInfo {
    /// Names the simulator pool: `<name>-Verify-<N>`.
    pub name: String,
    pub xcodeproj: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    /// Command that generates the Xcode project, run before every build.
    pub generate: Option<String>,
    /// Scheme of the app and its UI tests, used by scenarios.
    pub scheme: Option<String>,
    pub configuration: Option<String>,
    /// Extra `KEY=VALUE` build settings for every xcodebuild call.
    #[serde(default)]
    pub build_settings: BTreeMap<String, String>,
    /// Required Xcode version, matched as a prefix of the installed version ("27" or "26.6").
    pub xcode: Option<String>,
    #[serde(default)]
    pub platforms: Vec<Platform>,
    /// Builds share Xcode's compilation cache with the other worktrees of this project
    /// (ADR 0013). Off by default.
    #[serde(default)]
    pub compilation_cache: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Platform {
    #[serde(rename = "iOS")]
    Ios,
    #[serde(rename = "tvOS")]
    Tvos,
    #[serde(rename = "macOS")]
    Macos,
    #[serde(rename = "watchOS")]
    Watchos,
    #[serde(rename = "visionOS")]
    Visionos,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Ios => "iOS",
            Platform::Tvos => "tvOS",
            Platform::Macos => "macOS",
            Platform::Watchos => "watchOS",
            Platform::Visionos => "visionOS",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The simulator pool of this project.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Simulators {
    /// Runtime for every device kind, e.g. "iOS 18.6".
    pub runtime: Option<String>,
    /// Device kinds. Kind `default` is named `<project>-Verify-<N>`; any other kind
    /// `<project>-Verify-<kind>-<N>`.
    pub devices: BTreeMap<String, DeviceKind>,
}

/// The kind used when a scenario or suite names none.
pub const DEFAULT_DEVICE: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceKind {
    /// Device type name as `xcrun simctl list devicetypes` shows it, e.g. "iPhone 16".
    pub device_type: String,
    #[serde(default = "one")]
    pub count: u32,
    /// Overrides `simulators.runtime` for this kind.
    pub runtime: Option<String>,
}

fn one() -> u32 {
    1
}

/// A verification step of a gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Suite {
    /// Compile a scheme for some platforms (`generic/platform=…`).
    Build {
        /// Working directory, relative to the repo root. When set, xcodebuild runs
        /// there on its own (e.g. a Swift package) instead of the project's
        /// `xcodeproj` / `workspace`.
        dir: Option<PathBuf>,
        scheme: String,
        /// Default: the project platforms.
        #[serde(default)]
        platforms: Vec<Platform>,
        timeout: Option<String>,
    },
    /// Run a test scheme, optionally filtered.
    Test {
        /// Same as for `build`.
        dir: Option<PathBuf>,
        scheme: String,
        /// `-only-testing` identifiers. Empty: every test of the scheme.
        #[serde(default)]
        only: Vec<String>,
        /// `-skip-testing` identifiers.
        #[serde(default)]
        skip: Vec<String>,
        #[serde(default)]
        destination: Destination,
        /// Device kind when `destination = "simulator"`. Default: `default`.
        device: Option<String>,
        timeout: Option<String>,
    },
    /// Any shell command, e.g. backend tests. Runs with `sh -c`.
    Command {
        run: String,
        /// Working directory, relative to the repo root.
        dir: Option<PathBuf>,
        timeout: Option<String>,
    },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Destination {
    /// A device leased from the simulator pool.
    #[default]
    Simulator,
    /// This Mac.
    Macos,
}

/// A group of UI test classes that represents one verifiable feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// `-only-testing` identifiers, e.g. "KrillUITests/RatingsUITests".
    pub only: Vec<String>,
    #[serde(default)]
    pub skip: Vec<String>,
    /// Device kind. Default: `default`.
    pub device: Option<String>,
    /// Hard timeout, e.g. "15m". Default: 20m.
    pub timeout: Option<String>,
    /// Depends on an outside service: never served from the result cache.
    #[serde(default)]
    pub external: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prepare: Vec<Prepare>,
}

/// A dependency prepared before a scenario and cleaned up afterwards (ADR 0020).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prepare {
    pub run: String,
    pub cleanup: Option<String>,
    pub dir: Option<PathBuf>,
    pub timeout: Option<String>,
    #[serde(default)]
    pub background: bool,
    pub ready: Option<String>,
    pub shared: Option<String>,
}

/// A named set of suites and scenarios that decides whether a batch can merge.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Gate {
    pub suites: Vec<String>,
    /// Scenario names, or "*" for all scenarios.
    pub scenarios: Vec<String>,
}

/// What test builds may touch (data isolation check).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Isolation {
    /// Bundle identifiers allowed for the app under test.
    pub bundle_ids: Vec<String>,
    /// Allowed values per entitlement, e.g.
    /// "com.apple.developer.icloud-container-environment" = ["Development"].
    pub entitlements: BTreeMap<String, Vec<String>>,
}

/// What a build signed for a real device must be signed with (signing readback before
/// it is installed).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Signing {
    /// The Apple Developer Team ID the app must be signed by, e.g. "K4AXH95DCF".
    pub team: Option<String>,
    /// Let xcodebuild register a device that is not yet in the team on the developer
    /// website (`-allowProvisioningDeviceRegistration`). Off: it changes the team's account.
    pub register_devices: bool,
}

/// Default hard timeout of a scenario or suite.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Parses "90s", "15m", "1h", "1h30m".
pub fn parse_timeout(text: &str) -> Option<Duration> {
    let mut total = 0u64;
    let mut number = String::new();
    let mut seen_unit = false;
    for c in text.trim().chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let n: u64 = number.parse().ok()?;
        number.clear();
        let unit = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            _ => return None,
        };
        total = total.checked_add(n.checked_mul(unit)?)?;
        seen_unit = true;
    }
    if !number.is_empty() || !seen_unit || total == 0 {
        return None;
    }
    Some(Duration::from_secs(total))
}

impl ProjectConfig {
    /// Scenario names a gate runs, in order.
    pub fn gate_scenarios(&self, gate: &Gate) -> Vec<String> {
        if gate.scenarios.iter().any(|s| s == "*") {
            self.scenarios.keys().cloned().collect()
        } else {
            gate.scenarios.clone()
        }
    }

    /// Pool device names of a kind: `Krill-Verify-1`, `Krill-Verify-iPad-1`, …
    pub fn device_names(&self, kind: &str) -> Vec<String> {
        let count = self.simulators.devices.get(kind).map_or(0, |k| k.count);
        let name = &self.project.name;
        (1..=count)
            .map(|n| {
                if kind == DEFAULT_DEVICE {
                    format!("{name}-Verify-{n}")
                } else {
                    format!("{name}-Verify-{kind}-{n}")
                }
            })
            .collect()
    }

    /// Checks references between sections. Each problem has the key path it is about.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        let info = &self.project;
        if info.xcodeproj.is_some() && info.workspace.is_some() {
            out.push(Problem::new(
                &["project", "workspace"],
                "set either `xcodeproj` or `workspace`, not both",
                "keep the one xcodebuild should open",
            ));
        }
        if let Some(xcode) = &info.xcode
            && !is_version(xcode)
        {
            out.push(Problem::new(
                &["project", "xcode"],
                format!("`{xcode}` is not a version"),
                "write the Xcode version, for example \"27.1\" or \"26\"",
            ));
        }
        let kinds = &self.simulators.devices;
        let check_device = |path: &[&str], device: &Option<String>, out: &mut Vec<Problem>| {
            let kind = device.as_deref().unwrap_or(DEFAULT_DEVICE);
            if !kinds.contains_key(kind) {
                let mut full: Vec<&str> = path.to_vec();
                if device.is_some() {
                    full.push("device");
                }
                out.push(Problem::new(
                    &full,
                    format!("no simulator kind `{kind}`"),
                    format!(
                        "add [simulators.devices.{kind}] with a device_type, or use one of: {}",
                        list(kinds.keys())
                    ),
                ));
            }
        };
        for (name, scenario) in &self.scenarios {
            let path = ["scenarios", name.as_str()];
            if scenario.only.is_empty() {
                out.push(Problem::new(
                    &path,
                    "a scenario needs at least one test in `only`",
                    "add the UI test classes, e.g. only = [\"AppUITests/LoginUITests\"]",
                ));
            }
            check_device(&path, &scenario.device, &mut out);
            check_timeout(&path, &scenario.timeout, &mut out);
            for (index, step) in scenario.prepare.iter().enumerate() {
                let key = index.to_string();
                let base = ["scenarios", name.as_str(), "prepare", key.as_str()];
                let mut problem = |field: &str, message: &str, help: &str| {
                    let mut path = base.to_vec();
                    path.push(field);
                    out.push(Problem::new(
                        &path,
                        format!("{}: {message}", path.join(".")),
                        help,
                    ));
                };
                for (field, value) in [
                    ("run", Some(&step.run)),
                    ("ready", step.ready.as_ref()),
                    ("cleanup", step.cleanup.as_ref()),
                    ("shared", step.shared.as_ref()),
                ] {
                    if value.is_some_and(|s| s.trim().is_empty()) {
                        problem(
                            field,
                            "must not be empty",
                            "write a non-empty value or remove the optional field",
                        );
                    }
                }
                if step.background && step.ready.is_none() {
                    problem(
                        "ready",
                        "a background step needs a readiness check",
                        "add a readiness command, for example ready = \"curl --fail http://127.0.0.1:8080/health\"",
                    );
                }
                if !step.background && step.ready.is_some() {
                    problem(
                        "ready",
                        "ready requires background = true",
                        "set background = true or remove ready",
                    );
                }
                if step.timeout.as_ref().is_some_and(|t| {
                    parse_timeout(t)
                        .and_then(|d| std::time::Instant::now().checked_add(d))
                        .is_none()
                }) {
                    problem(
                        "timeout",
                        "invalid duration",
                        "write a positive duration such as \"30s\"",
                    );
                }
                if step.dir.as_ref().is_some_and(|p| {
                    p.is_absolute() || p.components().any(|c| c == std::path::Component::ParentDir)
                }) {
                    problem(
                        "dir",
                        "must be relative to the project root",
                        "use a project subdirectory without ..",
                    );
                }
            }
        }
        let mut shared: BTreeMap<&str, (&str, usize, &Prepare)> = BTreeMap::new();
        for (name, scenario) in &self.scenarios {
            for (index, step) in scenario.prepare.iter().enumerate() {
                if let Some(key) = step.shared.as_deref() {
                    if let Some((previous, previous_index, definition)) = shared.get(key) {
                        if *definition != step {
                            let index = index.to_string();
                            out.push(Problem::new(
                                &["scenarios", name, "prepare", &index],
                                format!("scenarios.{name}.prepare.{index}: shared `{key}` differs from scenarios.{previous}.prepare.{previous_index}"),
                                "use identical run/dir/ready/cleanup/background/timeout for the same shared name",
                            ));
                        }
                    } else {
                        shared.insert(key, (name, index, step));
                    }
                }
            }
        }
        for (name, suite) in &self.suites {
            let path = ["suites", name.as_str()];
            match suite {
                Suite::Test {
                    destination: Destination::Simulator,
                    device,
                    timeout,
                    ..
                } => {
                    check_device(&path, device, &mut out);
                    check_timeout(&path, timeout, &mut out);
                }
                Suite::Build {
                    platforms, timeout, ..
                } => {
                    if platforms.is_empty() && info.platforms.is_empty() {
                        out.push(Problem::new(
                            &path,
                            "no platforms to build for",
                            "set `platforms` here or in [project]",
                        ));
                    }
                    check_timeout(&path, timeout, &mut out);
                }
                Suite::Test { timeout, .. } | Suite::Command { timeout, .. } => {
                    check_timeout(&path, timeout, &mut out)
                }
            }
        }
        for (name, kind) in kinds {
            if kind.count == 0 {
                out.push(Problem::new(
                    &["simulators", "devices", name, "count"],
                    "count must be at least 1",
                    "remove the kind or set count = 1",
                ));
            }
            if kind.runtime.is_none() && self.simulators.runtime.is_none() {
                out.push(Problem::new(
                    &["simulators", "devices", name],
                    "no runtime for this kind",
                    "set `runtime` here or in [simulators], e.g. runtime = \"iOS 18.6\"",
                ));
            }
        }
        for (name, gate) in &self.gates {
            for (i, suite) in gate.suites.iter().enumerate() {
                if !self.suites.contains_key(suite) {
                    out.push(Problem::at(
                        &["gates", name, "suites"],
                        i,
                        format!("no suite `{suite}`"),
                        format!("known suites: {}", list(self.suites.keys())),
                    ));
                }
            }
            for (i, scenario) in gate.scenarios.iter().enumerate() {
                if scenario != "*" && !self.scenarios.contains_key(scenario) {
                    out.push(Problem::at(
                        &["gates", name, "scenarios"],
                        i,
                        format!("no scenario `{scenario}`"),
                        format!(
                            "known scenarios: {} (or \"*\" for all)",
                            list(self.scenarios.keys())
                        ),
                    ));
                }
            }
        }
        out
    }
}

fn check_timeout(path: &[&str], timeout: &Option<String>, out: &mut Vec<Problem>) {
    if let Some(t) = timeout
        && parse_timeout(t).is_none()
    {
        let mut full = path.to_vec();
        full.push("timeout");
        out.push(Problem::new(
            &full,
            format!("`{t}` is not a duration"),
            "write it like \"90s\", \"15m\" or \"1h30m\"",
        ));
    }
}

fn is_version(v: &str) -> bool {
    !v.is_empty()
        && v.split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

fn list<'a>(names: impl Iterator<Item = &'a String>) -> String {
    let names: Vec<&str> = names.map(String::as_str).collect();
    if names.is_empty() {
        "(none)".to_owned()
    } else {
        names.join(", ")
    }
}

/// A semantic problem in the project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Key path, e.g. ["gates", "smoke", "scenarios"].
    pub path: Vec<String>,
    /// Index into the array at `path`, if the problem is one element.
    pub index: Option<usize>,
    pub message: String,
    pub help: String,
}

impl Problem {
    fn new(path: &[&str], message: impl Into<String>, help: impl Into<String>) -> Self {
        Self {
            path: path.iter().map(|s| s.to_string()).collect(),
            index: None,
            message: message.into(),
            help: help.into(),
        }
    }

    fn at(
        path: &[&str],
        index: usize,
        message: impl Into<String>,
        help: impl Into<String>,
    ) -> Self {
        Self {
            index: Some(index),
            ..Self::new(path, message, help)
        }
    }
}
