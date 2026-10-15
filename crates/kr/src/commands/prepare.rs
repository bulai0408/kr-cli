//! Scenario dependency lifetimes (ADR 0020).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kr_config::{Prepare, parse_timeout};
use kr_exec::{Background, Command, Runner};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Report {
    pub step: usize,
    pub shared: Option<String>,
    pub scenarios: Vec<String>,
    pub status: String,
    pub log: PathBuf,
    pub ready_log: PathBuf,
    pub ready_attempts: usize,
    pub cleanup: Option<String>,
    pub cleanup_log: PathBuf,
    pub duration_ms: u64,
    pub reason: Option<String>,
}

pub(crate) struct Context<'a> {
    pub runner: &'a dyn Runner,
    pub root: &'a Path,
    pub events: kr_events::EventLog,
    pub run_id: kr_events::RunId,
    pub project: String,
}

pub(crate) struct Instance<'a> {
    context: Context<'a>,
    cwd: PathBuf,
    env: Vec<(String, String)>,
    spec: &'a Prepare,
    process: Option<Box<dyn Background>>,
    pub report: Report,
    started: Instant,
}

impl<'a> Instance<'a> {
    pub fn start(
        context: Context<'a>,
        spec: &'a Prepare,
        dir: &Path,
        step: usize,
        env: Vec<(String, String)>,
        scenarios: Vec<String>,
    ) -> Self {
        let mut instance = Self {
            cwd: context
                .root
                .join(spec.dir.as_deref().unwrap_or(Path::new(""))),
            context,
            env,
            spec,
            process: None,
            started: Instant::now(),
            report: Report {
                step,
                shared: spec.shared.clone(),
                scenarios,
                status: "starting".into(),
                log: dir.join(format!("prepare-{step}.log")),
                ready_log: dir.join(format!("ready-{step}.log")),
                ready_attempts: 0,
                cleanup: None,
                cleanup_log: dir.join(format!("cleanup-{step}.log")),
                duration_ms: 0,
                reason: None,
            },
        };
        if let Err(e) = instance
            .event(kr_events::EventKind::PreparationStart)
            .and_then(|()| instance.prepare())
        {
            instance.report.status = "failed".into();
            instance.report.reason = Some(format!(
                "preparation step {step} failed: {e:#}; inspect {} and fix the service configuration, then run again",
                instance.report.log.display()
            ));
        } else {
            instance.report.status = "ready".into();
        }
        instance
    }

    fn prepare(&mut self) -> anyhow::Result<()> {
        let timeout = self
            .spec
            .timeout
            .as_deref()
            .and_then(parse_timeout)
            .unwrap_or(Duration::from_secs(30));
        let cmd = self.command(&self.spec.run, timeout);
        if !self.spec.background {
            let out = self.context.runner.run_logged(&cmd, &self.report.log)?;
            anyhow::ensure!(
                out.success() && !out.timed_out,
                "exit {:?}, timed_out={}",
                out.status,
                out.timed_out
            );
            return Ok(());
        }
        self.process = Some(self.context.runner.start(&cmd, &self.report.log)?);
        let deadline = Instant::now() + timeout;
        loop {
            anyhow::ensure!(kr_exec::stop_signal().is_none(), "interrupted");
            anyhow::ensure!(
                !self.process.as_mut().unwrap().exited()?,
                "background process exited before readiness"
            );
            let remaining = deadline.saturating_duration_since(Instant::now());
            anyhow::ensure!(
                !remaining.is_zero(),
                "readiness timed out; inspect {}",
                self.report.ready_log.display()
            );
            self.report.ready_attempts += 1;
            let cmd = self.command(self.spec.ready.as_deref().unwrap_or("false"), remaining);
            let out = self
                .context
                .runner
                .run_logged(&cmd, &self.report.ready_log)?;
            if out.success() && !out.timed_out {
                return Ok(());
            }
            std::thread::sleep(
                Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }

    fn event(&self, kind: kr_events::EventKind) -> anyhow::Result<()> {
        self.context.events.append(&kr_events::Event::now(
            self.context.run_id,
            Some(&self.context.project),
            kind,
            serde_json::to_value(&self.report)?,
        ))
    }

    fn command(&self, script: &str, timeout: Duration) -> Command {
        let mut cmd = Command::new("sh")
            .args(["-c", script])
            .current_dir(&self.cwd)
            .timeout(timeout);
        cmd.env = self.env.clone();
        cmd
    }

    pub fn finish(&mut self) {
        if self.report.cleanup.is_some() {
            return;
        }
        self.report.cleanup = Some("passed".into());
        if let Some(process) = &mut self.process
            && !matches!(process.exited(), Ok(false))
        {
            self.report.reason.get_or_insert_with(|| {
                format!(
                    "background service exited before cleanup; inspect {} and fix the service, then run again",
                    self.report.log.display()
                )
            });
        }
        let process_ok = self.process.as_mut().is_none_or(|p| p.stop().is_ok());
        let script_ok = if let Some(script) = &self.spec.cleanup {
            let mut cmd = self.command(script, Duration::from_secs(5));
            cmd.cleanup = true;
            matches!(self.context.runner.run_logged(&cmd, &self.report.cleanup_log), Ok(out) if out.success() && !out.timed_out)
        } else {
            true
        };
        if !process_ok || !script_ok {
            self.report.cleanup = Some("failed".into());
            self.report.reason.get_or_insert_with(|| format!("preparation cleanup failed; inspect {} and clean the service state, then run again", self.report.cleanup_log.display()));
        }
        self.report.duration_ms = self.started.elapsed().as_millis() as u64;
        if let Err(e) = self.event(kr_events::EventKind::PreparationEnd) {
            self.report
                .reason
                .get_or_insert_with(|| format!("could not record preparation cleanup: {e:#}"));
        }
    }
}

impl Drop for Instance<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

pub(crate) struct SharedInstances<'a> {
    entries: std::sync::Mutex<Vec<(String, SharedEntry<'a>)>>,
    completed: std::sync::Mutex<std::collections::BTreeSet<String>>,
    users: std::collections::BTreeMap<String, Vec<String>>,
}

struct SharedEntry<'a> {
    instance: Instance<'a>,
    remaining: std::collections::BTreeSet<String>,
}

impl<'a> SharedInstances<'a> {
    pub fn new(project: &kr_config::ProjectConfig, selected: &[String]) -> Self {
        let mut users = std::collections::BTreeMap::<String, Vec<String>>::new();
        for name in selected {
            for step in &project.scenarios[name].prepare {
                if let Some(shared) = &step.shared {
                    let names = users.entry(shared.clone()).or_default();
                    if !names.contains(name) {
                        names.push(name.clone());
                    }
                }
            }
        }
        Self {
            entries: Default::default(),
            completed: Default::default(),
            users,
        }
    }

    pub fn acquire(
        &self,
        key: &str,
        scenario: &str,
        start: impl FnOnce(Vec<String>) -> Instance<'a>,
    ) -> Report {
        let mut entries = self.entries.lock().unwrap();
        let index = if let Some(index) = entries.iter().position(|(name, _)| name == key) {
            index
        } else {
            let users = self.users[key].clone();
            let remaining = {
                let completed = self.completed.lock().unwrap();
                users
                    .iter()
                    .filter(|name| !completed.contains(*name))
                    .cloned()
                    .collect()
            };
            entries.push((
                key.to_owned(),
                SharedEntry {
                    remaining,
                    instance: start(vec![scenario.to_owned()]),
                },
            ));
            entries.len() - 1
        };
        let report = &mut entries[index].1.instance.report;
        if !report.scenarios.iter().any(|name| name == scenario) {
            report.scenarios.push(scenario.to_owned());
            report.scenarios.sort();
        }
        report.clone()
    }

    pub fn release(&self, scenario: &str) {
        self.completed.lock().unwrap().insert(scenario.to_owned());
        for (_, entry) in self.entries.lock().unwrap().iter_mut().rev() {
            entry.remaining.remove(scenario);
            if entry.remaining.is_empty() || kr_exec::stop_signal().is_some() {
                entry.instance.finish();
            }
        }
    }

    pub fn release_step(&self, key: &str, scenario: &str) {
        if let Some((_, entry)) = self
            .entries
            .lock()
            .unwrap()
            .iter_mut()
            .find(|(name, _)| name == key)
        {
            entry.remaining.remove(scenario);
            if entry.remaining.is_empty() || kr_exec::stop_signal().is_some() {
                entry.instance.finish();
            }
        }
    }

    pub fn finish(&self) {
        for (_, entry) in self.entries.lock().unwrap().iter_mut().rev() {
            entry.instance.finish();
        }
    }

    pub fn report(&self, key: &str) -> Option<Report> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, e)| e.instance.report.clone())
    }
}

impl Drop for SharedInstances<'_> {
    fn drop(&mut self) {
        for (_, entry) in self
            .entries
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
            .rev()
        {
            entry.instance.finish();
        }
    }
}
