//! Checks of the machine against a project's requirements, each with a fix.

use kr_config::ProjectConfig;
use serde::Serialize;

use crate::{Profile, SimDevices, find_xcode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Work will fail or give wrong results until fixed.
    Error,
    /// Work can go on, but slower or less reliably.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub severity: Severity,
    /// Stable identifier, e.g. "runtime_missing".
    pub code: String,
    pub message: String,
    /// What to do about it.
    pub fix: String,
}

impl Problem {
    fn new(severity: Severity, code: &str, message: String, fix: String) -> Self {
        Self {
            severity,
            code: code.to_owned(),
            message,
            fix,
        }
    }
}

const GB: u64 = 1 << 30;

/// Everything wrong with this machine for `project`. `devices` is the current
/// `simctl list devices` answer; without it the pool is not checked.
pub fn diagnose(
    profile: &Profile,
    project: Option<&ProjectConfig>,
    devices: Option<&SimDevices>,
    load_per_core: f64,
) -> Vec<Problem> {
    let mut out = Vec::new();
    let free_gb = profile.disk_free_bytes / GB;
    if profile.disk_free_bytes < 20 * GB {
        let severity = if profile.disk_free_bytes < 10 * GB {
            Severity::Error
        } else {
            Severity::Warning
        };
        out.push(Problem::new(
            severity,
            "disk_low",
            format!("only {free_gb} GB free on the data volume"),
            "free space: delete old DerivedData (~/Library/Developer/Xcode/DerivedData) and unused simulators (`xcrun simctl delete unavailable`)".to_owned(),
        ));
    }
    if let Some(reason) = profile.hardware.overloaded(load_per_core) {
        out.push(Problem::new(
            Severity::Warning,
            "load_high",
            reason,
            "heavy work will wait for the load to drop; check `top -o cpu` for processes that do not belong to a build".to_owned(),
        ));
    }
    let Some(project) = project else {
        return out;
    };

    if let Some(required) = &project.project.xcode
        && find_xcode(&profile.xcodes, required).is_none()
    {
        let installed: Vec<String> = profile
            .xcodes
            .iter()
            .map(|x| format!("{} ({})", x.version, x.path.display()))
            .collect();
        out.push(Problem::new(
            Severity::Error,
            "xcode_missing",
            format!("Xcode {required} is required by .kr.toml but not installed"),
            format!(
                "install Xcode {required} in /Applications, or change [project].xcode; installed: {}",
                if installed.is_empty() { "none".to_owned() } else { installed.join(", ") }
            ),
        ));
    }

    let mut missing_devices = Vec::new();
    for (kind_name, kind) in &project.simulators.devices {
        let Some(runtime_name) = kind
            .runtime
            .as_ref()
            .or(project.simulators.runtime.as_ref())
        else {
            continue; // rejected when the config was loaded
        };
        let Some(runtime) = profile
            .runtimes
            .iter()
            .find(|r| r.is_available && r.matches(runtime_name))
        else {
            let (platform, version) = runtime_name.split_once(' ').unwrap_or((runtime_name, ""));
            out.push(Problem::new(
                Severity::Error,
                "runtime_missing",
                format!("simulator runtime {runtime_name} (simulators.devices.{kind_name}) is not installed"),
                format!(
                    "install it: `xcodebuild -downloadPlatform {platform} -buildVersion {version}` with an Xcode that supports it, or Xcode > Settings > Components"
                ),
            ));
            continue;
        };
        if !runtime
            .supported_device_types
            .iter()
            .any(|t| t.matches(&kind.device_type))
        {
            let family = kind.device_type.split_whitespace().next().unwrap_or("");
            let similar: Vec<&str> = runtime
                .supported_device_types
                .iter()
                .filter(|t| t.name.starts_with(family))
                .map(|t| t.name.as_str())
                .collect();
            out.push(Problem::new(
                Severity::Error,
                "device_type_unsupported",
                format!(
                    "{runtime_name} has no device type \"{}\" (simulators.devices.{kind_name})",
                    kind.device_type
                ),
                format!("use one of: {}", similar.join(", ")),
            ));
            continue;
        }
        if let Some(devices) = devices {
            for name in project.device_names(kind_name) {
                let present = devices
                    .named(&name)
                    .any(|(rt, d)| rt == runtime.identifier && d.is_available);
                if !present {
                    missing_devices.push(name);
                }
            }
        }
    }
    if !missing_devices.is_empty() {
        out.push(Problem::new(
            Severity::Warning,
            "pool_incomplete",
            format!("simulator pool is missing {}", missing_devices.join(", ")),
            "run `kr pool sync` to create them".to_owned(),
        ));
    }
    out
}
