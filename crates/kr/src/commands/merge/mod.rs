//! Conservative merge assistance (ADR 0017).

use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use kr_exec::{Command, Output, Runner};
use serde_json::Value;

use crate::{Ctx, output};

mod catalog;
mod install;
mod json;
mod predict;
mod quick;
pub(crate) use predict::{decide, pending_path};

#[derive(Debug, Args)]
pub struct MergeArgs {
    #[command(subcommand)]
    command: MergeCommand,
}

#[derive(Debug, Subcommand)]
enum MergeCommand {
    /// Check changed JSON, scenarios, migrations, credentials, and incremental builds.
    Check(quick::CheckArgs),
    /// Opt in for all existing worktrees: enable rerere, register the catalog driver,
    /// and edit tracked .gitattributes (review and commit it).
    Install,
    /// Compare branch pairs without merging; hand logical overlaps to the agent.
    Predict {
        #[arg(long, default_value = "HEAD")]
        base: String,
        #[arg(required = true, num_args = 2..)]
        branches: Vec<String>,
    },
    /// Three-way merge whole catalog entries; validate before replacing OURS.
    Xcstrings {
        base: std::path::PathBuf,
        ours: std::path::PathBuf,
        theirs: std::path::PathBuf,
    },
}

pub fn run(ctx: &Ctx, args: MergeArgs) -> Result<ExitCode> {
    match args.command {
        MergeCommand::Check(args) => quick::run(ctx, args),
        MergeCommand::Install => install::run(ctx),
        MergeCommand::Predict { base, branches } => predict::run(ctx, &base, &branches),
        MergeCommand::Xcstrings { base, ours, theirs } => catalog::run(ctx, &base, &ours, &theirs),
    }
}

fn git(ctx: &Ctx, runner: &dyn Runner, args: &[&str]) -> Result<Output> {
    runner.run(
        &Command::new("git")
            .args(args.iter().copied())
            .current_dir(&ctx.cwd),
    )
}

fn checked_git(ctx: &Ctx, runner: &dyn Runner, args: &[&str]) -> Result<String> {
    let out = git(ctx, runner, args)?;
    if !out.success() {
        bail!(
            "git {} failed; check the repository and reference names",
            args.first().unwrap_or(&"")
        );
    }
    Ok(out.stdout)
}

fn revision(ctx: &Ctx, runner: &dyn Runner, name: &str) -> Result<String> {
    Ok(checked_git(
        ctx,
        runner,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{name}^{{commit}}"),
        ],
    )?
    .trim()
    .to_owned())
}

fn report(ctx: &Ctx, value: Value, text: &str, ok: bool) -> Result<ExitCode> {
    if ctx.json {
        output::print_json(&value)?;
    } else {
        println!("{text}");
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
