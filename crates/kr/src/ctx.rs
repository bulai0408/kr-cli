use std::path::PathBuf;

use anyhow::{Context, Result};
use kr_exec::{RealRunner, ReplayRunner, Runner};
use kr_resource::Resource;
use serde::Serialize;

/// What every command gets from the process.
pub struct Ctx {
    pub cwd: PathBuf,
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

    /// Runs external commands for real, or replays recordings listed in `KR_REPLAY`
    /// (colon-separated files; for kr's own tests).
    pub fn runner(&self) -> Result<Box<dyn Runner>> {
        match self.env_var("KR_REPLAY") {
            Some(list) => Ok(Box::new(ReplayRunner::from_files(list.split(':'))?)),
            None => Ok(Box::new(RealRunner)),
        }
    }

    /// Where Xcode bundles are looked for. `KR_APPLICATIONS` overrides (for kr's tests).
    pub fn applications_dir(&self) -> PathBuf {
        PathBuf::from(
            self.env_var("KR_APPLICATIONS")
                .unwrap_or(kr_machine::APPLICATIONS),
        )
    }

    /// Loads the configuration and resolves the state directory.
    pub fn load(&self) -> Result<Loaded> {
        let config = kr_config::Sources::discover(&self.cwd, self.env.clone()).load()?;
        let state_dir = config
            .machine
            .state_dir(&self.env)
            .context("cannot find a state directory: set HOME or KR_STATE_DIR")?;
        Ok(Loaded { config, state_dir })
    }
}

pub struct Loaded {
    pub config: kr_config::Config,
    pub state_dir: PathBuf,
}

/// A resource capacity and where it came from.
#[derive(Debug, Clone, Serialize)]
pub struct Capacity {
    pub value: u32,
    /// "config: [resources]", "config: [machines.<host>]" or "suggested".
    pub source: String,
    /// How a suggested value was computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
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

    /// Slots of `res`: `[machines.<host>.resources]`, else `[resources]`, else
    /// suggested from the machine profile. Reads only what it needs.
    pub fn capacity(&self, res: Resource, runner: &dyn Runner) -> Result<u32> {
        let host = if self.config.machine.machines.is_empty() {
            None
        } else {
            kr_machine::host_name(runner)
        };
        if let Some(c) = self.configured(res, host.as_deref()) {
            return Ok(c.value);
        }
        Ok(suggested(res, &kr_machine::read_hardware(runner)?).value)
    }

    /// Like [`Loaded::capacity`], with the source, from an already detected profile.
    pub fn capacity_detail(
        &self,
        res: Resource,
        host: Option<&str>,
        hardware: &kr_machine::Hardware,
    ) -> Capacity {
        self.configured(res, host)
            .unwrap_or_else(|| suggested(res, hardware))
    }

    fn configured(&self, res: Resource, host: Option<&str>) -> Option<Capacity> {
        let machine = &self.config.machine;
        let pick = |r: &kr_config::Resources| match res {
            Resource::Build => r.build,
            Resource::Sim => r.sim,
            Resource::Light => r.light,
        };
        if let Some(host) = host
            && let Some(value) = machine.machines.get(host).and_then(|m| pick(&m.resources))
        {
            return Some(Capacity {
                value: value.max(1),
                source: format!("config: [machines.{host}]"),
                formula: None,
            });
        }
        pick(&machine.resources).map(|value| Capacity {
            value: value.max(1),
            source: "config: [resources]".to_owned(),
            formula: None,
        })
    }
}

fn suggested(res: Resource, hardware: &kr_machine::Hardware) -> Capacity {
    let s = kr_machine::suggest(hardware);
    let derived = match res {
        Resource::Build => s.build,
        Resource::Sim => s.sim,
        Resource::Light => s.light,
    };
    Capacity {
        value: derived.value,
        source: "suggested".to_owned(),
        formula: Some(derived.formula),
    }
}
