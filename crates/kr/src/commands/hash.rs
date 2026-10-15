//! Inspect the inputs used by the existing build cache (ADR 0006, ADR 0019).
use super::check;
use crate::Ctx;
use anyhow::{Result, bail};
use clap::Args;
use serde_json::json;
use std::process::ExitCode;

#[derive(Debug, Args)]
pub struct HashArgs {
    /// Scenarios or suites whose build inputs to inspect. Default: the default gate.
    targets: Vec<String>,
}
pub fn run(ctx: &Ctx, args: HashArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let (project, root) = loaded.require_project()?;
    let selection = check::select(project, &args.targets, None)?;
    let runner = ctx.runner()?;
    let developer_dir = loaded.developer_dir(ctx)?;
    let jobs = check::build_jobs(project, &selection)?;
    let inputs = check::job_inputs(
        project,
        &root,
        &jobs,
        runner.as_ref(),
        developer_dir.as_deref(),
    );
    if inputs.iter().any(Option::is_none) {
        bail!(
            "cannot list hash inputs in {}; run `git ls-files -co --exclude-standard` there and fix the error",
            root.display()
        );
    }
    let jobs: Vec<_> = inputs.into_iter().flatten().collect();
    if ctx.json {
        crate::output::print_json(&json!({"jobs":jobs}))?;
    } else {
        println!(
            "hash: {} build job(s); file contents + command + Xcode path + generate command",
            jobs.len()
        );
        for job in &jobs {
            println!("  {}: {}", job.job, job.hash);
            println!("    command: {}", job.inputs.command.join(" "));
            println!("    developer_dir: {}", job.inputs.developer_dir);
            println!("    generate: {}", job.inputs.generate);
            for file in &job.inputs.files {
                println!(
                    "    {}: {}",
                    file.path,
                    file.content_hash.as_deref().unwrap_or("<missing>")
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
