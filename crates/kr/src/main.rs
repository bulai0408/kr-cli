use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod commands;
mod output;

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
}

/// What every command gets from the process.
pub struct Ctx {
    pub cwd: std::path::PathBuf,
    pub env: Vec<(String, String)>,
    pub json: bool,
}

impl Ctx {
    pub fn env_var(&self, key: &str) -> Option<&str> {
        self.env
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Loads the configuration and resolves the state directory.
    pub fn load(&self) -> anyhow::Result<Loaded> {
        let config = kr_config::Sources::discover(&self.cwd, self.env.clone()).load()?;
        let state_dir = config.machine.state_dir(&self.env).ok_or_else(|| {
            anyhow::anyhow!("cannot find a state directory: set HOME or KR_STATE_DIR")
        })?;
        Ok(Loaded { config, state_dir })
    }
}

pub struct Loaded {
    pub config: kr_config::Config,
    pub state_dir: std::path::PathBuf,
}

impl Loaded {
    pub fn pool(&self) -> kr_resource::Pool {
        kr_resource::Pool::new(self.state_dir.join("resources"))
    }

    pub fn events(&self) -> kr_events::EventLog {
        kr_events::EventLog::new(self.state_dir.join("events"))
    }

    pub fn project(&self) -> Option<&str> {
        self.config
            .project
            .as_ref()
            .map(|p| p.project.name.as_str())
    }

    /// Slots of `res`. Until the machine profile lands (#4), unset capacities are 1.
    pub fn capacity(&self, res: kr_resource::Resource) -> u32 {
        let r = &self.config.machine.resources;
        let configured = match res {
            kr_resource::Resource::Build => r.build,
            kr_resource::Resource::Sim => r.sim,
            kr_resource::Resource::Light => r.light,
        };
        configured.unwrap_or(1).max(1)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    let ctx = Ctx {
        cwd: std::env::current_dir().unwrap_or_default(),
        env: std::env::vars().collect(),
        json: cli.json,
    };
    let result = match cli.command {
        Command::Config => commands::config::run(&ctx),
        Command::Lock(args) => commands::lock::run(&ctx, args),
        Command::Queue(args) => commands::queue::run(&ctx, args),
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
