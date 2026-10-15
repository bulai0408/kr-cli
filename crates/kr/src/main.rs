use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod commands;
mod ctx;
mod output;
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
    /// Show the merged configuration and where each layer came from.
    Config,
    /// Wait for a slot of a resource, run a command, then give the slot back.
    Lock(commands::lock::LockArgs),
    /// Show who holds and who waits for each resource.
    Queue(commands::queue::QueueArgs),
    /// Check this Mac against the project: Xcode, runtimes, pool, disk, load, capacities.
    Doctor,
    /// Pre-check before the simulator queue: generate, compile with the app's settings,
    /// confirm test filters, check data isolation. Holds only a `build` slot.
    Check(commands::check::CheckArgs),
    /// Build once, then run scenarios and suites on pool simulators and verify the
    /// results (test counts, real exit codes, one rerun of each failure).
    Run(commands::run::RunArgs),
    /// Run a gate from .kr.toml: its suites and scenarios, like `kr run`.
    Gate(commands::run::GateArgs),
    /// The project's simulator pool: status, sync, lease.
    Pool(commands::pool::PoolArgs),
    /// Draft the delivery report of a run: decided by machine, needs human judgment of
    /// feel, not verified. Lists the to-dos the run and the changed files show.
    Deliver(commands::deliver::DeliverArgs),
}

pub use ctx::{Ctx, Loaded};

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    slots::watch_children();
    let ctx = Ctx {
        cwd: std::env::current_dir().unwrap_or_default(),
        env: std::env::vars().collect(),
        json: cli.json,
    };
    let result = match cli.command {
        Command::Config => commands::config::run(&ctx),
        Command::Lock(args) => commands::lock::run(&ctx, args),
        Command::Queue(args) => commands::queue::run(&ctx, args),
        Command::Doctor => commands::doctor::run(&ctx),
        Command::Pool(args) => commands::pool::run(&ctx, args),
        Command::Check(args) => commands::check::run(&ctx, args),
        Command::Run(args) => commands::run::run(&ctx, args),
        Command::Gate(args) => commands::run::gate(&ctx, args),
        Command::Deliver(args) => commands::deliver::run(&ctx, args),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            output::report_error(&err, ctx.json);
            ExitCode::FAILURE
        }
    }
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
