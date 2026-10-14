//! Real `simctl`: creates, boots and shuts down a throwaway pool device.
//!
//! Off by default. It boots a simulator, so run it as heavy work, e.g.
//! `KR_INTEGRATION=1 kr lock sim -- cargo test -p kr-sim --test integration`
//! (or under the machine's existing heavy-work lock).

use std::time::Duration;

use kr_config::ProjectConfig;
use kr_exec::{Command, RealRunner, Runner};
use kr_sim::{DeviceLocks, LeaseHolder, Simctl, plan, sync};

fn project() -> ProjectConfig {
    toml::from_str(
        r#"
[project]
name = "KrIntegration"

[simulators]
runtime = "iOS 18.6"

[simulators.devices.default]
device_type = "iPhone 16 Pro"
"#,
    )
    .unwrap()
}

fn state_of(simctl: &Simctl<'_>, udid: &str) -> String {
    let devices = simctl.devices().unwrap();
    devices
        .devices
        .values()
        .flatten()
        .find(|d| d.udid == udid)
        .map(|d| d.state.clone())
        .unwrap_or_default()
}

/// Deletes this test's own devices, whatever happens.
struct Cleanup;

impl Drop for Cleanup {
    fn drop(&mut self) {
        let simctl = Simctl::new(&RealRunner, None);
        if let Ok(devices) = simctl.devices() {
            for d in devices.devices.values().flatten() {
                if d.name.starts_with("KrIntegration-Verify-") {
                    let _ = RealRunner.run(&Command::new("xcrun").args([
                        "simctl",
                        "delete",
                        d.udid.as_str(),
                    ]));
                }
            }
        }
    }
}

#[test]
fn real_pool_sync_lease_and_release() {
    if std::env::var("KR_INTEGRATION").as_deref() != Ok("1") {
        eprintln!("skipped: set KR_INTEGRATION=1 to run against real simctl");
        return;
    }
    let _cleanup = Cleanup;
    let simctl = Simctl::new(&RealRunner, None);
    let project = project();

    let report = sync(&simctl, &project).unwrap();
    assert_eq!(report.created, ["KrIntegration-Verify-1"], "{report:?}");
    assert_eq!(
        sync(&simctl, &project).unwrap().existing,
        ["KrIntegration-Verify-1"]
    );

    let devices = plan(
        &project,
        &simctl.runtimes().unwrap(),
        &simctl.devices().unwrap(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let holder = LeaseHolder {
        project: None,
        task: "integration".into(),
        pid: std::process::id(),
        started: jiff::Timestamp::now(),
    };
    let lease = kr_sim::lease(
        &simctl,
        &DeviceLocks::new(dir.path()),
        &devices,
        "default",
        &holder,
        Duration::from_secs(1),
        Some(Duration::ZERO),
    )
    .unwrap();
    let udid = lease.udid().to_owned();
    assert_eq!(state_of(&simctl, &udid), "Booted");

    lease.release().unwrap();
    assert_eq!(state_of(&simctl, &udid), "Shutdown");
}
