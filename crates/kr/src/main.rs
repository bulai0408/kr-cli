use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod commands;
mod compilation_cache;
mod ctx;
mod invocations;
mod output;
mod selection;
mod selection_history;
mod slots;

/// Kami's Apple development workflow tool: resource queues, simulator pool,
/// pre-checks, build-once test runs and gates.
#[derive(Debug, Parser)]
#[command(name = "kr", version, propagate_version = true)]
struct Cli {
    /// Print machine-readable JSON on stdout.
    #[arg(long, global = true)]
    json: bool,

    /// More log output on stderr (-v info, -vv debug, -vvv trace). `KR_LOG` overrides.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Predict merge conflicts, install rerere and the catalog driver, or check a merge.
    Merge(commands::merge::MergeArgs),
    /// Show the merged configuration and where each layer came from.
    Config,
    /// Wait for a slot of a resource, run a command, then give the slot back.
    Lock(commands::lock::LockArgs),
    /// Show who holds and who waits for each resource.
    Queue(commands::queue::QueueArgs),
    /// Check this Mac against the project: Xcode, runtimes, pool, disk, load, capacities,
    /// and with --device what a test run on that device needs.
    Doctor(commands::doctor::DoctorArgs),
    /// Pre-check before the simulator queue: generate, compile with the app's settings,
    /// confirm test filters, check data isolation. Holds only a `build` slot.
    Check(commands::check::CheckArgs),
    /// Build once, then run scenarios and suites on pool simulators and verify the
    /// results (test counts, real exit codes, one rerun of each failure).
    Run(commands::run::RunArgs),
    /// Run a gate from .kr.toml: its suites and scenarios, like `kr run`.
    Gate(commands::run::GateArgs),
    /// Hand back a decision of a run that stopped at decision points (exit 4): kr
    /// records it and, once all are decided, gives the run's verdict from its results.
    Decide(commands::decide::DecideArgs),
    /// Show what `kr run` / `kr gate` would do without doing it: cached builds, why each
    /// item runs, the lanes, and the expected time from history. With --exit-code, exit
    /// 3 when there is work to do.
    Plan(commands::plan::PlanArgs),
    /// Show build input hashes and their files, content digests, command and toolchain.
    Hash(commands::hash::HashArgs),
    /// Replay recorded runs with kr's scheduler in virtual time and compare settings
    /// (sim capacity, lane order, cache): p50/p90, lower bound, queue share, and the
    /// calibration error against the measured times.
    Simulate(commands::simulate::SimulateArgs),
    /// Judge saved results again with this kr's rules and list only the verdicts that
    /// change: the items of saved `kr run` / `kr gate` runs and, with --evidence, older
    /// result bundles.
    Rejudge(commands::rejudge::RejudgeArgs),
    /// The project's simulator pool: status, sync, lease.
    Pool(commands::pool::PoolArgs),
    /// Draft the delivery report of a run: decided by machine, needs human judgment of
    /// feel, not verified. Lists the to-dos the run and the changed files show.
    Deliver(commands::deliver::DeliverArgs),
    /// How to use this kr, for the agent that calls it: the usual flow, decision points,
    /// when to ask the owner, the hard rules. With project and personal additions.
    Skill(commands::skill::SkillArgs),
    /// Install or remove the kr discovery entry and rules for Claude Code and Codex.
    Setup(commands::setup::SetupArgs),
    /// Report on a batch of runs: queue and run time, rework rounds, flaky tests, which
    /// commands the agent called and where it went around, and how often its decisions
    /// held. Data only; it changes nothing.
    Report(commands::report::ReportArgs),
}

pub use ctx::{Ctx, Loaded};

fn main() -> ExitCode {
    let started = std::time::Instant::now();
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mut ctx = Ctx {
        cwd: std::env::current_dir().unwrap_or_default(),
        env: std::env::vars().collect(),
        json: false,
    };
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            // `--help` and `--version` print on stdout; they are not calls worth logging.
            if err.use_stderr() {
                invocations::record(
                    &ctx,
                    &args,
                    output::BAD_ARGUMENTS,
                    started.elapsed(),
                    Some(err.to_string()),
                );
            }
            err.exit();
        }
    };
    init_tracing(cli.verbose);
    slots::watch_children();
    ctx.json = cli.json;
    let result = match cli.command {
        Command::Merge(args) => commands::merge::run(&ctx, args),
        Command::Config => commands::config::run(&ctx),
        Command::Lock(args) => commands::lock::run(&ctx, args),
        Command::Queue(args) => commands::queue::run(&ctx, args),
        Command::Doctor(args) => commands::doctor::run(&ctx, args),
        Command::Plan(args) => commands::plan::run(&ctx, args),
        Command::Hash(args) => commands::hash::run(&ctx, args),
        Command::Simulate(args) => commands::simulate::run(&ctx, args),
        Command::Rejudge(args) => commands::rejudge::run(&ctx, args),
        Command::Pool(args) => commands::pool::run(&ctx, args),
        Command::Check(args) => commands::check::run(&ctx, args),
        Command::Run(args) => commands::run::run(&ctx, args),
        Command::Gate(args) => commands::run::gate(&ctx, args),
        Command::Decide(args) => commands::decide::run(&ctx, args),
        Command::Deliver(args) => commands::deliver::run(&ctx, args),
        Command::Skill(args) => commands::skill::run(&ctx, args),
        Command::Setup(args) => commands::setup::run(&ctx, args),
        Command::Report(args) => commands::report::run(&ctx, args),
    };
    let (code, error) = match result {
        Ok(code) => (code, None),
        Err(err) => {
            output::report_error(&err, ctx.json);
            let code = if err.downcast_ref::<output::BadArguments>().is_some() {
                ExitCode::from(output::BAD_ARGUMENTS)
            } else {
                ExitCode::FAILURE
            };
            (code, Some(format!("{err:#}")))
        }
    };
    // ExitCode has no getter: find the number it was made from.
    let number = (0..=u8::MAX)
        .find(|n| ExitCode::from(*n) == code)
        .unwrap_or(1);
    invocations::record(&ctx, &args, number, started.elapsed(), error);
    code
}

fn init_tracing(verbose: u8) {
    use tracing_subscriber::EnvFilter;
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_env("KR_LOG").unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .without_time()
        .init();
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        super::Cli::command().debug_assert();
    }
}
