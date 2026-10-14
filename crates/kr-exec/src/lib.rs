//! Command execution for every external tool kr calls (ADR 0008).
//!
//! Production code takes a `&dyn Runner`; tests pass a [`ReplayRunner`] loaded with
//! real outputs recorded from xcodebuild, simctl and friends.

use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// An external command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl Command {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.program)?;
        for arg in &self.args {
            write!(f, " {arg}")?;
        }
        Ok(())
    }
}

/// What a finished command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Exit code; `None` when the process was killed by a signal.
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

/// Runs external commands. Every call to an external tool goes through this trait.
pub trait Runner: Send + Sync {
    fn run(&self, cmd: &Command) -> Result<Output>;
}

/// Runs commands for real and captures their output.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealRunner;

impl Runner for RealRunner {
    fn run(&self, cmd: &Command) -> Result<Output> {
        let mut command = std::process::Command::new(&cmd.program);
        command.args(&cmd.args).envs(cmd.env.iter().cloned());
        if let Some(dir) = &cmd.cwd {
            command.current_dir(dir);
        }
        let out = command
            .output()
            .with_context(|| format!("failed to start `{cmd}`"))?;
        Ok(Output {
            status: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// One recorded command and its output, as stored in `tests/fixtures/<tool>/*.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recording {
    pub program: String,
    pub args: Vec<String>,
    pub status: Option<i32>,
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}

impl Recording {
    fn matches(&self, cmd: &Command) -> bool {
        self.program == cmd.program && self.args == cmd.args
    }

    fn output(&self) -> Output {
        Output {
            status: self.status,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
        }
    }
}

/// Replays recorded outputs. A command recorded several times is answered in recorded order.
#[derive(Debug)]
pub struct ReplayRunner {
    pending: Mutex<VecDeque<Recording>>,
    used: Mutex<Vec<Recording>>,
}

impl ReplayRunner {
    pub fn new(recordings: Vec<Recording>) -> Self {
        Self {
            pending: Mutex::new(recordings.into()),
            used: Mutex::new(Vec::new()),
        }
    }

    /// Loads one recording per JSON file, in the given order.
    pub fn from_files<I, P>(paths: I) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let recordings = paths
            .into_iter()
            .map(|p| read_recording(p.as_ref()))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::new(recordings))
    }
}

pub fn read_recording(path: &Path) -> Result<Recording> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read recording {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("{} is not a valid recording", path.display()))
}

impl Runner for ReplayRunner {
    fn run(&self, cmd: &Command) -> Result<Output> {
        let mut pending = self.pending.lock().unwrap();
        if let Some(i) = pending.iter().position(|r| r.matches(cmd)) {
            let rec = pending.remove(i).unwrap();
            let out = rec.output();
            self.used.lock().unwrap().push(rec);
            return Ok(out);
        }
        let times = self
            .used
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.matches(cmd))
            .count();
        if times > 0 {
            bail!("`{cmd}` was recorded {times} time(s) and all recordings are used up");
        }
        bail!("no recording for `{cmd}`; record it with the `record` example of kr-exec")
    }
}

/// Removes machine-specific and secret values from a recording before it is saved.
#[derive(Debug, Clone)]
pub struct Sanitizer {
    home: String,
    user: String,
    secrets: Vec<String>,
}

impl Sanitizer {
    pub fn new(home: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            home: home.into(),
            user: user.into(),
            secrets: Vec::new(),
        }
    }

    pub fn with_secrets<I, S>(mut self, secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.secrets.extend(
            secrets
                .into_iter()
                .map(Into::into)
                .filter(|s| !s.is_empty()),
        );
        self
    }

    pub fn sanitize(&self, rec: Recording) -> Recording {
        Recording {
            program: self.clean(&rec.program),
            args: rec.args.iter().map(|a| self.clean(a)).collect(),
            status: rec.status,
            stdout: self.clean(&rec.stdout),
            stderr: self.clean(&rec.stderr),
        }
    }

    fn clean(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in &self.secrets {
            text = text.replace(secret.as_str(), "<redacted>");
        }
        if !self.home.is_empty() {
            text = text.replace(self.home.as_str(), "~");
        }
        if !self.user.is_empty() {
            text = text.replace(self.user.as_str(), "<user>");
        }
        text
    }
}
