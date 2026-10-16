use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use jiff::Timestamp;
use kr_config::{DEFAULT_DEVICE, ProjectConfig};
use kr_events::{Event, EventKind, RunId};
use kr_resource::Resource;
use kr_sim::{DeviceLocks, LeaseHolder, Simctl};
use serde_json::json;

use super::lock::{parse_held, run_command};
use crate::{Ctx, Loaded, slots};

#[derive(Debug, Args)]
pub struct PoolArgs {
    #[command(subcommand)]
    command: Option<PoolCommand>,
}

#[derive(Debug, Subcommand)]
enum PoolCommand {
    /// Show the pool devices, their state and who leases them (the default).
    Status,
    /// Create the pool devices that do not exist yet. Never deletes or changes others.
    Sync,
    /// Take a `sim` slot, lease and boot a pool device, run a command, then shut it down.
    /// The command gets KR_SIM_UDID and KR_SIM_NAME.
    /// With --json, lease notices go to stderr; command stdout and exit code pass through.
    Lease(LeaseArgs),
}

#[derive(Debug, Args)]
struct LeaseArgs {
    /// Device kind from [simulators.devices]. Default: `default`. Conflicts with --name/--udid.
    #[arg(long, conflicts_with_all = ["name", "udid"])]
    kind: Option<String>,

    /// Lease only this pool device; wait for it when busy. Conflicts with --kind/--udid.
    #[arg(long, conflicts_with = "udid")]
    name: Option<String>,

    /// Lease only this pool UDID; wait for it when busy. Conflicts with --kind/--name.
    #[arg(long)]
    udid: Option<String>,

    /// Task name shown in `kr queue` and `kr pool`. Default: the command line.
    #[arg(long)]
    task: Option<String>,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

const LEASE_POLL: Duration = Duration::from_secs(2);

pub fn run(ctx: &Ctx, args: PoolArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let project =
        loaded.config.project.as_ref().context(
            "no .kr.toml found from here up to the repo root; the pool is defined there",
        )?;
    let runner = ctx.runner()?;
    let simctl = Simctl::new(runner.as_ref(), loaded.developer_dir(ctx)?);
    match args.command.unwrap_or(PoolCommand::Status) {
        PoolCommand::Status => status(ctx, &loaded, project, &simctl),
        PoolCommand::Sync => sync(ctx, project, &simctl),
        PoolCommand::Lease(lease) => {
            lease_and_run(ctx, &loaded, project, &simctl, runner.as_ref(), lease)
        }
    }
}

fn status(
    ctx: &Ctx,
    loaded: &Loaded,
    project: &ProjectConfig,
    simctl: &Simctl<'_>,
) -> Result<ExitCode> {
    let devices = kr_sim::plan(project, &simctl.runtimes()?, &simctl.devices()?)?;
    let locks = DeviceLocks::new(kr_sim::locks_dir(&loaded.state_dir));
    let rows: Vec<serde_json::Value> = devices
        .iter()
        .map(|d| {
            let holder = d.udid.as_deref().and_then(|u| locks.holder(u));
            json!({
                "name": d.name,
                "kind": d.kind,
                "device_type": d.device_type,
                "runtime": d.runtime,
                "udid": d.udid,
                "state": d.state.as_deref().unwrap_or("missing"),
                "leased_by": holder,
                "issue": d.issue,
            })
        })
        .collect();
    if ctx.json {
        crate::output::print_json(&json!({ "devices": rows }))?;
        return Ok(ExitCode::SUCCESS);
    }
    let missing = devices.iter().filter(|d| d.udid.is_none()).count();
    if missing == 0 {
        println!("pool {}: {} devices", project.project.name, devices.len());
    } else {
        println!(
            "pool {}: {missing} of {} devices missing; run `kr pool sync`",
            project.project.name,
            devices.len()
        );
    }
    for row in &rows {
        let leased = match row["leased_by"].as_object() {
            Some(h) => format!(
                " · leased by pid {} ({})",
                h["pid"],
                h["task"].as_str().unwrap_or("")
            ),
            None => String::new(),
        };
        println!(
            "  {:<24} {:<8} {} · {}{leased}",
            row["name"].as_str().unwrap_or(""),
            row["state"].as_str().unwrap_or(""),
            row["device_type"].as_str().unwrap_or(""),
            row["runtime"].as_str().unwrap_or("")
        );
        if let Some(issue) = row["issue"].as_str() {
            println!("    problem: {issue}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn sync(ctx: &Ctx, project: &ProjectConfig, simctl: &Simctl<'_>) -> Result<ExitCode> {
    let report = kr_sim::sync(simctl, project)?;
    if ctx.json {
        crate::output::print_json(&report)?;
    } else {
        if report.problems.is_empty() {
            println!(
                "pool {}: {} created, {} already there",
                project.project.name,
                report.created.len(),
                report.existing.len()
            );
        } else {
            println!(
                "pool {}: {} problem(s), {} created, {} already there",
                project.project.name,
                report.problems.len(),
                report.created.len(),
                report.existing.len()
            );
        }
        for name in &report.created {
            println!("  created {name}");
        }
        for p in &report.problems {
            println!("  problem: {p}");
        }
    }
    Ok(if report.problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn lease_and_run(
    ctx: &Ctx,
    loaded: &Loaded,
    project: &ProjectConfig,
    simctl: &Simctl<'_>,
    runner: &dyn kr_exec::Runner,
    args: LeaseArgs,
) -> Result<ExitCode> {
    let kind = args.kind.as_deref().unwrap_or(DEFAULT_DEVICE);
    if args.name.is_none() && args.udid.is_none() && !project.simulators.devices.contains_key(kind)
    {
        let kinds: Vec<&str> = project
            .simulators
            .devices
            .keys()
            .map(String::as_str)
            .collect();
        bail!(
            "no simulator kind `{}` in .kr.toml; known kinds: {}",
            kind,
            kinds.join(", ")
        );
    }
    let mut task = args.task.clone().unwrap_or_else(|| args.command.join(" "));
    let run_id = RunId::new();
    let mut devices = kr_sim::plan(project, &simctl.runtimes()?, &simctl.devices()?)?;
    let (by, value) = if let Some(name) = args.name.as_deref() {
        ("name", name)
    } else if let Some(udid) = args.udid.as_deref() {
        ("udid", udid)
    } else {
        ("kind", kind)
    };
    let selection = json!({ "by": by, "value": value });
    if by != "kind" {
        devices.retain(|d| {
            if by == "name" {
                d.name == value
            } else {
                d.udid.as_deref() == Some(value)
            }
        });
        if devices.is_empty() {
            return Err(crate::output::BadArguments(format!(
                "no pool device with {by} `{value}` in this project; run `kr pool status` to see pool devices"
            )).into());
        }
        let device = &devices[0];
        task.push_str(&format!(
            " [{by}={value}; device={} udid={}]",
            device.name,
            device.udid.as_deref().unwrap_or("missing")
        ));
    }
    let kind = devices
        .first()
        .filter(|_| by != "kind")
        .map_or(kind, |d| d.kind.as_str());
    let holder = LeaseHolder {
        project: Some(project.project.name.clone()),
        task: task.clone(),
        pid: std::process::id(),
        started: Timestamp::now(),
    };
    let locks = DeviceLocks::new(kr_sim::locks_dir(&loaded.state_dir));
    let mut announced = false;
    let mut notice = |device: &kr_sim::PoolDevice, current: Option<LeaseHolder>| {
        if by == "kind" || announced {
            return;
        }
        announced = true;
        let udid = device.udid.as_deref().unwrap_or("missing");
        if ctx.json {
            eprintln!(
                "{}",
                json!({
                    "waiting": "pool_device", "device": device.name, "udid": udid,
                    "holder": current, "selection": selection,
                })
            );
        } else {
            let task = current.as_ref().map_or("unknown", |h| h.task.as_str());
            eprintln!(
                "waiting for pool device {} ({udid}): held by {task}; only this device was requested (see `kr queue sim`)",
                device.name
            );
        }
    };
    if by != "kind" {
        let device = &devices[0];
        if let Some(current) = device.udid.as_deref().and_then(|u| locks.holder(u)) {
            notice(device, Some(current));
        }
    }
    let taken = slots::take(ctx, loaded, runner, run_id, Resource::Sim, &task)?;
    let booting = Instant::now();
    let lease = match kr_sim::lease_with_wait_notice(
        simctl, &locks, &devices, kind, &holder, LEASE_POLL, None, notice,
    ) {
        Ok(lease) => lease,
        Err(e) => {
            slots::give_back(loaded, run_id, taken, None)?;
            return Err(e);
        }
    };
    let log = loaded.events();
    if ctx.json {
        eprintln!(
            "{}",
            json!({
                "schema_version": crate::output::SCHEMA_VERSION,
                "type": "sim_leased", "device": lease.name(), "udid": lease.udid(),
                "selection": selection,
            })
        );
    }
    log.append(&Event::now(
        run_id,
        loaded.project(),
        EventKind::SimLeased,
        json!({ "device": lease.name(), "udid": lease.udid(), "task": task, "selection": selection, "boot_ms": booting.elapsed().as_millis() as u64 }),
    ))?;
    let held_since = Instant::now();
    let mut held = parse_held(ctx.env_var("KR_HELD").unwrap_or(""));
    held.retain(|(r, _)| *r != Resource::Sim);
    held.push((Resource::Sim, taken.slot.index()));
    let (name, udid) = (lease.name().to_owned(), lease.udid().to_owned());
    let result = run_command(
        &args.command,
        None,
        &held,
        &[
            ("KR_SIM_UDID", udid.as_str()),
            ("KR_SIM_NAME", name.as_str()),
        ],
    );
    let released = lease.release();
    log.append(&Event::now(
        run_id,
        loaded.project(),
        EventKind::SimReleased,
        json!({ "device": name, "udid": udid, "selection": selection, "held_ms": held_since.elapsed().as_millis() as u64 }),
    ))?;
    slots::give_back(loaded, run_id, taken, result.as_ref().ok().copied())?;
    released?;
    result.map(|code| ExitCode::from(u8::try_from(code).unwrap_or(1)))
}
