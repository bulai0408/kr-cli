use std::path::{Path, PathBuf};
use std::time::Duration;

use kr_config::ProjectConfig;
use kr_exec::{Recording, ReplayRunner};
use kr_machine::{SimDevices, parse_runtimes};
use kr_sim::{DeviceLocks, LeaseHolder, Simctl, plan, sync};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(rel)
}

fn rec(rel: &str) -> Recording {
    kr_exec::read_recording(&fixture(rel)).unwrap()
}

/// A real recording with the device it was made on swapped for another UDID or name.
fn rec_for(rel: &str, from: &str, to: &str) -> Recording {
    let mut r = rec(rel);
    for a in &mut r.args {
        *a = a.replace(from, to);
    }
    r.stdout = r.stdout.replace(from, to);
    r
}

const RECORDED_UDID: &str = "2A3B86EB-E206-4C43-9C35-0F8124258FB6";
const KRILL_1: &str = "6BFDF46B-1778-4C7E-B901-4E825CB78C4D";
const KRILL_2: &str = "DF280713-8C80-4056-8737-4E3B90B6B933";

fn runtimes() -> Vec<kr_machine::Runtime> {
    parse_runtimes(&rec("simctl/list-runtimes.json").stdout).unwrap()
}

fn devices() -> SimDevices {
    SimDevices::parse(&rec("simctl/list-devices.json").stdout).unwrap()
}

fn krill(count: u32) -> ProjectConfig {
    toml::from_str(&format!(
        r#"
[project]
name = "Krill"

[simulators]
runtime = "iOS 18.6"

[simulators.devices.default]
device_type = "iPhone 16 Pro"
count = {count}

[simulators.devices.iPad]
device_type = "iPad Pro 13-inch (M4) (16GB)"
"#
    ))
    .unwrap()
}

fn holder(task: &str) -> LeaseHolder {
    LeaseHolder {
        project: Some("Krill".into()),
        task: task.into(),
        pid: std::process::id(),
        started: jiff::Timestamp::now(),
    }
}

#[test]
fn plan_finds_existing_pool_devices_by_name_and_runtime() {
    let devices = plan(&krill(3), &runtimes(), &devices()).unwrap();
    let found: Vec<_> = devices
        .iter()
        .map(|d| (d.name.as_str(), d.udid.is_some()))
        .collect();
    // Krill-Verify-3 exists, but on iOS 27.0: not this pool's device.
    assert_eq!(
        found,
        [
            ("Krill-Verify-1", true),
            ("Krill-Verify-2", true),
            ("Krill-Verify-3", false),
            ("Krill-Verify-iPad-1", true)
        ]
    );
    assert_eq!(devices[0].udid.as_deref(), Some(KRILL_1));
    assert_eq!(devices[3].kind, "iPad");
}

#[test]
fn plan_flags_a_pool_name_with_the_wrong_device_type() {
    let json = rec("simctl/list-devices.json").stdout.replacen(
        "com.apple.CoreSimulator.SimDeviceType.iPhone-16-Pro",
        "com.apple.CoreSimulator.SimDeviceType.iPhone-16e",
        3,
    );
    let devices = plan(&krill(2), &runtimes(), &SimDevices::parse(&json).unwrap()).unwrap();
    let issues: Vec<_> = devices.iter().filter_map(|d| d.issue.as_deref()).collect();
    assert!(
        issues.iter().any(|i| i.contains("iPhone-16e")),
        "{issues:?}"
    );
}

#[test]
fn sync_creates_only_missing_pool_devices() {
    let runner = ReplayRunner::new(vec![
        rec("simctl/list-runtimes.json"),
        rec("simctl/list-devices.json"),
        rec_for("simctl/create.json", "KrTest-Verify-2", "Krill-Verify-3"),
    ]);
    let report = sync(&Simctl::new(&runner, None), &krill(3)).unwrap();

    assert_eq!(report.created, ["Krill-Verify-3"]);
    assert_eq!(report.existing.len(), 3);
    assert!(report.problems.is_empty());
    assert_eq!(
        runner.calls()[2],
        "xcrun simctl create Krill-Verify-3 iPhone 16 Pro com.apple.CoreSimulator.SimRuntime.iOS-18-6"
    );
}

#[test]
fn sync_reports_a_failed_create_with_simctls_reason() {
    // The recorded refusal ("Invalid device type"), answered for the pool's create call.
    let mut refused = rec("simctl/create-bad-type.json");
    refused.args = [
        "simctl",
        "create",
        "Krill-Verify-3",
        "iPhone 16 Pro",
        "com.apple.CoreSimulator.SimRuntime.iOS-18-6",
    ]
    .map(String::from)
    .to_vec();
    let runner = ReplayRunner::new(vec![
        rec("simctl/list-runtimes.json"),
        rec("simctl/list-devices.json"),
        refused,
    ]);

    let report = sync(&Simctl::new(&runner, None), &krill(3)).unwrap();

    assert!(report.created.is_empty());
    assert!(
        report.problems[0].contains("Invalid device type"),
        "{:?}",
        report.problems
    );
}

#[test]
fn sync_never_touches_devices_outside_the_pool() {
    let runner = ReplayRunner::new(vec![
        rec("simctl/list-runtimes.json"),
        rec("simctl/list-devices.json"),
    ]);
    sync(&Simctl::new(&runner, None), &krill(2)).unwrap();
    assert_eq!(
        runner.calls(),
        [
            "xcrun simctl list runtimes -j",
            "xcrun simctl list devices -j"
        ]
    );
}

#[test]
fn a_lease_boots_the_first_free_device_and_shuts_it_down_on_release() {
    let dir = tempfile::tempdir().unwrap();
    let locks = DeviceLocks::new(dir.path());
    let pool = plan(&krill(2), &runtimes(), &devices()).unwrap();
    let runner = ReplayRunner::new(vec![
        rec_for(
            "simctl/shutdown-already-shutdown.json",
            RECORDED_UDID,
            KRILL_1,
        ),
        rec_for("simctl/bootstatus.json", RECORDED_UDID, KRILL_1),
        rec_for("simctl/shutdown.json", RECORDED_UDID, KRILL_1),
    ]);
    let simctl = Simctl::new(&runner, None);

    let lease = kr_sim::lease(
        &simctl,
        &locks,
        &pool,
        "default",
        &holder("ratings"),
        Duration::ZERO,
        Some(Duration::ZERO),
    )
    .unwrap();
    assert_eq!((lease.name(), lease.udid()), ("Krill-Verify-1", KRILL_1));
    assert_eq!(locks.holder(KRILL_1).unwrap().task, "ratings");
    lease.release().unwrap();

    assert!(locks.holder(KRILL_1).is_none());
    assert_eq!(
        runner.calls(),
        [
            format!("xcrun simctl shutdown {KRILL_1}"),
            format!("xcrun simctl bootstatus {KRILL_1} -b"),
            format!("xcrun simctl shutdown {KRILL_1}"),
        ]
    );
}

#[test]
fn a_second_lease_gets_the_next_device_and_a_third_waits() {
    let dir = tempfile::tempdir().unwrap();
    let locks = DeviceLocks::new(dir.path());
    let pool = plan(&krill(2), &runtimes(), &devices()).unwrap();
    let runner = ReplayRunner::new(vec![
        rec_for(
            "simctl/shutdown-already-shutdown.json",
            RECORDED_UDID,
            KRILL_1,
        ),
        rec_for("simctl/bootstatus.json", RECORDED_UDID, KRILL_1),
        rec_for(
            "simctl/shutdown-already-shutdown.json",
            RECORDED_UDID,
            KRILL_2,
        ),
        rec_for("simctl/bootstatus.json", RECORDED_UDID, KRILL_2),
        rec_for("simctl/shutdown.json", RECORDED_UDID, KRILL_1),
        rec_for("simctl/shutdown.json", RECORDED_UDID, KRILL_2),
    ]);
    let simctl = Simctl::new(&runner, None);
    let wait = Some(Duration::from_millis(50));
    let a = kr_sim::lease(
        &simctl,
        &locks,
        &pool,
        "default",
        &holder("a"),
        Duration::from_millis(10),
        wait,
    )
    .unwrap();
    let b = kr_sim::lease(
        &simctl,
        &locks,
        &pool,
        "default",
        &holder("b"),
        Duration::from_millis(10),
        wait,
    )
    .unwrap();
    assert_eq!(b.name(), "Krill-Verify-2");
    let c = kr_sim::lease(
        &simctl,
        &locks,
        &pool,
        "default",
        &holder("c"),
        Duration::from_millis(10),
        wait,
    );
    let err = c.err().expect("all devices are leased").to_string();
    assert!(err.contains("no free default simulator"), "{err}");
    drop(a);
    drop(b);
}

#[test]
fn a_device_left_booted_by_a_crashed_holder_is_shut_down_before_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let locks = DeviceLocks::new(dir.path());
    let pool = plan(&krill(2), &runtimes(), &devices()).unwrap();
    // The previous holder died: its lock is gone, the device still runs.
    let runner = ReplayRunner::new(vec![
        rec_for("simctl/shutdown.json", RECORDED_UDID, KRILL_1),
        rec_for("simctl/bootstatus.json", RECORDED_UDID, KRILL_1),
        rec_for("simctl/shutdown.json", RECORDED_UDID, KRILL_1),
    ]);
    let simctl = Simctl::new(&runner, None);
    let lease = kr_sim::lease(
        &simctl,
        &locks,
        &pool,
        "default",
        &holder("x"),
        Duration::ZERO,
        Some(Duration::ZERO),
    )
    .unwrap();
    lease.release().unwrap();
    assert_eq!(runner.calls().len(), 3);
}

#[test]
fn simctl_runs_with_the_projects_developer_dir() {
    let runner = ReplayRunner::new(vec![]);
    let simctl = Simctl::new(
        &runner,
        Some(PathBuf::from(
            "/Applications/Xcode_26.6.app/Contents/Developer",
        )),
    );
    let cmd = simctl.command(["list", "devices", "-j"]);
    assert_eq!(
        cmd.env,
        [(
            "DEVELOPER_DIR".to_owned(),
            "/Applications/Xcode_26.6.app/Contents/Developer".to_owned()
        )]
    );
}
