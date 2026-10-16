//! The simulator pool (spec §3): devices named `<project>-Verify-<N>` that kr creates,
//! boots, shuts down and leases. kr never touches a device outside the pool.
//!
//! A pool device is identified by name *and* runtime: a device with a pool name on
//! another runtime (e.g. an old `Krill-Verify-3` on iOS 27.0) is not part of the pool.
//!
//! A lease holds `<state>/sims/<udid>.lock` with `File::try_lock`, so a crashed holder
//! frees the device; the next holder shuts it down before booting it again.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use kr_config::ProjectConfig;
use kr_exec::{Command, Output, Runner};
use kr_machine::{Runtime, SimDevices, parse_runtimes};
use serde::{Deserialize, Serialize};

/// `simctl` through the runner, with the project's Xcode.
pub struct Simctl<'a> {
    runner: &'a dyn Runner,
    developer_dir: Option<PathBuf>,
}

impl<'a> Simctl<'a> {
    pub fn new(runner: &'a dyn Runner, developer_dir: Option<PathBuf>) -> Self {
        Self {
            runner,
            developer_dir,
        }
    }

    pub fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut cmd = Command::new("xcrun").arg("simctl").args(args);
        if let Some(dir) = &self.developer_dir {
            cmd = cmd.env("DEVELOPER_DIR", dir.display().to_string());
        }
        cmd
    }

    fn run<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.runner.run(&self.command(args))
    }

    pub fn runtimes(&self) -> Result<Vec<Runtime>> {
        let out = self.run(["list", "runtimes", "-j"])?;
        ensure_ok(&out, "simctl list runtimes")?;
        parse_runtimes(&out.stdout)
    }

    pub fn devices(&self) -> Result<SimDevices> {
        let out = self.run(["list", "devices", "-j"])?;
        ensure_ok(&out, "simctl list devices")?;
        SimDevices::parse(&out.stdout)
    }

    fn create(&self, name: &str, device_type: &str, runtime_id: &str) -> Result<String> {
        let out = self.run(["create", name, device_type, runtime_id])?;
        ensure_ok(&out, &format!("simctl create {name}"))?;
        Ok(out.stdout.trim().to_owned())
    }

    /// Boots the device if needed and waits until it has finished booting.
    pub fn boot(&self, udid: &str) -> Result<()> {
        let out = self.run(["bootstatus", udid, "-b"])?;
        ensure_ok(&out, &format!("simctl bootstatus {udid} -b"))
    }

    /// Shuts the device down; a device that is already shut down is fine.
    pub fn shutdown(&self, udid: &str) -> Result<()> {
        let out = self.run(["shutdown", udid])?;
        if out.success() || out.stderr.contains("current state: Shutdown") {
            return Ok(());
        }
        ensure_ok(&out, &format!("simctl shutdown {udid}"))
    }
}

fn ensure_ok(out: &Output, what: &str) -> Result<()> {
    if out.success() {
        return Ok(());
    }
    let reason = out.stderr.lines().last().unwrap_or("").trim();
    bail!("`{what}` failed (exit {:?}): {reason}", out.status)
}

/// One device the project's pool should have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PoolDevice {
    pub kind: String,
    pub name: String,
    pub device_type: String,
    /// Runtime name from the config, e.g. "iOS 18.6".
    pub runtime: String,
    /// `None` if the runtime is not installed.
    pub runtime_id: Option<String>,
    /// `None` if the device does not exist yet.
    pub udid: Option<String>,
    pub state: Option<String>,
    /// Why the existing device cannot be used as is.
    pub issue: Option<String>,
}

/// The pool devices of `project` and which of them exist.
pub fn plan(
    project: &ProjectConfig,
    runtimes: &[Runtime],
    devices: &SimDevices,
) -> Result<Vec<PoolDevice>> {
    let mut out = Vec::new();
    for (kind_name, kind) in &project.simulators.devices {
        let runtime = kind
            .runtime
            .as_ref()
            .or(project.simulators.runtime.as_ref())
            .with_context(|| format!("simulators.devices.{kind_name} has no runtime"))?;
        let rt = runtimes
            .iter()
            .find(|r| r.is_available && r.matches(runtime));
        let wanted_type = rt.and_then(|rt| {
            rt.supported_device_types
                .iter()
                .find(|t| t.matches(&kind.device_type))
        });
        for name in project.device_names(kind_name) {
            let existing = rt.and_then(|rt| {
                devices
                    .named(&name)
                    .find(|(id, d)| *id == rt.identifier && d.is_available)
                    .map(|(_, d)| d.clone())
            });
            let issue = match (&existing, wanted_type) {
                (Some(d), Some(t)) if d.device_type_identifier != t.identifier => Some(format!(
                    "{name} is a {} but the pool wants {}; delete it (`xcrun simctl delete {}`) and run `kr pool sync`",
                    d.device_type_identifier, t.identifier, d.udid
                )),
                _ => None,
            };
            out.push(PoolDevice {
                kind: kind_name.clone(),
                name,
                device_type: kind.device_type.clone(),
                runtime: runtime.clone(),
                runtime_id: rt.map(|r| r.identifier.clone()),
                udid: existing.as_ref().map(|d| d.udid.clone()),
                state: existing.as_ref().map(|d| d.state.clone()),
                issue,
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SyncReport {
    pub created: Vec<String>,
    pub existing: Vec<String>,
    /// Devices that could not be created or are wrong; kr leaves them alone.
    pub problems: Vec<String>,
}

/// Creates the pool devices that do not exist. Never deletes or changes others.
pub fn sync(simctl: &Simctl<'_>, project: &ProjectConfig) -> Result<SyncReport> {
    let runtimes = simctl.runtimes()?;
    let devices = simctl.devices()?;
    let mut report = SyncReport::default();
    for device in plan(project, &runtimes, &devices)? {
        if let Some(issue) = device.issue {
            report.problems.push(issue);
            continue;
        }
        if device.udid.is_some() {
            report.existing.push(device.name);
            continue;
        }
        let Some(runtime_id) = device.runtime_id else {
            report.problems.push(format!(
                "{}: runtime {} is not installed (see `kr doctor`)",
                device.name, device.runtime
            ));
            continue;
        };
        match simctl.create(&device.name, &device.device_type, &runtime_id) {
            Ok(_) => report.created.push(device.name),
            Err(e) => report.problems.push(format!("{}: {e:#}", device.name)),
        }
    }
    Ok(report)
}

/// Who holds a device, as written into its lock file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseHolder {
    pub project: Option<String>,
    pub task: String,
    pub pid: u32,
    pub started: Timestamp,
}

/// Per-device lock files.
#[derive(Debug, Clone)]
pub struct DeviceLocks {
    dir: PathBuf,
}

impl DeviceLocks {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, udid: &str) -> PathBuf {
        self.dir.join(format!("{udid}.lock"))
    }

    fn try_lock(&self, udid: &str, holder: &LeaseHolder) -> Result<Option<File>> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(udid);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {
                file.set_len(0)?;
                file.write_all(&serde_json::to_vec(holder)?)?;
                Ok(Some(file))
            }
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => {
                Err(e).with_context(|| format!("failed to lock {}", path.display()))
            }
        }
    }

    /// The current holder of a device, if it is leased.
    pub fn holder(&self, udid: &str) -> Option<LeaseHolder> {
        let path = self.path(udid);
        let file = File::open(&path).ok()?;
        if !matches!(file.try_lock(), Err(TryLockError::WouldBlock)) {
            return None;
        }
        serde_json::from_slice(&std::fs::read(&path).ok()?).ok()
    }
}

/// A booted pool device reserved for one task. Shut down and freed on release or drop.
pub struct Lease<'a> {
    simctl: &'a Simctl<'a>,
    device: PoolDevice,
    udid: String,
    lock: Option<File>,
}

impl Lease<'_> {
    pub fn udid(&self) -> &str {
        &self.udid
    }

    pub fn name(&self) -> &str {
        &self.device.name
    }

    pub fn kind(&self) -> &str {
        &self.device.kind
    }

    /// Shuts the device down and frees it.
    pub fn release(mut self) -> Result<()> {
        let result = self.simctl.shutdown(&self.udid);
        if let Some(lock) = self.lock.take() {
            let _ = lock.set_len(0);
        }
        result
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(lock) = self.lock.take() {
            let _ = self.simctl.shutdown(&self.udid);
            let _ = lock.set_len(0);
        }
    }
}

/// Leases a device of `kind` from `pool`: the first one whose lock is free. Shuts it
/// down first (a crashed holder may have left it running), then boots it. Polls every
/// `poll` while all are taken; gives up after `wait` if given.
pub fn lease<'a>(
    simctl: &'a Simctl<'a>,
    locks: &DeviceLocks,
    pool: &[PoolDevice],
    kind: &str,
    holder: &LeaseHolder,
    poll: Duration,
    wait: Option<Duration>,
) -> Result<Lease<'a>> {
    lease_with_wait_notice(simctl, locks, pool, kind, holder, poll, wait, |_, _| {})
}

/// Like [`lease`], with a notice for each busy candidate while waiting.
#[allow(clippy::too_many_arguments)]
pub fn lease_with_wait_notice<'a>(
    simctl: &'a Simctl<'a>,
    locks: &DeviceLocks,
    pool: &[PoolDevice],
    kind: &str,
    holder: &LeaseHolder,
    poll: Duration,
    wait: Option<Duration>,
    mut notice: impl FnMut(&PoolDevice, Option<LeaseHolder>),
) -> Result<Lease<'a>> {
    let candidates: Vec<&PoolDevice> = pool
        .iter()
        .filter(|d| d.kind == kind && d.udid.is_some() && d.issue.is_none())
        .collect();
    if candidates.is_empty() {
        bail!("the pool has no usable {kind} simulator; run `kr pool sync` and `kr doctor`");
    }
    let start = Instant::now();
    loop {
        for device in &candidates {
            let udid = device.udid.as_deref().expect("filtered above");
            if let Some(lock) = locks.try_lock(udid, holder)? {
                let lease = Lease {
                    simctl,
                    device: (*device).clone(),
                    udid: udid.to_owned(),
                    lock: Some(lock),
                };
                simctl.shutdown(udid)?;
                simctl.boot(udid)?;
                return Ok(lease);
            }
            notice(device, locks.holder(udid));
        }
        if wait.is_some_and(|w| start.elapsed() >= w) {
            bail!(
                "no free {kind} simulator after {:?}; all {} are leased",
                start.elapsed(),
                candidates.len()
            );
        }
        std::thread::sleep(poll);
    }
}

/// `<state>/sims`.
pub fn locks_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("sims")
}
