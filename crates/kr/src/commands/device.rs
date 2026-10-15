//! A real device for `kr doctor --device`, `kr check --device` and `kr run --device`
//! (spec phase 3 §10): find it with devicectl and check what a test run on it needs.

use anyhow::{Result, bail};
use kr_device::{Device, Missing, Reach};
use kr_exec::Runner;
use serde_json::{Value, json};

use super::check::with_xcode;
use crate::output::Refused;
use crate::{Ctx, Loaded};

/// A device and what is missing for a test run on it.
pub struct Inspected {
    pub device: Device,
    /// `None` when it was not asked (not paired, or Developer Mode off).
    pub reach: Option<Reach>,
    pub missing: Vec<Missing>,
}

impl Inspected {
    /// For people: what kr cannot check on the device before tests run.
    pub const UI_AUTOMATION_NOTE: &str = "devicectl cannot read Settings > Developer > Enable UI Automation; if UI tests fail with kAXError, turn it on on the device";

    pub fn json(&self) -> Value {
        let mut device = json!(self.device);
        device["reach"] = json!(self.reach);
        // devicectl has no way to read Settings > Developer > Enable UI Automation.
        device["ui_automation"] = json!("unknown");
        json!({ "device": device, "missing": self.missing })
    }

    /// One line for people: name · model · OS · pairing · reach · Developer Mode.
    pub fn line(&self) -> String {
        let d = &self.device;
        let mode = match d.developer_mode {
            Some(true) => "Developer Mode on",
            Some(false) => "Developer Mode off",
            None => "Developer Mode unknown",
        };
        let os = d
            .os_version
            .as_deref()
            .map_or_else(|| d.platform.clone(), |v| format!("{} {v}", d.platform));
        let paired = match (d.paired, d.transport.as_deref()) {
            (false, _) => "not paired".to_owned(),
            (true, Some(t)) => format!("paired ({t})"),
            (true, None) => "paired".to_owned(),
        };
        let reach = match &self.reach {
            Some(Reach::Unlocked) => "reachable, unlocked",
            Some(Reach::Locked) => "reachable, locked",
            Some(Reach::Unreachable(_)) => "not reachable",
            None => "not asked",
        };
        let model = d.model.as_deref().unwrap_or(&d.device_type);
        format!("{} · {model} · {os} · {paired} · {reach} · {mode}", d.name)
    }

    /// Fails with every missing prerequisite (warnings do not count).
    pub fn require(&self) -> Result<()> {
        let problems: Vec<kr_remote::Problem> = self
            .missing
            .iter()
            .filter(|m| !m.warning)
            .map(|m| kr_remote::Problem::new(m.code, m.message.clone(), m.next.clone()))
            .collect();
        if problems.is_empty() {
            return Ok(());
        }
        Err(Refused {
            code: "kr::device::prerequisites",
            what: format!("cannot run tests on {}", self.device.name),
            problems,
        }
        .into())
    }
}

/// Finds `wanted` with devicectl and checks it against the Xcode kr runs with.
pub fn inspect(ctx: &Ctx, loaded: &Loaded, runner: &dyn Runner, wanted: &str) -> Result<Inspected> {
    let out = runner.run(&kr_device::list_command())?;
    if !out.success() {
        bail!(
            "`xcrun devicectl list devices` failed: {}; check that Xcode's command line tools work (`xcrun devicectl --version`)",
            out.stderr.trim()
        );
    }
    let devices = kr_device::parse_list(&out.stdout)?;
    let device = kr_device::find(&devices, wanted)?.clone();
    if device.platform != "iOS" {
        bail!(
            "{} is a {} device; kr runs tests on iPhone and iPad only",
            device.name,
            device.platform
        );
    }
    let developer_dir = loaded.developer_dir(ctx)?;
    let sdk = runner
        .run(&with_xcode(
            kr_device::sdk_version_command(&device.platform),
            developer_dir.as_deref(),
        ))
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout.trim().to_owned());
    let xcode = loaded
        .config
        .project
        .as_ref()
        .and_then(|p| p.project.xcode.clone())
        .unwrap_or_else(|| "(xcode-select)".to_owned());
    let mut device = device;
    let reach = (device.paired && device.developer_mode != Some(false))
        .then(|| reach(runner, &device))
        .transpose()?;
    if reach == Some(Reach::Unlocked) {
        // Connected now: how, not how devicectl last saw it.
        device.transport = transport(runner, &device)?.or(device.transport);
    }
    let missing = device.missing(reach.as_ref(), &xcode, sdk.as_deref());
    Ok(Inspected {
        device,
        reach,
        missing,
    })
}

/// Connects to the device to see whether it is there and unlocked.
pub fn reach(runner: &dyn Runner, device: &Device) -> Result<Reach> {
    let udid = device.udid.as_deref().unwrap_or(&device.identifier);
    let out = runner.run(&kr_device::lock_state_command(udid))?;
    Ok(kr_device::parse_lock_state(
        out.success(),
        &out.stdout,
        &out.stderr,
    ))
}

/// How devicectl reaches the device now ("wired", "localNetwork", …); ask after [`reach`],
/// which connects to it.
pub fn transport(runner: &dyn Runner, device: &Device) -> Result<Option<String>> {
    let out = runner.run(&kr_device::list_command())?;
    if !out.success() {
        return Ok(None);
    }
    let devices = kr_device::parse_list(&out.stdout)?;
    Ok(devices
        .into_iter()
        .find(|d| d.identifier == device.identifier)
        .and_then(|d| d.transport))
}
