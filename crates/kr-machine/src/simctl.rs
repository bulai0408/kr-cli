//! `xcrun simctl list … -j` output.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    /// "iOS 18.6"
    pub name: String,
    /// "com.apple.CoreSimulator.SimRuntime.iOS-18-6"
    pub identifier: String,
    pub version: String,
    #[serde(default)]
    pub platform: String,
    pub is_available: bool,
    #[serde(default)]
    pub supported_device_types: Vec<DeviceType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceType {
    /// "iPhone 16 Pro"
    pub name: String,
    /// "com.apple.CoreSimulator.SimDeviceType.iPhone-16-Pro"
    pub identifier: String,
}

impl DeviceType {
    pub fn matches(&self, name_or_id: &str) -> bool {
        self.name == name_or_id || self.identifier == name_or_id
    }
}

impl Runtime {
    pub fn matches(&self, name_or_id: &str) -> bool {
        self.name == name_or_id || self.identifier == name_or_id
    }
}

pub fn parse_runtimes(json: &str) -> Result<Vec<Runtime>> {
    #[derive(Deserialize)]
    struct List {
        runtimes: Vec<Runtime>,
    }
    let list: List = serde_json::from_str(json).context("cannot read `simctl list runtimes -j`")?;
    Ok(list.runtimes)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimDevice {
    pub name: String,
    pub udid: String,
    pub state: String,
    #[serde(default)]
    pub is_available: bool,
    #[serde(default)]
    pub device_type_identifier: String,
}

/// Devices by runtime identifier, as `xcrun simctl list devices -j` reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimDevices {
    pub devices: BTreeMap<String, Vec<SimDevice>>,
}

impl SimDevices {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("cannot read `simctl list devices -j`")
    }

    /// All devices with this name, with their runtime identifier.
    pub fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = (&'a str, &'a SimDevice)> {
        self.devices.iter().flat_map(move |(runtime, devices)| {
            devices
                .iter()
                .filter(move |d| d.name == name)
                .map(move |d| (runtime.as_str(), d))
        })
    }
}
