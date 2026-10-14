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

/// One line of the event log. Timestamps are always UTC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub ts: Timestamp,
    pub run_id: RunId,
    pub project: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub fields: serde_json::Value,
}

impl Event {
    pub fn now(
        run_id: RunId,
        project: Option<&str>,
        kind: &str,
        fields: serde_json::Value,
    ) -> Self {
        Self::at(Timestamp::now(), run_id, project, kind, fields)
    }

    pub fn at(
        ts: Timestamp,
        run_id: RunId,
        project: Option<&str>,
        kind: &str,
        fields: serde_json::Value,
    ) -> Self {
        Self {
            ts,
            run_id,
            project: project.map(str::to_owned),
            kind: kind.to_owned(),
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
}
