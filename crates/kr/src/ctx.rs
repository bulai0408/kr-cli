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
        // KR_RECORD=<dir>: run for real and save every call as a fixture (developers only).
        if let Some(dir) = self.env_var("KR_RECORD") {
            let sanitizer = kr_exec::Sanitizer::new(
                self.env_var("HOME").unwrap_or(""),
                self.env_var("USER").unwrap_or(""),
            );
            return Ok(Box::new(kr_exec::RecordingRunner::new(
                RealRunner, dir, sanitizer,
            )));
        }
        match self.env_var("KR_REPLAY") {
            Some(list) => Ok(Box::new(ReplayRunner::from_files(
                list.split(':').filter(|p| !p.is_empty()),
            )?)),
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
        let sources = kr_config::Sources::discover(&self.cwd, self.env.clone());
        let config = sources.load()?;
        let project_file = sources.project.filter(|_| config.project.is_some());
        let state_dir = config
            .machine
            .state_dir(&self.env)
            .context("cannot find a state directory: set HOME or KR_STATE_DIR")?;
        Ok(Loaded {
            config,
            state_dir,
            project_file,
        })
    }
}

pub struct Loaded {
    pub config: kr_config::Config,
    pub state_dir: PathBuf,
    /// The `.kr.toml` the project came from; its directory is the repo root.
    pub project_file: Option<PathBuf>,
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
    /// The project and its repo root, or an error saying how to get one.
    pub fn require_project(&self) -> Result<(&kr_config::ProjectConfig, PathBuf)> {
        let project = self.config.project.as_ref().context(
            "no .kr.toml found from here up to the repo root; run `kr init` in the project root, then read `kr skill --reference config`",
        )?;
        let root = self
            .project_file
            .as_deref()
            .and_then(|f| f.parent())
            .context("cannot find the repo root")?
            .to_owned();
        Ok((project, root))
    }

    /// Where a run keeps its logs: `<state>/runs/<run id>`.
    pub fn run_dir(&self, run_id: kr_events::RunId) -> PathBuf {
        self.state_dir.join("runs").join(run_id.to_string())
    }

    pub fn pool(&self) -> kr_resource::Pool {
        kr_resource::Pool::new(self.state_dir.join("resources"))
    }

    pub fn events(&self) -> kr_events::EventLog {
        kr_events::EventLog::new(self.state_dir.join("events"))
    }

    /// The Xcode to run tools with: the project's required one, else `$DEVELOPER_DIR`,
    /// else whatever `xcode-select` chose (`None`).
    pub fn developer_dir(&self, ctx: &Ctx) -> Result<Option<PathBuf>> {
        if let Some(required) = self
            .config
            .project
            .as_ref()
            .and_then(|p| p.project.xcode.as_deref())
        {
            let xcodes = kr_machine::find_xcodes(&ctx.applications_dir());
            let xcode = kr_machine::find_xcode(&xcodes, required).with_context(|| {
                format!("Xcode {required} (required by .kr.toml) is not installed; run `kr doctor`")
            })?;
            return Ok(Some(xcode.developer_dir()));
        }
        Ok(ctx
            .env_var("DEVELOPER_DIR")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from))
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
        if res == Resource::Device {
            return Ok(1);
        }
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
            Resource::Device => Some(1),
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
        Resource::Device => kr_machine::Derived {
            value: 1,
            formula: "one test run per device".to_owned(),
        },
    };
    Capacity {
        value: derived.value,
        source: "suggested".to_owned(),
        formula: Some(derived.formula),
    }
}
