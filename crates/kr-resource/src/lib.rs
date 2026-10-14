//! Resource layer (ADR 0004): named resources with a capacity, one lock file per slot,
//! and a first-come-first-served queue shared by every project on the machine.
//!
//! Layout under the pool root (`<state dir>/resources`):
//!
//! ```text
//! <resource>/slot-<N>.lock       held with File::try_lock; content = holder JSON
//! <resource>/queue/<ns>-<pid>.json  one ticket per waiter, locked by its owner
//! ```
//!
//! Locks are released by the OS when a process exits or crashes, so there is no
//! stale-lock detection by age. A ticket nobody holds a lock on belongs to a dead
//! waiter and is removed.
//!
//! The command kr runs under a slot gets its own process group, recorded in the slot
//! file. If kr is killed, the lock goes away but the record stays; the slot counts as
//! held ("orphan") while any process of that group is alive, so killing kr never lets
//! two heavy jobs run at once.

pub mod legacy;

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// A kind of work that has to wait its turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resource {
    /// Heavy builds: app builds, build-for-testing.
    Build,
    /// Running simulators.
    Sim,
    /// Light single-module tests.
    Light,
}

impl Resource {
    pub const ALL: [Resource; 3] = [Resource::Build, Resource::Sim, Resource::Light];

    pub fn as_str(self) -> &'static str {
        match self {
            Resource::Build => "build",
            Resource::Sim => "sim",
            Resource::Light => "light",
        }
    }
}

impl fmt::Display for Resource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Resource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Resource::ALL
            .into_iter()
            .find(|r| r.as_str() == s)
            .with_context(|| format!("unknown resource `{s}`; use build, sim or light"))
    }
}

/// Who holds or waits for a slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub project: Option<String>,
    pub task: String,
    /// The kr process.
    pub pid: u32,
    /// The command kr runs while holding the slot. It leads its own process group,
    /// so this is also the group ID.
    pub child_pid: Option<u32>,
    /// When the slot was taken, or for a waiter, when it joined the queue.
    pub started: Timestamp,
}

/// All resources of one machine.
#[derive(Debug, Clone)]
pub struct Pool {
    root: PathBuf,
}

impl Pool {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dir(&self, res: Resource) -> PathBuf {
        self.root.join(res.as_str())
    }

    fn slot_path(&self, res: Resource, index: u32) -> PathBuf {
        self.dir(res).join(format!("slot-{index}.lock"))
    }

    fn queue_dir(&self, res: Resource) -> PathBuf {
        self.dir(res).join("queue")
    }

    /// Takes any free slot, ignoring the queue. Callers that wait use [`Ticket`].
    pub fn try_acquire(
        &self,
        res: Resource,
        capacity: u32,
        holder: &Holder,
    ) -> Result<Option<Slot>> {
        std::fs::create_dir_all(self.dir(res))?;
        for index in 1..=capacity {
            let path = self.slot_path(res, index);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .with_context(|| format!("failed to open {}", path.display()))?;
            match file.try_lock() {
                Ok(()) if orphan_alive(&path) => continue,
                Ok(()) => {
                    let mut slot = Slot {
                        resource: res,
                        index,
                        file,
                        holder: Holder {
                            started: Timestamp::now(),
                            ..holder.clone()
                        },
                    };
                    slot.write_holder()?;
                    return Ok(Some(slot));
                }
                Err(TryLockError::WouldBlock) => continue,
                Err(TryLockError::Error(e)) => {
                    return Err(e).with_context(|| format!("failed to lock {}", path.display()));
                }
            }
        }
        Ok(None)
    }

    /// Joins the end of the queue for `res`.
    pub fn enqueue(&self, res: Resource, holder: &Holder) -> Result<Ticket> {
        let queue = self.queue_dir(res);
        std::fs::create_dir_all(&queue)?;
        let ns = Timestamp::now().as_nanosecond();
        let name = format!("{ns:020}-{}.json", std::process::id());
        // Lock before the ticket becomes visible, so nobody mistakes it for a dead waiter.
        let tmp = queue.join(format!(".{name}.tmp"));
        let mut file = File::create(&tmp)?;
        file.lock()?;
        file.write_all(&serde_json::to_vec(holder)?)?;
        let path = queue.join(name);
        std::fs::rename(&tmp, &path)?;
        Ok(Ticket {
            pool: self.clone(),
            resource: res,
            path,
            holder: holder.clone(),
            _file: file,
        })
    }

    /// True if someone holds slot `index` of `res`, or an orphaned command still runs in it.
    pub fn is_held(&self, res: Resource, index: u32) -> bool {
        self.slot_state(res, index) != SlotState::Free
    }

    fn slot_state(&self, res: Resource, index: u32) -> SlotState {
        let path = self.slot_path(res, index);
        let Ok(file) = File::open(&path) else {
            return SlotState::Free;
        };
        match file.try_lock() {
            Err(TryLockError::WouldBlock) => SlotState::Held,
            _ if orphan_alive(&path) => SlotState::Orphan,
            _ => SlotState::Free,
        }
    }

    /// The holder record of slot `index`, if it can be read.
    pub fn holder(&self, res: Resource, index: u32) -> Option<Holder> {
        read_holder(&self.slot_path(res, index))
    }

    /// Current holders and waiters. Removes tickets of dead waiters.
    pub fn status(&self, res: Resource, capacity: u32) -> Result<Status> {
        let mut holders = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.dir(res)) {
            let mut indexes: Vec<u32> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    e.file_name()
                        .to_str()?
                        .strip_prefix("slot-")?
                        .strip_suffix(".lock")?
                        .parse()
                        .ok()
                })
                .collect();
            indexes.sort_unstable();
            for index in indexes {
                let state = self.slot_state(res, index);
                if state != SlotState::Free {
                    holders.push(HeldSlot {
                        slot: index,
                        holder: self.holder(res, index),
                        orphan: state == SlotState::Orphan,
                    });
                }
            }
        }
        Ok(Status {
            resource: res,
            capacity,
            holders,
            waiters: self.waiters(res)?,
        })
    }

    fn waiters(&self, res: Resource) -> Result<Vec<Waiter>> {
        let Ok(entries) = std::fs::read_dir(self.queue_dir(res)) else {
            return Ok(Vec::new());
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        let mut waiters = Vec::new();
        for path in paths {
            let Ok(file) = File::open(&path) else {
                continue; // just left the queue
            };
            match file.try_lock() {
                Ok(()) => {
                    // Its owner is gone.
                    let _ = std::fs::remove_file(&path);
                }
                Err(TryLockError::WouldBlock) => {
                    if let Some(holder) = std::fs::read(&path)
                        .ok()
                        .and_then(|b| serde_json::from_slice(&b).ok())
                    {
                        waiters.push(Waiter { path, holder });
                    }
                }
                Err(TryLockError::Error(e)) => {
                    return Err(e).with_context(|| format!("failed to check {}", path.display()));
                }
            }
        }
        Ok(waiters)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    Free,
    Held,
    Orphan,
}

fn read_holder(path: &Path) -> Option<Holder> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
}

/// True if the slot record names a command group that still has live processes.
fn orphan_alive(path: &Path) -> bool {
    read_holder(path)
        .and_then(|h| h.child_pid)
        .is_some_and(group_alive)
}

fn group_alive(pgid: u32) -> bool {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return false;
    };
    // SAFETY: signal 0 only checks whether the group exists; nothing is sent.
    let rc = unsafe { libc::kill(-pgid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// True if `pid` is this process or one of its ancestors.
pub fn is_ancestor(pid: u32) -> bool {
    let mut current = std::process::id();
    for _ in 0..64 {
        if current == pid {
            return true;
        }
        match parent_of(current) {
            Some(parent) if parent != current && parent != 0 => current = parent,
            _ => return false,
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn parent_of(pid: u32) -> Option<u32> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is a properly sized, writable proc_bsdinfo.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (n == size).then_some(info.pbi_ppid)
}

#[cfg(not(target_os = "macos"))]
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// A held slot. Dropping it gives the slot back.
#[derive(Debug)]
pub struct Slot {
    resource: Resource,
    index: u32,
    file: File,
    holder: Holder,
}

impl Slot {
    pub fn resource(&self) -> Resource {
        self.resource
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    /// Records the PID of the command running under this slot, for `kr queue`.
    pub fn set_child_pid(&mut self, pid: u32) -> Result<()> {
        self.holder.child_pid = Some(pid);
        self.write_holder()
    }

    fn write_holder(&mut self) -> Result<()> {
        self.file.set_len(0)?;
        self.file.rewind()?;
        self.file.write_all(&serde_json::to_vec(&self.holder)?)?;
        Ok(())
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let _ = self.file.set_len(0);
        // Closing the file releases the lock.
    }
}

/// A place in the queue. Dropping it leaves the queue.
#[derive(Debug)]
pub struct Ticket {
    pool: Pool,
    resource: Resource,
    path: PathBuf,
    holder: Holder,
    _file: File,
}

impl Ticket {
    /// How many live waiters are ahead of this ticket.
    pub fn position(&self, capacity: u32) -> Result<usize> {
        let status = self.pool.status(self.resource, capacity)?;
        status
            .waiters
            .iter()
            .position(|w| w.path == self.path)
            .context("queue ticket disappeared")
    }

    /// Takes a slot if one is free and no earlier waiter is still entitled to it.
    pub fn try_acquire(&self, capacity: u32) -> Result<Option<Slot>> {
        let status = self.pool.status(self.resource, capacity)?;
        let Some(position) = status.waiters.iter().position(|w| w.path == self.path) else {
            bail!("queue ticket {} disappeared", self.path.display());
        };
        let free = (capacity as usize).saturating_sub(status.holders.len());
        if position >= free {
            return Ok(None);
        }
        self.pool.try_acquire(self.resource, capacity, &self.holder)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub resource: Resource,
    pub capacity: u32,
    pub holders: Vec<HeldSlot>,
    pub waiters: Vec<Waiter>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeldSlot {
    pub slot: u32,
    /// `None` if the holder info could not be read yet.
    pub holder: Option<Holder>,
    /// kr is gone but the command it started still runs.
    pub orphan: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Waiter {
    #[serde(skip)]
    path: PathBuf,
    #[serde(flatten)]
    pub holder: Holder,
}

impl Waiter {
    pub fn ticket_path(&self) -> &Path {
        &self.path
    }
}

/// Expected wait for each waiter, in queue order.
///
/// Each held slot frees up after `avg_hold` minus how long it has been held (never
/// below zero); each waiter takes the slot that frees up first and then holds it for
/// `avg_hold`.
pub fn estimate_waits(
    avg_hold: Duration,
    capacity: u32,
    held_for: &[Duration],
    waiters: usize,
) -> Vec<Duration> {
    let mut free_at: Vec<Duration> = held_for
        .iter()
        .map(|h| avg_hold.saturating_sub(*h))
        .collect();
    free_at.resize(free_at.len().max(capacity as usize), Duration::ZERO);
    (0..waiters)
        .map(|_| {
            let (i, &t) = free_at
                .iter()
                .enumerate()
                .min_by_key(|(_, t)| **t)
                .expect("capacity is at least one");
            free_at[i] = t + avg_hold;
            t
        })
        .collect()
}
