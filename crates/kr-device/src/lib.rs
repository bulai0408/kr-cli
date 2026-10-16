//! Real devices (spec phase 3 §10): find a paired iPhone or iPad with `devicectl`, read
//! its state, and say what to do on it when a prerequisite of a test run is missing.
//!
//! kr reads devicectl's JSON output, which its help calls "versioned and will remain
//! stable across releases" (the table on stdout is not), and the `properties`
//! dictionary of each device: the older `deviceProperties`, `connectionProperties` and
//! `hardwareProperties` are deprecated. kr never erases, restarts or manages a device.

use anyhow::{Context, Result, bail};
use kr_exec::Command;
use serde::Serialize;
use serde_json::Value;

/// `xcrun devicectl list devices` with its JSON on stdout.
pub fn list_command() -> Command {
    Command::new("xcrun").args([
        "devicectl",
        "list",
        "devices",
        "--json-output",
        "-",
        "--quiet",
    ])
}

/// `xcrun devicectl device info lockState` for one device. Unlike `list devices`, it
/// connects to the device, so it also tells whether the device can be reached now.
pub fn lock_state_command(udid: &str) -> Command {
    Command::new("xcrun").args([
        "devicectl",
        "device",
        "info",
        "lockState",
        "--device",
        udid,
        "--json-output",
        "-",
        "--quiet",
    ])
}

/// Whether a device can be reached now, and if so whether it is locked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reach {
    Unlocked,
    /// A passcode is required: UI tests cannot run.
    Locked,
    /// devicectl could not connect to it; the reason it gave.
    Unreachable(String),
}

/// Reads [`lock_state_command`]'s result: its exit status, JSON on stdout and errors.
pub fn parse_lock_state(status_ok: bool, stdout: &str, stderr: &str) -> Reach {
    let doc: Option<Value> = serde_json::from_str(stdout).ok();
    let outcome_ok = doc
        .as_ref()
        .is_some_and(|d| d["info"]["outcome"] == "success");
    match doc.as_ref().map(|d| &d["result"]["passcodeRequired"]) {
        Some(Value::Bool(required)) if status_ok && outcome_ok => {
            if *required {
                Reach::Locked
            } else {
                Reach::Unlocked
            }
        }
        _ => {
            let from_json = doc.as_ref().and_then(|d| {
                d["error"]["userInfo"]["NSLocalizedDescription"]["string"]
                    .as_str()
                    .map(str::to_owned)
            });
            let from_stderr = stderr
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .map(str::to_owned);
            Reach::Unreachable(
                from_json
                    .or(from_stderr)
                    .unwrap_or_else(|| "devicectl gave no reason".to_owned()),
            )
        }
    }
}

/// The version of the SDK the selected Xcode builds `platform` with, e.g. "27.1".
pub fn sdk_version_command(platform: &str) -> Command {
    Command::new("xcrun").args(["--sdk", sdk(platform), "--show-sdk-version"])
}

fn sdk(platform: &str) -> &'static str {
    match platform {
        "tvOS" => "appletvos",
        "watchOS" => "watchos",
        "visionOS" => "xros",
        _ => "iphoneos",
    }
}

/// A physical device devicectl knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    pub name: String,
    /// The CoreDevice identifier.
    pub identifier: String,
    /// What xcodebuild's `-destination id=` takes.
    pub udid: Option<String>,
    /// "iOS" for iPhone and iPad.
    pub platform: String,
    /// "iPhone", "iPad", …
    pub device_type: String,
    /// e.g. "iPad mini (A17 Pro)".
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub paired: bool,
    /// How devicectl last saw it: "wired", "localNetwork", … Its connection state in
    /// `list devices` is not used: it says "disconnected" for a device on a cable until
    /// something connects to it.
    pub transport: Option<String>,
    /// `None` when devicectl does not say (e.g. it never connected).
    pub developer_mode: Option<bool>,
}

impl Device {
    /// Who to address in "On the iPad: …".
    fn on(&self) -> String {
        format!("On the {}", self.device_type)
    }

    /// What keeps a test run on this device from working, given whether it can be
    /// reached (`None`: not asked) and the SDK version of the Xcode kr runs with
    /// (`None` if unknown). Every entry says what to do.
    pub fn missing(
        &self,
        reach: Option<&Reach>,
        xcode: &str,
        sdk_version: Option<&str>,
    ) -> Vec<Missing> {
        let mut out = Vec::new();
        let name = &self.name;
        if !self.paired {
            out.push(Missing::error(
                "device_not_paired",
                format!("{name} is not paired with this Mac"),
                format!(
                    "{}: connect it to this Mac with a cable, unlock it and tap Trust, then run again",
                    self.on()
                ),
            ));
        }
        if self.developer_mode == Some(false) {
            out.push(Missing::error(
                "device_developer_mode_off",
                format!("Developer Mode is off on {name}"),
                format!(
                    "{}: Settings > Privacy & Security > Developer Mode, turn it on and tap Restart; after the restart tap Enable and enter the passcode",
                    self.on()
                ),
            ));
        }
        match reach {
            Some(Reach::Unreachable(why)) => out.push(Missing::error(
                "device_not_connected",
                format!("cannot reach {name}: {why}"),
                format!(
                    "{}: connect it to this Mac with a cable, unlock it and keep the screen on, then run again",
                    self.on()
                ),
            )),
            Some(Reach::Locked) => out.push(Missing::error(
                "device_locked",
                format!("{name} is locked"),
                format!(
                    "{}: unlock it and keep the screen on (Settings > Display & Brightness > Auto-Lock: Never while tests run), then run again",
                    self.on()
                ),
            )),
            _ => {}
        }
        if let (Some(os), Some(sdk)) = (self.os_version.as_deref(), sdk_version) {
            match (major_minor(os), major_minor(sdk)) {
                (Some(device), Some(known)) if device.0 > known.0 => {
                    out.push(Missing::error(
                        "device_os_unsupported",
                        format!(
                            "{name} runs {} {os}; Xcode {xcode} only knows {} {sdk}",
                            self.platform, self.platform
                        ),
                        format!(
                            "install an Xcode with the {} {os} SDK or later and set [project].xcode in .kr.toml to it, or use a device on {} {} or earlier",
                            self.platform, self.platform, known.0
                        ),
                    ));
                }
                (Some(device), Some(known)) if device > known => {
                    out.push(Missing::warning(
                        "device_os_newer",
                        format!(
                            "{name} runs {} {os}, newer than the {} {sdk} SDK of Xcode {xcode}",
                            self.platform, self.platform
                        ),
                        format!(
                            "if installing or running tests fails, install an Xcode with the {} {os} SDK and set [project].xcode in .kr.toml to it",
                            self.platform
                        ),
                    ));
                }
                _ => {}
            }
        }
        out
    }
}

fn major_minor(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((major, minor))
}

/// A missing prerequisite and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Missing {
    pub code: &'static str,
    pub message: String,
    pub next: String,
    /// A warning does not stop a run.
    pub warning: bool,
}

impl Missing {
    fn error(code: &'static str, message: String, next: String) -> Self {
        Self {
            code,
            message,
            next,
            warning: false,
        }
    }

    fn warning(code: &'static str, message: String, next: String) -> Self {
        Self {
            warning: true,
            ..Self::error(code, message, next)
        }
    }
}

/// The physical devices in `devicectl list devices` JSON output (simulators are left out).
pub fn parse_list(json: &str) -> Result<Vec<Device>> {
    let doc: Value = serde_json::from_str(json).context("devicectl did not print JSON")?;
    if doc["info"]["outcome"].as_str() != Some("success") {
        bail!(
            "devicectl could not list devices: {}",
            doc["error"]["userInfo"]["NSLocalizedDescription"]["string"]
                .as_str()
                .unwrap_or("no reason given")
        );
    }
    let devices = doc["result"]["devices"]
        .as_array()
        .context("devicectl's JSON has no result.devices")?;
    Ok(devices
        .iter()
        .filter(|d| d["properties"]["hardware"]["reality"] == "physical")
        .map(|d| {
            let p = &d["properties"];
            let text = |v: &Value| v.as_str().map(str::to_owned);
            Device {
                name: text(&p["state"]["name"]).unwrap_or_default(),
                identifier: text(&d["identifier"]).unwrap_or_default(),
                udid: text(&p["hardware"]["udid"]),
                platform: text(&p["hardware"]["platform"]).unwrap_or_default(),
                device_type: text(&p["hardware"]["deviceType"]).unwrap_or_else(|| "device".into()),
                model: text(&p["hardware"]["marketingName"]),
                os_version: text(&p["software"]["osVersionNumber"]["stringValue"]),
                paired: p["connection"]["pairingState"] == "paired",
                transport: text(&p["connection"]["transportType"]),
                developer_mode: p["state"]["developerModeStatus"]
                    .as_object()
                    .and_then(|m| m.keys().next())
                    .map(|k| k == "enabled"),
            }
        })
        .collect())
}

/// The device named `wanted` (its name, UDID or CoreDevice identifier).
pub fn find<'a>(devices: &'a [Device], wanted: &str) -> Result<&'a Device> {
    let hits: Vec<&Device> = devices
        .iter()
        .filter(|d| d.name == wanted || d.udid.as_deref() == Some(wanted) || d.identifier == wanted)
        .collect();
    match hits.as_slice() {
        [one] => Ok(one),
        [] => {
            let known: Vec<&str> = devices.iter().map(|d| d.name.as_str()).collect();
            bail!(
                "no paired device `{wanted}`; devices this Mac knows: {}. Pair it in Xcode (Window > Devices and Simulators) or use one of these names",
                if known.is_empty() {
                    "none".to_owned()
                } else {
                    known.join(", ")
                }
            )
        }
        many => {
            let ids: Vec<String> = many
                .iter()
                .map(|d| d.udid.clone().unwrap_or_else(|| d.identifier.clone()))
                .collect();
            bail!(
                "{} devices are named `{wanted}`; pass the UDID instead: {}",
                many.len(),
                ids.join(", ")
            )
        }
    }
}
