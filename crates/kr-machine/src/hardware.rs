//! CPU, memory and load from `sysctl`, and resource capacities derived from them.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use kr_exec::{Command, Runner};
use serde::Serialize;

const SYSCTL_KEYS: &[&str] = &[
    "hw.ncpu",
    "hw.perflevel0.physicalcpu",
    "hw.memsize",
    "vm.loadavg",
    "kern.memorystatus_vm_pressure_level",
    "vm.swapusage",
    "machdep.cpu.brand_string",
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hardware {
    pub chip: String,
    /// Logical cores.
    pub cores: u32,
    /// Performance cores; all cores on Macs without performance levels.
    pub perf_cores: u32,
    pub memory_bytes: u64,
    pub load: Load,
    pub pressure: Pressure,
    pub swap_used_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Load {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

/// `kern.memorystatus_vm_pressure_level`: 1 normal, 2 warn, 4 critical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pressure {
    Normal,
    Warn,
    Critical,
}

/// One `sysctl` call. Keys a Mac does not have (e.g. performance levels on Intel) are
/// reported on stderr and skipped.
pub fn read_hardware(runner: &dyn Runner) -> Result<Hardware> {
    let out = runner.run(&Command::new("sysctl").args(SYSCTL_KEYS.iter().copied()))?;
    let value = |key: &str| {
        out.stdout.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_owned())
        })
    };
    let number = |key: &str| -> Result<u64> {
        value(key)
            .with_context(|| format!("sysctl did not report {key}"))?
            .parse()
            .with_context(|| format!("sysctl {key} is not a number"))
    };
    let cores = number("hw.ncpu")? as u32;
    let perf_cores = number("hw.perflevel0.physicalcpu").map_or(cores, |n| n as u32);
    let load: Vec<f64> = value("vm.loadavg")
        .context("sysctl did not report vm.loadavg")?
        .trim_matches(|c| c == '{' || c == '}' || c == ' ')
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    let [one, five, fifteen] = load[..] else {
        anyhow::bail!("cannot read vm.loadavg");
    };
    let pressure = match number("kern.memorystatus_vm_pressure_level").unwrap_or(1) {
        4 => Pressure::Critical,
        2 => Pressure::Warn,
        _ => Pressure::Normal,
    };
    Ok(Hardware {
        chip: value("machdep.cpu.brand_string").unwrap_or_default(),
        cores,
        perf_cores,
        memory_bytes: number("hw.memsize")?,
        load: Load { one, five, fifteen },
        pressure,
        swap_used_bytes: value("vm.swapusage")
            .as_deref()
            .and_then(swap_used)
            .unwrap_or(0),
    })
}

/// "total = 3072.00M  used = 1590.00M  free = 1482.00M  (encrypted)" → bytes used.
fn swap_used(text: &str) -> Option<u64> {
    let used = text.split("used = ").nth(1)?.split_whitespace().next()?;
    let (number, unit) = used.split_at(used.len() - 1);
    let n: f64 = number.parse().ok()?;
    let scale = match unit {
        "K" => 1u64 << 10,
        "M" => 1 << 20,
        "G" => 1 << 30,
        _ => return None,
    };
    Some((n * scale as f64) as u64)
}

impl Hardware {
    pub fn memory_gb(&self) -> u64 {
        self.memory_bytes >> 30
    }

    /// Above this 1-minute load, heavy work waits: `per_core` × logical cores.
    pub fn load_threshold(&self, per_core: f64) -> f64 {
        self.cores as f64 * per_core
    }

    /// Why heavy work should wait now, if it should.
    pub fn overloaded(&self, per_core: f64) -> Option<String> {
        if self.load.one > self.load_threshold(per_core) {
            return Some(format!(
                "1-minute load {:.2} is above {:.0} ({per_core} × {} cores)",
                self.load.one,
                self.load_threshold(per_core),
                self.cores
            ));
        }
        if self.pressure == Pressure::Critical {
            return Some("memory pressure is critical".to_owned());
        }
        None
    }
}

/// A suggested capacity and how it was computed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Derived {
    pub value: u32,
    pub formula: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub build: Derived,
    pub sim: Derived,
    pub light: Derived,
}

/// Capacities from P (performance cores) and M (memory in GB). A heavy build uses
/// every core and ~16 GB; a simulator with its test runner ~2 cores and ~4 GB.
pub fn suggest(hw: &Hardware) -> Suggestion {
    let p = hw.perf_cores as u64;
    let m = hw.memory_gb();
    let build_raw = (p / 6).min(m / 16);
    let sim_raw = (p / 2).min(m.saturating_sub(8) / 4);
    let light_raw = p / 4;
    let clamp = |v: u64, max: u64| v.clamp(1, max) as u32;
    let build = clamp(build_raw, 4);
    let sim = clamp(sim_raw, 6);
    let light = clamp(light_raw, 4);
    Suggestion {
        build: Derived {
            value: build,
            formula: format!(
                "clamp(min(P/6, M/16), 1, 4) with P={p}, M={m} → min({}, {}) → {build}",
                p / 6,
                m / 16
            ),
        },
        sim: Derived {
            value: sim,
            formula: format!(
                "clamp(min(P/2, (M-8)/4), 1, 6) with P={p}, M={m} → min({}, {}) → {sim}",
                p / 2,
                m.saturating_sub(8) / 4
            ),
        },
        light: Derived {
            value: light,
            formula: format!("clamp(P/4, 1, 4) with P={p} → {light_raw} → {light}"),
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadWait {
    /// How many times the load was read.
    pub checks: u32,
    pub waited: Duration,
    /// Gave up after the limit; the work goes ahead anyway.
    pub timed_out: bool,
}

/// Reads the load until it is acceptable, sleeping `poll` between reads, at most `limit`.
/// `on_busy` is called with each reading that is too high.
pub fn wait_for_load(
    runner: &dyn Runner,
    per_core: f64,
    poll: Duration,
    limit: Duration,
    mut on_busy: impl FnMut(&Hardware),
) -> Result<LoadWait> {
    let start = Instant::now();
    let mut checks = 0;
    loop {
        let hw = read_hardware(runner)?;
        checks += 1;
        if hw.overloaded(per_core).is_none() {
            return Ok(LoadWait {
                checks,
                waited: start.elapsed(),
                timed_out: false,
            });
        }
        on_busy(&hw);
        if start.elapsed() >= limit {
            return Ok(LoadWait {
                checks,
                waited: start.elapsed(),
                timed_out: true,
            });
        }
        std::thread::sleep(poll);
    }
}
