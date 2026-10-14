//! Adapter for the old machine-wide lock (ADR 0009): a `mkdir` lock directory plus a
//! queue directory, as described in the global AGENTS.md "Native work lock". While other
//! projects and old scripts still use it, kr also takes it whenever it holds a `build`
//! or `sim` slot, so the two never run heavy work at the same time.
//!
//! Protocol: create `<queue>/<epoch-ns>-<task>-<pid>`; try `mkdir <lock>` only when
//! that file is the oldest live one; then write `<lock>/owner` as
//! `<task> <session> <pid> <start>`. Release: remove owner, rmdir, remove the queue
//! file. A lock is stale only if its owner process is gone, or it has no owner and is
//! older than one minute.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;

/// How many guards of this process share each lock directory it owns.
static HELD: Mutex<Option<HashMap<PathBuf, usize>>> = Mutex::new(None);
/// One thread of this process acquires at a time, so parallel lanes of one kr run
/// share the lock instead of queueing behind each other.
static ACQUIRING: Mutex<()> = Mutex::new(());

/// The old protocol's retry interval.
pub const DEFAULT_POLL: Duration = Duration::from_secs(30);
const OWNERLESS_GRACE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct LegacyLock {
    lock_dir: PathBuf,
    queue_dir: PathBuf,
    poll: Duration,
    max_wait: Option<Duration>,
    session: String,
}

/// The content of `<lock>/owner`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub task: String,
    pub session: String,
    pub pid: u32,
    pub started: String,
}

impl Owner {
    fn parse(text: &str) -> Option<Self> {
        let mut f = text.split_whitespace();
        Some(Self {
            task: f.next()?.to_owned(),
            session: f.next()?.to_owned(),
            pid: f.next()?.parse().ok()?,
            started: f.next().unwrap_or("").to_owned(),
        })
    }
}

impl LegacyLock {
    pub fn new(lock_dir: impl Into<PathBuf>, queue_dir: impl Into<PathBuf>) -> Self {
        Self {
            lock_dir: lock_dir.into(),
            queue_dir: queue_dir.into(),
            poll: DEFAULT_POLL,
            max_wait: None,
            session: std::env::var("ORCA_SESSION_ID").unwrap_or_else(|_| "kr".to_owned()),
        }
    }

    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Give up after this long (tests). Default: wait as long as it takes.
    pub fn with_max_wait(mut self, max_wait: Duration) -> Self {
        self.max_wait = Some(max_wait);
        self
    }

    pub fn lock_dir(&self) -> &Path {
        &self.lock_dir
    }

    /// The current owner, if the lock is held.
    pub fn owner(&self) -> Option<Owner> {
        Owner::parse(&std::fs::read_to_string(self.lock_dir.join("owner")).ok()?)
    }

    /// Waits for the lock. `on_wait` is called once, with the current owner, if kr has
    /// to wait.
    pub fn acquire(
        &self,
        task: &str,
        mut on_wait: impl FnMut(Option<&Owner>),
    ) -> Result<LegacyGuard> {
        let _one_at_a_time = ACQUIRING.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = held_map().get_mut(&self.lock_dir) {
            *count += 1;
            return Ok(self.guard(Kind::Shared));
        }
        if self.owner().is_some_and(|o| crate::is_ancestor(o.pid)) {
            return Ok(self.guard(Kind::Ancestor));
        }
        std::fs::create_dir_all(&self.queue_dir)
            .with_context(|| format!("failed to create {}", self.queue_dir.display()))?;
        let task: String = task
            .chars()
            .map(|c| {
                if c.is_whitespace() || c == '/' {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let ticket = self.queue_dir.join(format!(
            "{}-{task}-{}",
            Timestamp::now().as_nanosecond(),
            std::process::id()
        ));
        std::fs::write(&ticket, "")
            .with_context(|| format!("failed to create {}", ticket.display()))?;
        let ticket = Ticket(ticket);
        let start = Instant::now();
        let mut announced = false;
        loop {
            self.remove_dead_tickets();
            self.remove_stale_lock();
            let owner = self.owner();
            if owner.as_ref().is_some_and(|o| crate::is_ancestor(o.pid)) {
                return Ok(self.guard(Kind::Ancestor));
            }
            if self.first_in_line(&ticket.0) && std::fs::create_dir(&self.lock_dir).is_ok() {
                let line = format!(
                    "{task} {} {} {}\n",
                    self.session,
                    std::process::id(),
                    Timestamp::now()
                );
                if let Err(e) = write_owner(&self.lock_dir, &line) {
                    let _ = std::fs::remove_dir(&self.lock_dir);
                    return Err(e).context("failed to write the machine lock owner");
                }
                held_map().insert(self.lock_dir.clone(), 1);
                drop(ticket);
                return Ok(self.guard(Kind::Owner));
            }
            if !announced {
                announced = true;
                on_wait(owner.as_ref());
            }
            if self.max_wait.is_some_and(|max| start.elapsed() >= max) {
                bail!(
                    "the machine lock {} is still taken after {:?}",
                    self.lock_dir.display(),
                    start.elapsed()
                );
            }
            std::thread::sleep(self.poll);
        }
    }

    fn guard(&self, kind: Kind) -> LegacyGuard {
        LegacyGuard {
            kind,
            lock_dir: self.lock_dir.clone(),
        }
    }

    fn tickets(&self) -> Vec<PathBuf> {
        let mut tickets: Vec<PathBuf> = std::fs::read_dir(&self.queue_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        tickets.sort();
        tickets
    }

    fn remove_dead_tickets(&self) {
        for ticket in self.tickets() {
            let pid = ticket
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.rsplit('-').next())
                .and_then(|p| p.parse::<u32>().ok());
            if pid.is_some_and(|p| !alive(p)) {
                let _ = std::fs::remove_file(&ticket);
            }
        }
    }

    fn first_in_line(&self, ticket: &Path) -> bool {
        self.tickets().first().is_some_and(|t| t == ticket)
    }

    fn remove_stale_lock(&self) {
        if !self.lock_dir.is_dir() {
            return;
        }
        match std::fs::read_to_string(self.lock_dir.join("owner")) {
            Ok(text) => {
                if let Some(owner) = Owner::parse(&text)
                    && !alive(owner.pid)
                {
                    let _ = std::fs::remove_file(self.lock_dir.join("owner"));
                    let _ = std::fs::remove_dir(&self.lock_dir);
                }
            }
            Err(_) => {
                let age = std::fs::metadata(&self.lock_dir)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok());
                if age.is_some_and(|a| a > OWNERLESS_GRACE) {
                    let _ = std::fs::remove_dir(&self.lock_dir);
                }
            }
        }
    }
}

/// Rewrites the owner line of every machine lock this process holds so it names `pid`
/// (the process group doing the heavy work). If kr is killed while that group still
/// runs, old scripts see a live owner and wait, as kr's own slots do (orphan rule).
pub fn set_owner_pid(pid: u32) {
    let dirs: Vec<PathBuf> = held_map()
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for dir in dirs {
        let path = dir.join("owner");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut fields: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
        if fields.len() >= 3 {
            fields[2] = pid.to_string();
            let _ = write_owner(&dir, &(fields.join(" ") + "\n"));
        }
    }
}

/// Replaces `<lock>/owner` atomically, so readers never see a partial line.
fn write_owner(lock_dir: &Path, line: &str) -> std::io::Result<()> {
    let tmp = lock_dir.join(format!(".owner-{}", std::process::id()));
    std::fs::write(&tmp, line)?;
    std::fs::rename(&tmp, lock_dir.join("owner"))
}

fn held_map() -> std::sync::MutexGuard<'static, Option<HashMap<PathBuf, usize>>> {
    let mut map = HELD.lock().unwrap_or_else(|e| e.into_inner());
    map.get_or_insert_with(HashMap::new);
    map
}

trait HeldExt {
    fn get_mut(&mut self, key: &Path) -> Option<&mut usize>;
    fn insert(&mut self, key: PathBuf, value: usize);
}

impl HeldExt for std::sync::MutexGuard<'static, Option<HashMap<PathBuf, usize>>> {
    fn get_mut(&mut self, key: &Path) -> Option<&mut usize> {
        self.as_mut().and_then(|m| m.get_mut(key))
    }

    fn insert(&mut self, key: PathBuf, value: usize) {
        if let Some(m) = self.as_mut() {
            m.insert(key, value);
        }
    }
}

fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Removes the queue file when waiting ends, however it ends.
struct Ticket(PathBuf);

impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// This guard created the lock directory.
    Owner,
    /// Another guard of this process holds it.
    Shared,
    /// An ancestor process holds it (e.g. an old script that calls kr).
    Ancestor,
}

/// Holds the machine lock (or a share of it). The last guard of the process releases it.
#[derive(Debug)]
pub struct LegacyGuard {
    kind: Kind,
    lock_dir: PathBuf,
}

impl LegacyGuard {
    /// True if this guard took the lock itself.
    pub fn is_owned(&self) -> bool {
        self.kind == Kind::Owner
    }

    /// True if an ancestor process holds the lock and this guard only relies on it.
    pub fn is_ancestors(&self) -> bool {
        self.kind == Kind::Ancestor
    }
}

impl Drop for LegacyGuard {
    fn drop(&mut self) {
        if self.kind == Kind::Ancestor {
            return;
        }
        let mut map = held_map();
        let Some(count) = map.get_mut(&self.lock_dir) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            if let Some(m) = map.as_mut() {
                m.remove(&self.lock_dir);
            }
            let _ = std::fs::remove_file(self.lock_dir.join("owner"));
            let _ = std::fs::remove_dir(&self.lock_dir);
        }
    }
}
