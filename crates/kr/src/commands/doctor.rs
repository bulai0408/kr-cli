use std::process::ExitCode;

use anyhow::Result;
use kr_exec::Command;
use kr_machine::{Profile, Severity, SimDevices, diagnose, find_xcode};
use kr_resource::Resource;
use serde_json::{Value, json};

use crate::Ctx;

const GB: f64 = (1u64 << 30) as f64;

pub fn run(ctx: &Ctx) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let runner = ctx.runner()?;
    let profile = kr_machine::detect(
        runner.as_ref(),
        &ctx.applications_dir(),
        ctx.env_var("DEVELOPER_DIR").filter(|d| !d.is_empty()),
    )?;
    let project = loaded.config.project.as_ref();
    let devices = match project {
        Some(_) => {
            let out =
                runner.run(&Command::new("xcrun").args(["simctl", "list", "devices", "-j"]))?;
            out.success()
                .then(|| SimDevices::parse(&out.stdout))
                .transpose()?
        }
        None => None,
    };
    let per_core = loaded.config.machine.load.per_core;
    let problems = diagnose(&profile, project, devices.as_ref(), per_core);
    let capacity: Vec<(String, Value)> = Resource::ALL
        .iter()
        .map(|&res| {
            let c = loaded.capacity_detail(res, profile.host.as_deref(), &profile.hardware);
            (res.to_string(), json!(c))
        })
        .collect();
    let required = project.and_then(|p| p.project.xcode.clone());
    let selected = selected_xcode(&profile, required.as_deref());
    let errors = problems
        .iter()
        .filter(|p| p.severity == Severity::Error)
        .count();
    let warnings = problems.len() - errors;

    if ctx.json {
        let out = json!({
            "ok": errors == 0,
            "problems": problems,
            "machine": {
                "host": profile.host,
                "chip": profile.hardware.chip,
                "cores": profile.hardware.cores,
                "performance_cores": profile.hardware.perf_cores,
                "memory_bytes": profile.hardware.memory_bytes,
                "disk_free_bytes": profile.disk_free_bytes,
                "load": profile.hardware.load,
                "load_threshold": profile.hardware.load_threshold(per_core),
                "memory_pressure": profile.hardware.pressure,
                "swap_used_bytes": profile.hardware.swap_used_bytes,
            },
            "xcode": {
                "required": required,
                "selected": selected,
                "installed": profile.xcodes,
                "developer_dir": profile.developer_dir,
            },
            "runtimes": profile.runtimes.iter().filter(|r| r.is_available).map(|r| &r.name).collect::<Vec<_>>(),
            "capacity": capacity.iter().cloned().collect::<serde_json::Map<_, _>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print_human(
            &profile,
            &problems,
            (errors, warnings),
            &capacity,
            selected.as_ref(),
            &loaded,
        );
    }
    Ok(if errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The Xcode kr will use: the required one if installed, else the selected one.
fn selected_xcode(profile: &Profile, required: Option<&str>) -> Option<Value> {
    let xcode = match required {
        Some(req) => find_xcode(&profile.xcodes, req),
        None => profile
            .developer_dir
            .as_ref()
            .and_then(|dir| profile.xcodes.iter().find(|x| &x.developer_dir() == dir)),
    }?;
    Some(json!({
        "version": xcode.version,
        "build": xcode.build,
        "path": xcode.path,
        "developer_dir": xcode.developer_dir(),
    }))
}

fn print_human(
    profile: &Profile,
    problems: &[kr_machine::Problem],
    (errors, warnings): (usize, usize),
    capacity: &[(String, Value)],
    selected: Option<&Value>,
    loaded: &crate::Loaded,
) {
    let verdict = match (errors, warnings) {
        (0, 0) => "doctor: OK".to_owned(),
        (0, w) => format!("doctor: OK with {w} warning(s)"),
        (e, w) => format!("doctor: {e} error(s), {w} warning(s)"),
    };
    println!("{verdict}");
    for p in problems {
        let tag = match p.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        println!("  {tag} {}: {}", p.code, p.message);
        println!("    fix: {}", p.fix);
    }
    println!();
    let hw = &profile.hardware;
    println!(
        "machine   {} · {} · {} cores ({} performance) · {} GB · {:.0} GB free",
        profile.host.as_deref().unwrap_or("?"),
        hw.chip,
        hw.cores,
        hw.perf_cores,
        hw.memory_gb(),
        profile.disk_free_bytes as f64 / GB
    );
    println!(
        "load      {:.2} / {:.2} / {:.2} (heavy work waits above {:.0}) · memory pressure {} · swap {:.1} GB used",
        hw.load.one,
        hw.load.five,
        hw.load.fifteen,
        hw.load_threshold(loaded.config.machine.load.per_core),
        json!(hw.pressure).as_str().unwrap_or("?"),
        hw.swap_used_bytes as f64 / GB
    );
    let xcodes: Vec<String> = profile
        .xcodes
        .iter()
        .map(|x| format!("{} ({}) {}", x.version, x.build, x.path.display()))
        .collect();
    println!(
        "xcode     {}",
        if xcodes.is_empty() {
            "none found".to_owned()
        } else {
            xcodes.join(" · ")
        }
    );
    if let Some(sel) = selected {
        println!(
            "          kr uses DEVELOPER_DIR={}",
            sel["developer_dir"].as_str().unwrap_or("?")
        );
    }
    let runtimes: Vec<&str> = profile
        .runtimes
        .iter()
        .filter(|r| r.is_available)
        .map(|r| r.name.as_str())
        .collect();
    println!("runtimes  {}", runtimes.join(", "));
    for (i, (name, c)) in capacity.iter().enumerate() {
        let label = if i == 0 { "capacity" } else { "" };
        let how = match c["formula"].as_str() {
            Some(f) => format!("suggested: {f}"),
            None => c["source"].as_str().unwrap_or("").to_owned(),
        };
        println!("{label:<9} {name:<5} {}  {how}", c["value"]);
    }
    if let Some(project) = &loaded.config.project {
        let kinds: Vec<String> = project
            .simulators
            .devices
            .iter()
            .map(|(kind, k)| {
                let runtime = k
                    .runtime
                    .as_deref()
                    .or(project.simulators.runtime.as_deref())
                    .unwrap_or("?");
                format!("{kind}: {} × {} on {runtime}", k.device_type, k.count)
            })
            .collect();
        println!("pool      {}: {}", project.project.name, kinds.join(" · "));
    }
}
