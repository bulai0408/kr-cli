use std::path::{Path, PathBuf};
use std::time::Duration;

use kr_exec::{Recording, ReplayRunner};
use kr_machine::{Hardware, Pressure, detect, find_xcode, read_hardware, suggest, wait_for_load};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(rel)
}

fn rec(rel: &str) -> Recording {
    kr_exec::read_recording(&fixture(rel)).unwrap()
}

/// A sysctl answer in the recorded format, for machines we do not have.
fn sysctl(
    ncpu: u32,
    perf: Option<u32>,
    mem_gb: u64,
    load: f64,
    pressure: u32,
    chip: &str,
) -> Recording {
    let mut r = rec("sysctl/m3-imac.json");
    let perf = perf.map_or_else(String::new, |p| format!("hw.perflevel0.physicalcpu: {p}\n"));
    r.stdout = format!(
        "hw.ncpu: {ncpu}\n{perf}hw.memsize: {}\nvm.loadavg: {{ {load:.2} {load:.2} {load:.2} }}\nkern.memorystatus_vm_pressure_level: {pressure}\nvm.swapusage: total = 0.00M  used = 0.00M  free = 0.00M  (encrypted)\nmachdep.cpu.brand_string: {chip}\n",
        mem_gb << 30
    );
    r
}

fn m3() -> Hardware {
    read_hardware(&ReplayRunner::new(vec![rec("sysctl/m3-imac.json")])).unwrap()
}

#[test]
fn reads_the_m3_imac_hardware() {
    let hw = m3();
    assert_eq!(hw.chip, "Apple M3");
    assert_eq!((hw.cores, hw.perf_cores), (8, 4));
    assert_eq!(hw.memory_bytes, 24 << 30);
    assert_eq!(hw.pressure, Pressure::Normal);
    assert!(hw.load.one > 0.0);
    assert!(hw.swap_used_bytes > 0);
}

#[test]
fn low_end_m3_gets_one_build_two_sims_one_light() {
    let s = suggest(&m3());
    assert_eq!((s.build.value, s.sim.value, s.light.value), (1, 2, 1));
    assert_eq!(
        s.build.formula,
        "clamp(min(P/6, M/16), 1, 4) with P=4, M=24 → min(0, 1) → 1"
    );
    assert_eq!(
        s.sim.formula,
        "clamp(min(P/2, (M-8)/4), 1, 6) with P=4, M=24 → min(2, 4) → 2"
    );
    assert_eq!(s.light.formula, "clamp(P/4, 1, 4) with P=4 → 1 → 1");
}

#[test]
fn high_end_m4_max_gets_more_slots() {
    let runner = ReplayRunner::new(vec![sysctl(16, Some(12), 64, 3.0, 1, "Apple M4 Max")]);
    let s = suggest(&read_hardware(&runner).unwrap());
    assert_eq!((s.build.value, s.sim.value, s.light.value), (2, 6, 3));
}

#[test]
fn a_mac_without_performance_levels_counts_all_cores() {
    let runner = ReplayRunner::new(vec![sysctl(4, None, 8, 1.0, 1, "Intel(R) Core(TM) i5")]);
    let hw = read_hardware(&runner).unwrap();
    assert_eq!(hw.perf_cores, 4);
    let s = suggest(&hw);
    assert_eq!((s.build.value, s.sim.value, s.light.value), (1, 1, 1));
}

#[test]
fn load_over_one_and_a_quarter_per_core_is_too_high() {
    let busy = read_hardware(&ReplayRunner::new(vec![sysctl(
        8,
        Some(4),
        24,
        10.5,
        1,
        "Apple M3",
    )]))
    .unwrap();
    assert_eq!(busy.load_threshold(), 10.0);
    assert!(busy.overloaded().unwrap().contains("10.50"));
    let pressured = read_hardware(&ReplayRunner::new(vec![sysctl(
        8,
        Some(4),
        24,
        1.0,
        4,
        "Apple M3",
    )]))
    .unwrap();
    assert!(pressured.overloaded().unwrap().contains("critical"));
    assert_eq!(m3().overloaded(), None);
}

#[test]
fn waiting_for_load_rechecks_until_it_drops() {
    let runner = ReplayRunner::new(vec![
        sysctl(8, Some(4), 24, 30.0, 1, "Apple M3"),
        sysctl(8, Some(4), 24, 12.0, 1, "Apple M3"),
        sysctl(8, Some(4), 24, 4.0, 1, "Apple M3"),
    ]);
    let mut seen = Vec::new();
    let wait = wait_for_load(&runner, Duration::ZERO, Duration::from_secs(60), |hw| {
        seen.push(hw.load.one)
    })
    .unwrap();
    assert_eq!(seen, [30.0, 12.0]);
    assert_eq!(wait.checks, 3);
    assert!(!wait.timed_out);
}

#[test]
fn waiting_for_load_gives_up_after_the_limit() {
    let runner = ReplayRunner::new(vec![
        sysctl(8, Some(4), 24, 30.0, 1, "Apple M3"),
        sysctl(8, Some(4), 24, 30.0, 1, "Apple M3"),
    ]);
    let wait = wait_for_load(
        &runner,
        Duration::from_millis(5),
        Duration::from_millis(1),
        |_| {},
    )
    .unwrap();
    assert!(wait.timed_out);
}

fn full_runner() -> ReplayRunner {
    ReplayRunner::new(vec![
        rec("sysctl/m3-imac.json"),
        rec("scutil/local-host-name.json"),
        rec("df/root.json"),
        rec("xcode-select/print-path.json"),
        rec("simctl/list-runtimes.json"),
    ])
}

#[test]
fn detect_reads_xcodes_runtimes_and_disk() {
    let profile = detect(&full_runner(), &fixture("xcode/apps"), None).unwrap();
    assert_eq!(profile.host.as_deref(), Some("iMac"));
    assert!(profile.disk_free_bytes > 1 << 30);
    let xcodes: Vec<_> = profile
        .xcodes
        .iter()
        .map(|x| (x.version.as_str(), x.build.as_str()))
        .collect();
    assert_eq!(xcodes, [("27.1", "27A9269"), ("26.6", "17F113")]);
    assert_eq!(
        profile.developer_dir.as_deref(),
        Some(Path::new("/Applications/Xcode.app/Contents/Developer"))
    );
    let runtimes: Vec<_> = profile.runtimes.iter().map(|r| r.name.as_str()).collect();
    assert!(
        runtimes.contains(&"iOS 18.6") && runtimes.contains(&"tvOS 27.0"),
        "{runtimes:?}"
    );
}

#[test]
fn developer_dir_from_the_environment_wins() {
    let runner = ReplayRunner::new(vec![
        rec("sysctl/m3-imac.json"),
        rec("scutil/local-host-name.json"),
        rec("df/root.json"),
        rec("simctl/list-runtimes.json"),
    ]);
    let profile = detect(
        &runner,
        &fixture("xcode/apps"),
        Some("/Applications/Xcode_26.6.app/Contents/Developer"),
    )
    .unwrap();
    assert_eq!(
        profile.developer_dir.as_deref(),
        Some(Path::new("/Applications/Xcode_26.6.app/Contents/Developer"))
    );
}

#[test]
fn required_xcode_matches_by_version_components() {
    let profile = detect(&full_runner(), &fixture("xcode/apps"), None).unwrap();
    assert_eq!(find_xcode(&profile.xcodes, "27").unwrap().version, "27.1");
    assert_eq!(find_xcode(&profile.xcodes, "26.6").unwrap().version, "26.6");
    assert!(find_xcode(&profile.xcodes, "27.10").is_none());
    assert!(find_xcode(&profile.xcodes, "2").is_none());
}
