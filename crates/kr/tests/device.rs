//! Real devices (#36): `kr doctor --device`, `kr check --device` and `kr run --device`
//! against recorded devicectl, xcodebuild and codesign outputs (tests/fixtures/devicectl).

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use kr_exec::Recording;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

const IPAD: &str = "蔡煜恒的 iPad Mini";

/// Machine and pool recordings `kr doctor` needs, plus the given ones.
fn doctor_replay(extra: &[&str]) -> String {
    let f = fixtures();
    [
        "sysctl/m3-imac.json",
        "scutil/local-host-name.json",
        "df/root.json",
        "xcode-select/print-path.json",
        "simctl/list-runtimes.json",
        "simctl/list-devices.json",
        "xcrun/sdk-version-iphoneos.json",
    ]
    .iter()
    .chain(extra)
    .map(|r| f.join(r).display().to_string())
    .collect::<Vec<_>>()
    .join(":")
}

fn doctor(replay: &str, args: &[&str]) -> (Option<i32>, String) {
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::copy(fixtures().join("../sample/.kr.toml"), repo.join(".kr.toml")).unwrap();
    let f = fixtures();
    let out = Command::cargo_bin("kr")
        .unwrap()
        .env_clear()
        .env("HOME", home.path())
        .env("NO_COLOR", "1")
        .env("KR_REPLAY", replay)
        .env("KR_APPLICATIONS", f.join("xcode/apps"))
        .current_dir(&repo)
        .arg("doctor")
        .args(args)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code(), text)
}

/// The device part of doctor's human output: the verdict, each device problem with
/// its fix, and the device line.
fn device_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut keep = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let after_device_problem = i > 0 && lines[i - 1].contains(" device_");
        if l.starts_with("doctor:")
            || l.contains(" device_")
            || after_device_problem
            || l.starts_with("device ")
            || l.starts_with("error")
        {
            keep.push(*l);
        }
    }
    keep.join("\n")
}

#[test]
fn doctor_says_what_to_do_on_the_device_for_each_missing_prerequisite() {
    let (code, text) = doctor(
        &doctor_replay(&["devicectl/list-devices-missing-prerequisites.json"]),
        &["--device", IPAD],
    );
    assert_eq!(code, Some(1), "{text}");
    insta::assert_snapshot!(device_lines(&text));
}

#[test]
fn doctor_passes_a_ready_device_and_warns_that_its_ios_is_newer_than_the_sdk() {
    // Recorded with the iPad on a cable and unlocked: iOS 27.2 against Xcode 27.1.
    let (code, text) = doctor(
        &doctor_replay(&[
            "devicectl/list-devices-connected.json",
            "devicectl/lock-state-unlocked.json",
            // Read again after connecting, for how it is connected now.
            "devicectl/list-devices-connected.json",
        ]),
        &["--device", IPAD],
    );
    assert_eq!(code, Some(0), "{text}");
    insta::assert_snapshot!(device_lines(&text));
}

#[test]
fn doctor_says_to_unlock_a_locked_device() {
    let (code, text) = doctor(
        &doctor_replay(&[
            "devicectl/list-devices-connected.json",
            "devicectl/lock-state-locked.json",
        ]),
        &["--device", IPAD],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("error device_locked: 蔡煜恒的 iPad Mini is locked"),
        "{text}"
    );
    assert!(text.contains("fix: On the iPad: unlock it"), "{text}");
}

#[test]
fn doctor_json_has_the_device_and_says_ui_automation_cannot_be_read() {
    let (code, text) = doctor(
        &doctor_replay(&[
            "devicectl/list-devices-connected.json",
            "devicectl/lock-state-unlocked.json",
            // Read again after connecting, for how it is connected now.
            "devicectl/list-devices-connected.json",
        ]),
        &["--device", IPAD, "--json"],
    );
    assert_eq!(code, Some(0), "{text}");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let device = &json["device"];
    assert_eq!(device["name"], IPAD);
    assert_eq!(device["udid"], "00008130-00AA00AA00AA001C");
    assert_eq!(device["os_version"], "27.2");
    assert_eq!(device["developer_mode"], true);
    assert_eq!(device["reach"], "unlocked");
    assert_eq!(device["ui_automation"], "unknown");
    let codes: Vec<&str> = json["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["code"].as_str())
        .collect();
    assert!(codes.contains(&"device_os_newer"), "{codes:?}");
}

#[test]
fn an_unknown_device_name_lists_the_paired_devices() {
    let (code, text) = doctor(
        &doctor_replay(&["devicectl/list-devices-connected.json"]),
        &["--device", "Kami's iPad"],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("no paired device `Kami's iPad`; devices this Mac knows: 蔡煜恒的 iPad Mini"),
        "{text}"
    );
}

/// A recorded `kr run --device` (tests/fixtures/device/<name>), in order.
fn device_recordings(name: &str) -> Vec<Recording> {
    let dir = fixtures().join("device").join(name);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    files
        .iter()
        .map(|f| kr_exec::read_recording(f).unwrap())
        .collect()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The sample project with its test products and the signing team it is built with.
struct Sample {
    home: tempfile::TempDir,
}

impl Sample {
    fn new() -> Self {
        Self::with_signing("")
    }

    /// With more lines in [signing].
    fn with_signing(extra: &str) -> Self {
        Self::with(extra, "")
    }

    fn with(signing: &str, scenario: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        // As recorded: signed by the team, for a device.
        let config = std::fs::read_to_string(fixtures().join("../sample/.kr.toml"))
            .unwrap()
            .replace(
                r#"build_settings = { KR_SAMPLE_BUNDLE_ID = "dev.kr.sample.e2e" }"#,
                r#"build_settings = { KR_SAMPLE_BUNDLE_ID = "dev.kr.sample.e2e", DEVELOPMENT_TEAM = "K4AXH95DCF", CODE_SIGN_IDENTITY = "Apple Development" }"#,
            );
        assert!(
            config.contains("DEVELOPMENT_TEAM"),
            "the sample's build settings changed"
        );
        std::fs::write(
            repo.join(".kr.toml"),
            config + "\n" + scenario + "\n[signing]\nteam = \"K4AXH95DCF\"\n" + signing,
        )
        .unwrap();
        copy_dir(
            &fixtures().join("xcode/products"),
            &repo.join("build/kr/DerivedData/Build/Products"),
        );
        Self { home }
    }

    /// Runs kr on `recordings`; stdout and stderr together.
    fn kr(&self, recordings: &[Recording], args: &[&str]) -> (Option<i32>, String) {
        let dir = self.home.path().join("replay");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<String> = recordings
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let p = dir.join(format!("{i:03}.json"));
                std::fs::write(&p, serde_json::to_string(r).unwrap()).unwrap();
                p.display().to_string()
            })
            .collect();
        let out = Command::cargo_bin("kr")
            .unwrap()
            .env_clear()
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .env("KR_REPLAY", files.join(":"))
            .env("KR_APPLICATIONS", fixtures().join("xcode/apps"))
            .env("KR_RESOURCES__BUILD", "1")
            .current_dir(self.home.path().join("repo"))
            .args(args)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.code(), text)
    }

    fn events(&self) -> Vec<serde_json::Value> {
        let dir = self.home.path().join(".local/state/kr/events");
        let mut all = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
            all.extend(
                text.lines()
                    .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()),
            );
        }
        all
    }
}

#[test]
fn a_test_runner_the_device_refuses_fails_with_xcodebuilds_reason() {
    // Real: the profile automatic signing made did not include the iPad.
    let sample = Sample::new();
    let (code, text) = sample.kr(
        &device_recordings("runner-not-installed"),
        &["run", "--device", IPAD, "launch"],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("FAIL: launch (0 tests, 蔡煜恒的 iPad Mini"),
        "{text}"
    );
    assert!(
        text.contains(
            "xcodebuild could not list the tests: KrSampleUITests-Runner encountered an error."
        ) && text
            .contains("0xe8008012 (This provisioning profile cannot be installed on this device.)"),
        "{text}"
    );
    assert!(!text.contains("selects no test on the device"), "{text}");
    assert!(
        text.contains("note: the provisioning profile does not include 蔡煜恒的 iPad Mini: register it in team K4AXH95DCF (Xcode > Window > Devices and Simulators), or set [signing] register_devices = true in .kr.toml"),
        "{text}"
    );
    assert!(
        !sample
            .events()
            .iter()
            .any(|e| e["type"] == "filter_mismatch"),
        "an enumeration error is not a filter mismatch"
    );
    assert!(sample.events().iter().any(|e| e["type"] == "item_start"));
}

/// The recorded run with the codesign output of the app edited.
fn signed(edit: impl Fn(&mut Recording)) -> Vec<Recording> {
    let mut recs = device_recordings("runner-not-installed");
    for r in recs.iter_mut().filter(|r| r.program == "codesign") {
        edit(r);
    }
    recs
}

#[test]
fn an_app_signed_for_icloud_production_stops_before_installing() {
    let sample = Sample::new();
    let recs = signed(|r| {
        if r.args.contains(&"--entitlements".to_owned()) {
            r.stdout = r.stdout.replace(
                "<key>get-task-allow</key>",
                "<key>com.apple.developer.icloud-container-identifiers</key><array><string>iCloud.dev.kr.sample</string></array>\
                 <key>com.apple.developer.icloud-container-environment</key><string>Production</string>\
                 <key>get-task-allow</key>",
            );
        }
    });
    let (code, text) = sample.kr(&recs, &["run", "--device", IPAD, "launch"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("KrSample.app is signed for the iCloud Production environment; tests may only use [\"Development\"]"),
        "{text}"
    );
    assert!(
        text.contains("set com.apple.developer.icloud-container-environment to `Development` in the app's entitlements file"),
        "{text}"
    );
    let installed = sample.events().iter().any(|e| e["type"] == "item_start");
    assert!(!installed, "nothing may be installed: {text}");
}

#[test]
fn an_app_signed_by_another_team_stops_before_installing() {
    let sample = Sample::new();
    let recs = signed(|r| {
        r.stderr = r
            .stderr
            .replace("TeamIdentifier=K4AXH95DCF", "TeamIdentifier=ZZZZ999999");
    });
    let (code, text) = sample.kr(&recs, &["run", "--device", IPAD, "launch"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("KrSample.app is signed by team ZZZZ999999, not K4AXH95DCF"),
        "{text}"
    );
    assert!(text.contains("set DEVELOPMENT_TEAM = K4AXH95DCF"), "{text}");
    let installed = sample.events().iter().any(|e| e["type"] == "item_start");
    assert!(!installed, "nothing may be installed: {text}");
}

#[test]
fn check_on_a_device_with_missing_prerequisites_refuses_before_building() {
    let sample = Sample::new();
    let f = fixtures();
    let recs: Vec<Recording> = [
        "devicectl/list-devices-missing-prerequisites.json",
        "xcrun/sdk-version-iphoneos.json",
    ]
    .iter()
    .map(|r| kr_exec::read_recording(&f.join(r)).unwrap())
    .collect();
    let (code, text) = sample.kr(&recs, &["check", "--device", IPAD, "launch"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        text.contains("cannot run tests on 蔡煜恒的 iPad Mini"),
        "{text}"
    );
    assert!(
        text.contains("On the iPad: Settings > Privacy & Security > Developer Mode"),
        "{text}"
    );
    assert!(
        text.contains("蔡煜恒的 iPad Mini runs iOS 28.0; Xcode 27.1 only knows iOS 27.1"),
        "{text}"
    );
    // Only the two recordings above: a build would have no recording and fail differently.
    assert!(!text.contains("no recording"), "{text}");
}

/// `devicectl device info lockState` failing: the device is gone.
fn lost(r: &mut Recording) {
    let lost = kr_exec::read_recording(&fixtures().join("devicectl/lock-state-lost.json")).unwrap();
    r.status = lost.status;
    r.stdout = lost.stdout;
    r.stderr = lost.stderr;
}

/// The recorded run with the `n`-th lockState call (0: before the run) edited.
fn lock_state_edited(n: usize, edit: impl Fn(&mut Recording)) -> Vec<Recording> {
    let mut recs = device_recordings("runner-not-installed");
    if let Some(r) = recs
        .iter_mut()
        .filter(|r| r.args.contains(&"lockState".to_owned()))
        .nth(n)
    {
        edit(r);
    }
    recs
}

fn item_end(sample: &Sample) -> serde_json::Value {
    sample
        .events()
        .into_iter()
        .find(|e| e["type"] == "item_end")
        .unwrap_or_else(|| panic!("no item_end in {:#?}", sample.events()))
}

#[test]
fn a_device_lost_before_an_item_makes_it_unverified() {
    let sample = Sample::new();
    let (code, text) = sample.kr(
        &lock_state_edited(1, lost),
        &["run", "--device", IPAD, "launch"],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("UNVERIFIED: launch"), "{text}");
    assert!(!text.contains("PASS: launch"), "{text}");
    let end = item_end(&sample);
    assert_eq!(end["fields"]["tests"]["unverified"], 1, "{end}");
    let why = end["fields"]["unverified_reason"].as_str().unwrap();
    assert!(
        why.starts_with("lost 蔡煜恒的 iPad Mini: ") && why.ends_with("; the item did not start"),
        "{why}"
    );
}

#[test]
fn a_device_lost_during_an_item_makes_it_unverified_not_failed() {
    // The item fails, then the device cannot be reached: the failure says nothing about the app.
    let sample = Sample::new();
    let (code, text) = sample.kr(
        &lock_state_edited(2, lost),
        &["run", "--device", IPAD, "launch"],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("UNVERIFIED: launch"), "{text}");
    assert!(!text.contains("FAIL: launch"), "{text}");
    let end = item_end(&sample);
    assert_eq!(end["fields"]["tests"]["unverified"], 1, "{end}");
    assert_eq!(end["fields"]["tests"]["failed"], 0, "{end}");
    assert!(
        end["fields"]["unverified_reason"]
            .as_str()
            .unwrap()
            .starts_with("lost 蔡煜恒的 iPad Mini: "),
        "{end}"
    );
}

#[test]
fn register_devices_lets_xcodebuild_register_the_device() {
    let sample = Sample::with_signing("register_devices = true\n");
    let mut recs = device_recordings("runner-not-installed");
    for r in recs
        .iter_mut()
        .filter(|r| r.args.first().map(String::as_str) == Some("build-for-testing"))
    {
        let at = r
            .args
            .iter()
            .position(|a| a == "-allowProvisioningUpdates")
            .unwrap();
        r.args
            .insert(at + 1, "-allowProvisioningDeviceRegistration".to_owned());
    }
    let (code, text) = sample.kr(&recs, &["run", "--device", IPAD, "launch"]);
    assert!(!text.contains("no recording"), "{text}");
    assert_eq!(code, Some(1), "{text}");
    // Registration was on: no advice to turn it on.
    assert!(!text.contains("register_devices = true"), "{text}");
}

#[test]
fn a_cable_pulled_mid_run_makes_the_items_unverified_though_wifi_still_reaches_the_device() {
    // Real (2026-10-15): the owner pulled the cable while `slow` ran. devicectl still
    // reached the iPad over the local network, but the test runner's connection broke.
    let sample = Sample::with(
        "register_devices = true\n",
        "[scenarios.slow]\nonly = [\"KrSampleUITests/SlowUITests\"]\ntimeout = \"25m\"\n",
    );
    let (code, text) = sample.kr(
        &device_recordings("cable-pulled"),
        &["run", "--device", IPAD, "slow", "launch"],
    );
    assert_eq!(code, Some(1), "{text}");
    assert!(text.contains("UNVERIFIED: slow"), "{text}");
    assert!(text.contains("UNVERIFIED: launch"), "{text}");
    assert!(!text.contains("FAIL: "), "{text}");
    assert!(
        !text.contains("failed again when run again"),
        "no rerun on a lost device: {text}"
    );
    let reasons: Vec<String> = sample
        .events()
        .iter()
        .filter(|e| e["type"] == "item_end")
        .map(|e| {
            assert!(
                e["fields"]["tests"]["unverified"].as_u64() >= Some(1),
                "{e}"
            );
            e["fields"]["unverified_reason"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    let lost = "lost 蔡煜恒的 iPad Mini: it was connected over wired when the run started and is now on localNetwork";
    assert_eq!(
        reasons,
        [
            format!("{lost}; the item did not finish"),
            format!("{lost}; the item did not start"),
        ]
    );
}

#[test]
fn a_normal_run_builds_once_takes_the_device_and_passes() {
    // Real (2026-10-15): KrSample's launch scenario on the iPad, 2 tests.
    let sample = Sample::with_signing("register_devices = true\n");
    let (code, text) = sample.kr(
        &device_recordings("launch-passed"),
        &["run", "--device", IPAD, "launch"],
    );
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("PASS: launch (2 tests, 蔡煜恒的 iPad Mini"),
        "{text}"
    );
    assert!(
        text.contains("run on 蔡煜恒的 iPad Mini: 1/1 PASS"),
        "{text}"
    );
    let events = sample.events();
    // The device is a resource of capacity 1, held while its items run.
    let device_slot = events
        .iter()
        .find(|e| e["type"] == "slot_acquired" && e["fields"]["resource"] == "device")
        .unwrap_or_else(|| panic!("no device slot in {events:#?}"));
    assert_eq!(device_slot["fields"]["slot"], 1, "{device_slot}");
    let end = events.iter().find(|e| e["type"] == "item_end").unwrap();
    assert_eq!(end["fields"]["tests"]["passed"], 2, "{end}");
    assert!(end["fields"].get("unverified_reason").is_none(), "{end}");
    // #29: what it ran on is the real device.
    let identity = &end["fields"]["identity"];
    assert_eq!(identity["runtime"], "iOS 27.2", "{end}");
    assert_eq!(identity["device_type"], "iPad mini (A17 Pro)", "{end}");
}
