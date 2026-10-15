//! Machine profile (ADR 0003): what this Mac has and how busy it is, detected at run
//! time, plus `kr doctor`'s checks against a project's requirements.

mod doctor;
mod hardware;
mod simctl;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use kr_exec::{Command, Runner};
use serde::Serialize;

pub use doctor::{Problem, Severity, diagnose};
pub use hardware::{
    Derived, Hardware, Load, LoadWait, Pressure, Suggestion, read_hardware, sample_load, suggest,
    wait_for_load,
};
pub use simctl::{DeviceType, Runtime, SimDevice, SimDevices, parse_runtimes};

/// Where Xcode installs are looked for.
pub const APPLICATIONS: &str = "/Applications";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Profile {
    /// LocalHostName, used to pick `[machines.<host>]` settings.
    pub host: Option<String>,
    pub hardware: Hardware,
    /// Free space on the data volume.
    pub disk_free_bytes: u64,
    /// Installed Xcodes, newest first.
    pub xcodes: Vec<Xcode>,
    /// `DEVELOPER_DIR`, else `xcode-select -p`.
    pub developer_dir: Option<PathBuf>,
    pub runtimes: Vec<Runtime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Xcode {
    pub path: PathBuf,
    /// CFBundleShortVersionString, e.g. "27.1".
    pub version: String,
    /// ProductBuildVersion, e.g. "27A9269".
    pub build: String,
}

impl Xcode {
    pub fn developer_dir(&self) -> PathBuf {
        self.path.join("Contents/Developer")
    }
}

/// Detects everything `kr doctor` shows. `developer_dir` is `$DEVELOPER_DIR` if set.
pub fn detect(
    runner: &dyn Runner,
    apps_dir: &Path,
    developer_dir: Option<&str>,
) -> Result<Profile> {
    let hardware = read_hardware(runner)?;
    let host = host_name(runner);
    let disk_free_bytes = disk_free(runner)?;
    let developer_dir = match developer_dir {
        Some(dir) => Some(PathBuf::from(dir)),
        None => runner
            .run(&Command::new("xcode-select").arg("-p"))
            .ok()
            .filter(|o| o.success())
            .map(|o| PathBuf::from(o.stdout.trim())),
    };
    let runtimes = runner.run(&Command::new("xcrun").args(["simctl", "list", "runtimes", "-j"]))?;
    let runtimes = if runtimes.success() {
        parse_runtimes(&runtimes.stdout)?
    } else {
        Vec::new()
    };
    Ok(Profile {
        host,
        hardware,
        disk_free_bytes,
        xcodes: find_xcodes(apps_dir),
        developer_dir,
        runtimes,
    })
}

/// Host name only, for picking per-machine settings cheaply.
pub fn host_name(runner: &dyn Runner) -> Option<String> {
    runner
        .run(&Command::new("scutil").args(["--get", "LocalHostName"]))
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout.trim().to_owned())
        .filter(|h| !h.is_empty())
}

/// Available space from `df -kP /`. On APFS every volume of the container shares it.
fn disk_free(runner: &dyn Runner) -> Result<u64> {
    let out = runner.run(&Command::new("df").args(["-kP", "/"]))?;
    let kb: u64 = out
        .stdout
        .lines()
        .nth(1)
        .and_then(|l| l.split_whitespace().nth(3))
        .and_then(|n| n.parse().ok())
        .context("cannot read free disk space from `df -kP /`")?;
    Ok(kb * 1024)
}

/// `Xcode*.app` bundles in `apps_dir`, newest version first.
pub fn find_xcodes(apps_dir: &Path) -> Vec<Xcode> {
    let Ok(entries) = std::fs::read_dir(apps_dir) else {
        return Vec::new();
    };
    let mut xcodes: Vec<Xcode> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("Xcode") && n.ends_with(".app"))
        })
        .filter_map(|path| {
            let info: plist::Dictionary =
                plist::from_file(path.join("Contents/version.plist")).ok()?;
            let text = |k: &str| info.get(k).and_then(|v| v.as_string()).map(str::to_owned);
            Some(Xcode {
                version: text("CFBundleShortVersionString")?,
                build: text("ProductBuildVersion").unwrap_or_default(),
                path,
            })
        })
        .collect();
    xcodes.sort_by_key(|x| std::cmp::Reverse(version_key(&x.version)));
    xcodes
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// The newest installed Xcode whose version starts with `required`, compared by
/// components: "27" matches 27.1, "27.1" matches 27.1.2, "27.10" does not match 27.1.
pub fn find_xcode<'a>(xcodes: &'a [Xcode], required: &str) -> Option<&'a Xcode> {
    let want = version_key(required);
    xcodes
        .iter()
        .find(|x| version_key(&x.version).starts_with(&want))
}
