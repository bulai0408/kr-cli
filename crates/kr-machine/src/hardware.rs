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

/// Load and memory pressure now, from the kernel instead of a command: cheap enough to
/// sample every few seconds while a run lasts (ADR 0010), and not part of what replayed
/// recordings answer.
pub fn sample_load() -> Option<(Load, Pressure)> {
    let mut avg = [0f64; 3];
    // SAFETY: getloadavg writes at most 3 values into `avg`.
    if unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) } != 3 {
        return None;
    }
    let mut level: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is NUL-terminated; `level` and `size` describe one c_int.
    let read = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_vm_pressure_level".as_ptr(),
            (&raw mut level).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    let pressure = match (read, level) {
        (0, 4) => Pressure::Critical,
        (0, 2) => Pressure::Warn,
        _ => Pressure::Normal,
    };
    Some((
        Load {
            one: avg[0],
            five: avg[1],
            fifteen: avg[2],
        },
        pressure,
    ))
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

    /// Why heavy work must wait now: memory pressure is critical (the Mac is paging and
    /// tests fail for that alone), or, if `block_on_load`, the load is high.
    pub fn must_wait(&self, block_on_load: bool, per_core: f64) -> Option<String> {
        if self.pressure == Pressure::Critical {
            return Some("memory pressure is critical".to_owned());
        }
        if block_on_load {
            return self.overloaded(per_core);
        }
        None
    }

    /// Why the load counts as high now, if it does.
    pub fn overloaded(&self, per_core: f64) -> Option<String> {
        if self.load.one > self.load_threshold(per_core) {
            return Some(format!(
                "1-minute load {:.2} is above {:.0} ({per_core} × {} cores)",
                self.load.one,
                self.load_threshold(per_core),
                self.cores
            ));
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

/// Capacities from P (performance cores) and M (memory in GB). ADR 0022 calibrates
/// two builds on 10 performance cores; retain 16 GB per build and the tested cap.
/// A simulator with its test runner budgets ~2 cores and ~4 GB.
pub fn suggest(hw: &Hardware) -> Suggestion {
    let p = hw.perf_cores as u64;
    let m = hw.memory_gb();
    let build_raw = (p / 5).min(m / 16);
    let sim_raw = (p / 2).min(m.saturating_sub(8) / 4);
    let light_raw = p / 4;
    let clamp = |v: u64, max: u64| v.clamp(1, max) as u32;
    let build = clamp(build_raw, 2);
    let sim = clamp(sim_raw, 6);
    let light = clamp(light_raw, 4);
    Suggestion {
        build: Derived {
            value: build,
            formula: format!(
                "clamp(min(P/5, M/16), 1, 2) with P={p}, M={m} → min({}, {}) → {build}",
                p / 5,
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

/// Reads the machine state until work may start (see [`Hardware::must_wait`]), sleeping
/// `poll` between reads, at most `limit`. `on_busy` is called with each blocking reading.
pub fn wait_for_load(
    runner: &dyn Runner,
    block_on_load: bool,
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
        if hw.must_wait(block_on_load, per_core).is_none() {
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
