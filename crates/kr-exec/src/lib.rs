//! Command execution for every external tool kr calls (ADR 0008).
//!
//! Production code takes a `&dyn Runner`; tests pass a [`ReplayRunner`] loaded with
//! real outputs recorded from xcodebuild, simctl and friends.

use std::collections::VecDeque;
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// An external command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    /// Hard limit for [`Runner::run_logged`]: the process group is terminated after it.
    pub timeout: Option<Duration>,
}

impl Command {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            timeout: None,
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

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
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
    /// For [`Runner::run_logged`] on a real process: the tail of the combined log.
    pub stdout: String,
    pub stderr: String,
    /// The command ran past its timeout and was killed.
    pub timed_out: bool,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

/// A long-running child process started or finished (see [`set_spawn_observer`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spawn {
    /// Started; the PID also names its process group.
    Started(u32),
    Finished(u32),
}

type Observer = Arc<dyn Fn(Spawn) + Send + Sync>;
static SPAWN_OBSERVER: Mutex<Option<Observer>> = Mutex::new(None);

/// Sets the process-wide function told about long-running children (logged runs and
/// commands kr runs under a slot). kr uses it to keep lock owner records pointing at
/// the process that actually does the work.
pub fn set_spawn_observer(observer: impl Fn(Spawn) + Send + Sync + 'static) {
    *SPAWN_OBSERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(observer));
}

/// Tells the observer, if any, about a child.
pub fn notify_spawn(event: Spawn) {
    let observer = SPAWN_OBSERVER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(observer) = observer {
        observer(event);
    }
}

/// How much of a logged run's output is kept in memory.
pub const TAIL_BYTES: u64 = 64 * 1024;
/// After a timeout: time between SIGTERM and SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(5);

/// Runs external commands. Every call to an external tool goes through this trait.
pub trait Runner: Send + Sync {
    fn run(&self, cmd: &Command) -> Result<Output>;

    /// Runs a long command (e.g. xcodebuild) with stdout and stderr streamed into
    /// `log`, honouring `cmd.timeout`. Only a tail of the output is kept in memory.
    fn run_logged(&self, cmd: &Command, log: &Path) -> Result<Output> {
        let out = self.run(cmd)?;
        write_log(log, &[out.stdout.as_bytes(), out.stderr.as_bytes()])?;
        // Same shape as a real logged run: only the tail of the combined output.
        Ok(Output {
            stdout: tail(log)?,
            stderr: String::new(),
            ..out
        })
    }
}

fn write_log(log: &Path, parts: &[&[u8]]) -> Result<()> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file =
        File::create(log).with_context(|| format!("failed to create {}", log.display()))?;
    for part in parts {
        file.write_all(part)?;
    }
    Ok(())
}

fn tail(log: &Path) -> Result<String> {
    let mut file = File::open(log)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn kill_group(pgid: libc::pid_t, signal: libc::c_int) {
    // SAFETY: plain kill(2) on a process group we started.
    unsafe { libc::kill(-pgid, signal) };
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
            timed_out: false,
        })
    }

    /// The command leads its own process group, so a timeout or a forwarded signal
    /// reaches everything it started. SIGINT, SIGTERM and SIGHUP sent to kr meanwhile
    /// are passed on, and the call returns an error once the command has stopped.
    fn run_logged(&self, cmd: &Command, log: &Path) -> Result<Output> {
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file =
            File::create(log).with_context(|| format!("failed to create {}", log.display()))?;
        let mut command = std::process::Command::new(&cmd.program);
        command
            .args(&cmd.args)
            .envs(cmd.env.iter().cloned())
            .stdin(Stdio::null())
            .stdout(file.try_clone()?)
            .stderr(file)
            .process_group(0);
        if let Some(dir) = &cmd.cwd {
            command.current_dir(dir);
        }
        let pending = Arc::new(AtomicUsize::new(0));
        let mut hooks = Vec::new();
        for signal in [
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
        ] {
            hooks.push(signal_hook::flag::register_usize(
                signal,
                Arc::clone(&pending),
                signal as usize,
            )?);
        }
        let result = (|| {
            let mut child = command
                .spawn()
                .with_context(|| format!("failed to start `{cmd}`"))?;
            let pgid = libc::pid_t::try_from(child.id())?;
            notify_spawn(Spawn::Started(child.id()));
            let deadline = cmd.timeout.map(|t| Instant::now() + t);
            let mut terminated_at: Option<Instant> = None;
            let mut interrupted = None;
            let status = loop {
                if let Some(status) = child.try_wait()? {
                    break status;
                }
                let signal = pending.swap(0, Ordering::SeqCst);
                if signal != 0 {
                    interrupted = Some(signal);
                    kill_group(pgid, signal as libc::c_int);
                }
                match terminated_at {
                    None if deadline.is_some_and(|d| Instant::now() >= d) => {
                        kill_group(pgid, libc::SIGTERM);
                        terminated_at = Some(Instant::now());
                    }
                    Some(at) if at.elapsed() >= KILL_GRACE => kill_group(pgid, libc::SIGKILL),
                    _ => {}
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            notify_spawn(Spawn::Finished(child.id()));
            // Whatever the command left behind in its group goes too.
            if terminated_at.is_some() || interrupted.is_some() {
                kill_group(pgid, libc::SIGKILL);
            }
            if let Some(signal) = interrupted {
                bail!(
                    "`{cmd}` was interrupted (signal {signal}); its log is {}",
                    log.display()
                );
            }
            Ok(Output {
                status: status.code().or_else(|| status.signal().map(|s| 128 + s)),
                stdout: tail(log)?,
                stderr: String::new(),
                timed_out: terminated_at.is_some(),
            })
        })();
        for hook in hooks {
            signal_hook::low_level::unregister(hook);
        }
        result
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
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
            timed_out: self.timed_out,
        }
    }
}

/// Replays recorded outputs. A command recorded several times is answered in recorded order.
#[derive(Debug)]
pub struct ReplayRunner {
    pending: Mutex<VecDeque<Recording>>,
    used: Mutex<Vec<Recording>>,
    calls: Mutex<Vec<String>>,
}

impl ReplayRunner {
    pub fn new(recordings: Vec<Recording>) -> Self {
        Self {
            pending: Mutex::new(recordings.into()),
            used: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// The command lines answered so far, in call order.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
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
            self.calls.lock().unwrap().push(cmd.to_string());
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
            timed_out: rec.timed_out,
        }
    }

    fn clean(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for secret in &self.secrets {
            text = text.replace(secret.as_str(), "<redacted>");
        }
        // Paths appear plain and, in JSON from tools like simctl, with escaped slashes.
        if !self.home.is_empty() {
            text = text.replace(self.home.as_str(), "~");
            text = text.replace(&self.home.replace('/', "\\/"), "~");
        }
        if !self.user.is_empty() {
            // Only as a path segment: a short user name may also be part of normal words.
            let user = &self.user;
            text = text.replace(&format!("/Users/{user}/"), "/Users/<user>/");
            text = text.replace(&format!("\\/Users\\/{user}\\/"), "\\/Users\\/<user>\\/");
        }
        text
    }
}

/// Runs commands with another runner and saves every call as a sanitized recording:
/// `<dir>/001-xcodebuild.json`, `<dir>/002-xcrun.json`, … For capturing fixtures.
pub struct RecordingRunner<R> {
    inner: R,
    dir: PathBuf,
    sanitizer: Sanitizer,
    count: AtomicUsize,
}

impl<R: Runner> RecordingRunner<R> {
    pub fn new(inner: R, dir: impl Into<PathBuf>, sanitizer: Sanitizer) -> Self {
        Self {
            inner,
            dir: dir.into(),
            sanitizer,
            count: AtomicUsize::new(0),
        }
    }

    fn save(&self, cmd: &Command, out: &Output, combined: Option<String>) -> Result<()> {
        let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        let name = Path::new(&cmd.program)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("cmd");
        let rec = self.sanitizer.sanitize(Recording {
            program: cmd.program.clone(),
            args: cmd.args.clone(),
            status: out.status,
            stdout: combined.unwrap_or_else(|| out.stdout.clone()),
            stderr: out.stderr.clone(),
            timed_out: out.timed_out,
        });
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(format!("{n:03}-{name}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&rec)? + "\n")?;
        Ok(())
    }
}

impl<R: Runner> Runner for RecordingRunner<R> {
    fn run(&self, cmd: &Command) -> Result<Output> {
        let out = self.inner.run(cmd)?;
        self.save(cmd, &out, None)?;
        Ok(out)
    }

    fn run_logged(&self, cmd: &Command, log: &Path) -> Result<Output> {
        let out = self.inner.run_logged(cmd, log)?;
        // The whole log, not just the tail, so a replay reproduces it.
        let full = std::fs::read_to_string(log).unwrap_or_default();
        self.save(cmd, &out, Some(full))?;
        Ok(out)
    }
}
