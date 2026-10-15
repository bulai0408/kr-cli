//! Event records: one JSON line per step of a run (spec §7).
//!
//! Events go to `<state dir>/events/<UTC date>.ndjson`. Every line is written while the
//! file holds an exclusive lock, so lines from concurrent kr processes never interleave.

use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Identifies one run. UUID v7, so IDs sort by creation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(Uuid);

impl RunId {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for RunId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// What happened. New kinds are added as later commands start writing events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A task joined the queue of a resource.
    QueueStart,
    /// A task got a slot. Fields include `waited_ms`.
    SlotAcquired,
    /// A task gave a slot back. Fields include `held_ms`.
    SlotReleased,
    /// A nested `kr lock` claimed a held slot it could not prove it owns.
    ReentryDenied,
    /// Heavy work is waiting because the machine is too busy. Fields: load, reason.
    LoadWait,
    /// The load check passed. Fields: `waited_ms`, `checks`, `timed_out`.
    LoadOk,
    /// The old machine-wide lock was taken (ADR 0009). Fields: `waited_ms`, owner it
    /// waited for.
    MachineLock,
    /// A build was skipped: its inputs match its last successful build. Fields: job, hash.
    BuildCacheHit,
    /// A step of a command finished. Fields: command, step, ok, `duration_ms`.
    StepEnd,
    /// A scenario or suite started. Fields: item, kind, device.
    ItemStart,
    /// A scenario or suite finished. Fields: item, kind, passed, `duration_ms`, device,
    /// tests (counts per outcome), flaky.
    ///
    /// The outcome "unverified" (the device or host was lost during the run) is written
    /// as an `unverified` count in `tests`, next to passed / failed / flaky / skipped,
    /// with the reason in `unverified_reason` (a string, may be missing). An item with
    /// unverified tests does not count as passed. `kr deliver` reads this.
    ItemEnd,
    /// A failed test was run again on its own. Fields: item, test, passed.
    Rerun,
    /// A filter passed the static check but selected nothing on the device.
    FilterMismatch,
    /// A pool simulator was leased and booted. Fields: device, udid, `boot_ms`.
    SimLeased,
    /// A leased simulator was shut down and freed. Fields: device, udid, `held_ms`.
    SimReleased,
    /// A kind written by a newer kr.
    #[serde(other)]
    Other,
}

/// One line of the event log. Timestamps are always UTC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub ts: Timestamp,
    pub run_id: RunId,
    pub project: Option<String>,
    #[serde(rename = "type")]
    pub kind: EventKind,
    pub fields: serde_json::Value,
}

impl Event {
    pub fn now(
        run_id: RunId,
        project: Option<&str>,
        kind: EventKind,
        fields: serde_json::Value,
    ) -> Self {
        Self::at(Timestamp::now(), run_id, project, kind, fields)
    }

    pub fn at(
        ts: Timestamp,
        run_id: RunId,
        project: Option<&str>,
        kind: EventKind,
        fields: serde_json::Value,
    ) -> Self {
        Self {
            ts,
            run_id,
            project: project.map(str::to_owned),
            kind,
            fields,
        }
    }
}

/// Appends events to day files in one directory.
#[derive(Debug, Clone)]
pub struct EventLog {
    dir: PathBuf,
}

impl EventLog {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn append(&self, event: &Event) -> Result<()> {
        let day = event.ts.to_zoned(jiff::tz::TimeZone::UTC).date();
        let path = self.dir.join(format!("{day}.ndjson"));
        let mut line = serde_json::to_vec(event)?;
        line.push(b'\n');

        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("failed to create {}", self.dir.display()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        file.lock()
            .with_context(|| format!("failed to lock {}", path.display()))?;
        file.write_all(&line)
            .with_context(|| format!("failed to write {}", path.display()))?;
        // The lock is released when `file` is closed.
        Ok(())
    }

    /// Events at or after `since`, oldest file first. Lines that do not parse are skipped.
    pub fn read_since(&self, since: Timestamp) -> Result<Vec<Event>> {
        let first_day = since.to_zoned(jiff::tz::TimeZone::UTC).date().to_string();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Ok(Vec::new());
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "ndjson")
                    && p.file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|day| day >= first_day.as_str())
            })
            .collect();
        files.sort();
        let mut events = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            events.extend(
                text.lines()
                    .filter_map(|l| serde_json::from_str::<Event>(l).ok())
                    .filter(|e| e.ts >= since),
            );
        }
        Ok(events)
    }
}
