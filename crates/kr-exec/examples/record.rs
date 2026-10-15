//! Records a real command's output as a replay fixture.
//!
//! Run only by a developer, on purpose:
//!
//! ```sh
//! cargo run -p kr-exec --example record -- tests/fixtures/simctl/list.json \
//!     [--redact-file ~/.config/krill/dev.env] -- xcrun simctl list -j
//! ```
//!
//! The home directory, the user name and every value in the redact file
//! (`KEY=VALUE` lines) are removed before the file is written.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use kr_exec::{Command, RealRunner, Recording, Runner, Sanitizer};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let out_path = PathBuf::from(args.next().context("missing output path")?);
    let mut secrets = Vec::new();
    loop {
        match args.next().as_deref() {
            Some("--") => break,
            Some("--redact-file") => {
                let path = args.next().context("--redact-file needs a path")?;
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {path}"))?;
                secrets.extend(
                    text.lines()
                        .filter_map(|l| l.split_once('='))
                        .map(|(_, v)| v.trim().trim_matches('"').to_owned()),
                );
            }
            Some(other) => bail!("unexpected argument {other}; put the command after `--`"),
            None => bail!("missing `-- <command>`"),
        }
    }
    let program = args.next().context("missing command after `--`")?;
    let cmd = Command::new(program).args(args);

    let out = RealRunner.run(&cmd)?;
    let recording = Recording {
        program: cmd.program.clone(),
        args: cmd.args.clone(),
        status: out.status,
        stdout: out.stdout,
        stderr: out.stderr,
        timed_out: out.timed_out,
        duration_ms: None,
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    let clean = Sanitizer::new(home, user)
        .with_secrets(secrets)
        .sanitize(recording);

    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&clean)? + "\n")?;
    eprintln!(
        "recorded `{} {}` (exit {:?}) to {}",
        clean.program,
        clean.args.join(" "),
        clean.status,
        out_path.display()
    );
    Ok(())
}
