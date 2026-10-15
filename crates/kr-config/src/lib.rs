//! Configuration (ADR 0001).
//!
//! Machine settings come in layers, later wins: `~/.config/kr/config.toml` →
//! `KR_*` environment variables → command-line overrides (figment).
//! Project settings come from `.kr.toml` at the repo root. The two files have
//! separate keys: capacities belong to the machine, not to a repo.
//!
//! Each TOML file is parsed on its own with `toml`, so errors point at a file,
//! line and column.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use figment::value::{Dict, Map, Value};
use figment::{Figment, Metadata, Profile, Provider};
use miette::{Diagnostic, NamedSource, SourceSpan};
use serde::{Deserialize, Serialize};

/// The project configuration file name, at the repo root.
pub const PROJECT_FILE: &str = ".kr.toml";

/// `KR_*` variables that are not configuration keys.
const RESERVED_ENV: &[&str] = &[
    "KR_LOG",
    "KR_CONFIG",
    "KR_HELD",
    "KR_REPLAY",
    "KR_RECORD",
    "KR_APPLICATIONS",
];

mod project;
mod span;

pub use project::*;

/// Keys that only the machine file may set.
const MACHINE_KEYS: &[&str] = &[
    "state_dir",
    "resources",
    "machines",
    "load",
    "legacy",
    "remotes",
];

/// The loaded configuration: machine layers merged, plus the project file if any.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Config {
    pub machine: MachineConfig,
    pub project: Option<ProjectConfig>,
}

/// Machine-level settings: `~/.config/kr/config.toml`, `KR_*` variables, overrides.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MachineConfig {
    /// Where kr keeps events and other state. Default: `~/.local/state/kr`.
    pub state_dir: Option<PathBuf>,
    /// Manual resource capacities. Unset values come from the machine profile.
    pub resources: Resources,
    /// Per-machine settings, keyed by LocalHostName (`scutil --get LocalHostName`).
    pub machines: BTreeMap<String, MachineOverride>,
    /// Load check before heavy work.
    pub load: LoadCheck,
    /// Bridge to the old machine-wide lock (ADR 0009). Off unless set.
    pub legacy: Legacy,
    /// SSH hosts `kr run --remote <name>` may run on, keyed by that name. kr connects
    /// only to these, with the system `ssh`.
    pub remotes: BTreeMap<String, Remote>,
}

/// A remote machine: an SSH host the user declared. kr does not check the address and
/// never stores or reads credentials; `ssh` and its own configuration handle both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    /// What `ssh` connects to, passed as is: a host name or a `Host` alias of
    /// `~/.ssh/config`.
    pub ssh: String,
    /// Where kr keeps project clones on that host. Default: `~/kr-remote`.
    pub dir: Option<String>,
    /// The kr program on that host. Default: `kr` (found on the PATH of an SSH command).
    pub kr: Option<String>,
}

impl Remote {
    pub fn dir(&self) -> &str {
        self.dir.as_deref().unwrap_or("~/kr-remote")
    }

    pub fn kr(&self) -> &str {
        self.kr.as_deref().unwrap_or("kr")
    }
}

/// The old `mkdir` lock other projects and scripts still use. When both paths are set,
/// kr also holds this lock while it holds a `build` or `sim` slot.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Legacy {
    /// e.g. "/tmp/native-build.lock"
    pub machine_lock: Option<PathBuf>,
    /// e.g. "/tmp/native-build.queue"
    pub queue: Option<PathBuf>,
    /// Retry interval while waiting. Default "30s", as in the old protocol.
    pub poll: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoadCheck {
    /// Also wait while the 1-minute load is above `per_core` × cores. Off by default:
    /// macOS load counts processes waiting for I/O, so it does not say how much CPU
    /// is left (ADR 0003). kr always waits while memory pressure is critical.
    pub block: bool,
    /// "High load" is a 1-minute load above `per_core` × logical cores. Failures then
    /// are marked as high-load failures and the item is run once more.
    pub per_core: f64,
    /// Give up waiting after this long and go ahead, e.g. "20m".
    pub max_wait: String,
}

impl Default for LoadCheck {
    fn default() -> Self {
        Self {
            block: false,
            per_core: 4.0,
            max_wait: "20m".to_owned(),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MachineOverride {
    pub resources: Resources,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Resources {
    pub build: Option<u32>,
    pub sim: Option<u32>,
    pub light: Option<u32>,
}

impl MachineConfig {
    /// The state directory: `state_dir`, else `$XDG_STATE_HOME/kr`, else `$HOME/.local/state/kr`.
    pub fn state_dir(&self, env: &[(String, String)]) -> Option<PathBuf> {
        if let Some(dir) = &self.state_dir {
            return Some(dir.clone());
        }
        if let Some(xdg) = var(env, "XDG_STATE_HOME") {
            return Some(Path::new(xdg).join("kr"));
        }
        var(env, "HOME").map(|home| Path::new(home).join(".local/state/kr"))
    }

    /// Capacities set for this machine: `[machines.<host>.resources]` over `[resources]`.
    pub fn resources_for(&self, host: Option<&str>) -> Resources {
        let general = &self.resources;
        let Some(specific) = host.and_then(|h| self.machines.get(h)) else {
            return general.clone();
        };
        let r = &specific.resources;
        Resources {
            build: r.build.or(general.build),
            sim: r.sim.or(general.sim),
            light: r.light.or(general.light),
        }
    }
}

/// Where each layer comes from.
#[derive(Debug, Default, Clone)]
pub struct Sources {
    /// Machine file. A missing file is skipped.
    pub machine: Option<PathBuf>,
    /// Project file. A missing file is skipped.
    pub project: Option<PathBuf>,
    /// Environment variables; only `KR_*` ones are used. `KR_A__B=1` sets `a.b = 1`.
    pub env: Vec<(String, String)>,
    /// Command-line overrides as `(dotted.key, value)`.
    pub overrides: Vec<(String, String)>,
}

impl Sources {
    /// Finds the layers for a command started in `cwd` with environment `env`.
    pub fn discover(cwd: &Path, env: Vec<(String, String)>) -> Self {
        Self {
            machine: machine_file(&env),
            project: find_project_file(cwd),
            env,
            overrides: Vec::new(),
        }
    }

    pub fn load(&self) -> Result<Config, ConfigError> {
        let mut figment = Figment::new();
        if let Some(path) = &self.machine
            && let Some(text) = read_optional(path)?
        {
            check_file::<MachineConfig>(path, &text)?;
            figment = figment.merge(Layer::new(path.display().to_string(), toml_dict(&text)));
        }
        for (key, value) in &self.env {
            if !key.starts_with("KR_") || RESERVED_ENV.contains(&key.as_str()) {
                continue;
            }
            let path = key["KR_".len()..].to_lowercase().replace("__", ".");
            figment = figment.merge(Layer::value(
                format!("environment variable {key}"),
                &path,
                value,
            ));
        }
        for (key, value) in &self.overrides {
            figment = figment.merge(Layer::value(format!("override {key}"), key, value));
        }
        let machine = figment.extract().map_err(ConfigError::from_figment)?;

        let project = match &self.project {
            Some(path) => match read_optional(path)? {
                Some(text) => Some(load_project(path, &text)?),
                None => None,
            },
            None => None,
        };
        Ok(Config { machine, project })
    }
}

/// Line and column (1-based) where `path` (and array element `index`) is written in
/// a TOML file, for pointing users at the item to fix.
pub fn locate(file: &Path, path: &[&str], index: Option<usize>) -> Option<(usize, usize)> {
    let text = std::fs::read_to_string(file).ok()?;
    let doc = toml::de::DeTable::parse(&text).ok()?;
    let span = span::find(&doc, path, index)?;
    Some(line_column(&text, span.start))
}

fn load_project(path: &Path, text: &str) -> Result<ProjectConfig, ConfigError> {
    let doc = toml::de::DeTable::parse(text).map_err(|e| toml_error(path, text, &e))?;
    for (key, _) in doc.get_ref().iter() {
        if MACHINE_KEYS.contains(&&**key.get_ref()) {
            return Err(file_error(
                path,
                text,
                key.span(),
                format!("`{}` is a machine setting", key.get_ref()),
                format!(
                    "move it to the machine config (~/.config/kr/config.toml); {} is shared by every machine",
                    PROJECT_FILE
                ),
            ));
        }
    }
    let config: ProjectConfig = check_file(path, text)?;
    let problems = config.problems();
    if let Some(first) = problems.first() {
        let segs: Vec<&str> = first.path.iter().map(String::as_str).collect();
        let span = span::find(&doc, &segs, first.index).unwrap_or(0..0);
        let mut err = file_error(path, text, span, first.message.clone(), first.help.clone());
        if let ConfigError::File(file) = &mut err {
            file.others = problems.len() - 1;
        }
        return Err(err);
    }
    Ok(config)
}

/// The machine file: `$KR_CONFIG`, else `$XDG_CONFIG_HOME/kr/config.toml`, else
/// `$HOME/.config/kr/config.toml`.
pub fn machine_file(env: &[(String, String)]) -> Option<PathBuf> {
    if let Some(path) = var(env, "KR_CONFIG") {
        return Some(PathBuf::from(path));
    }
    if let Some(xdg) = var(env, "XDG_CONFIG_HOME") {
        return Some(Path::new(xdg).join("kr/config.toml"));
    }
    var(env, "HOME").map(|home| Path::new(home).join(".config/kr/config.toml"))
}

/// Looks for `.kr.toml` from `start` upward, stopping at the repo root (the first
/// directory that contains `.git`).
pub fn find_project_file(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join(PROJECT_FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").exists() {
            return None;
        }
    }
    None
}

fn var<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
    env.iter()
        .rev()
        .find(|(k, v)| k == key && !v.is_empty())
        .map(|(_, v)| v.as_str())
}

fn read_optional(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Read {
            path: path.to_owned(),
            source,
        }),
    }
}

fn check_file<T: serde::de::DeserializeOwned>(path: &Path, text: &str) -> Result<T, ConfigError> {
    toml::from_str::<T>(text).map_err(|e| toml_error(path, text, &e))
}

fn toml_error(path: &Path, text: &str, err: &toml::de::Error) -> ConfigError {
    file_error(
        path,
        text,
        err.span().unwrap_or(0..0),
        err.message().to_owned(),
        format!("fix the marked item in {}", path.display()),
    )
}

fn file_error(
    path: &Path,
    text: &str,
    span: std::ops::Range<usize>,
    message: String,
    help: String,
) -> ConfigError {
    let (line, column) = line_column(text, span.start);
    ConfigError::File(Box::new(FileError {
        path: path.to_owned(),
        line,
        column,
        message,
        help,
        others: 0,
        src: NamedSource::new(path.display().to_string(), text.to_owned()),
        span: span.into(),
    }))
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    (line, column)
}

fn toml_dict(text: &str) -> Dict {
    // Already validated by `check_file`.
    toml::from_str(text).unwrap_or_default()
}

/// One configuration layer with a human-readable origin for error messages.
struct Layer {
    name: String,
    dict: Dict,
}

impl Layer {
    fn new(name: String, dict: Dict) -> Self {
        Self { name, dict }
    }

    fn value(name: String, key: &str, raw: &str) -> Self {
        let value: Value = raw.parse().unwrap_or_else(|_| Value::from(raw.to_owned()));
        let dict = match figment::util::nest(key, value) {
            Value::Dict(_, dict) => dict,
            _ => Dict::new(),
        };
        Self { name, dict }
    }
}

impl Provider for Layer {
    fn metadata(&self) -> Metadata {
        Metadata::named(self.name.clone())
    }

    fn data(&self) -> Result<Map<Profile, Dict>, figment::Error> {
        Ok(Profile::Default.collect(self.dict.clone()))
    }
}

/// A configuration error that says which file or variable is wrong and how to fix it.
#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum ConfigError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    File(Box<FileError>),

    #[error("invalid value for `{key}` from {origin}: {message}")]
    #[diagnostic(code(kr::config::value), help("fix or unset {origin}"))]
    Value {
        key: String,
        origin: String,
        message: String,
    },

    #[error("failed to read {}", path.display())]
    #[diagnostic(code(kr::config::read))]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A TOML file that does not parse or does not match the schema.
#[derive(Debug, thiserror::Error, Diagnostic)]
#[error("{}:{line}:{column}: {message}{}", path.display(), more(*others))]
#[diagnostic(code(kr::config::file))]
pub struct FileError {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub message: String,
    #[help]
    pub help: String,
    /// How many more problems the file has after this one.
    pub others: usize,
    #[source_code]
    src: NamedSource<String>,
    #[label("{message}")]
    span: SourceSpan,
}

fn more(others: usize) -> String {
    match others {
        0 => String::new(),
        n => format!(" (and {n} more problem(s); fix this one and run again)"),
    }
}

impl ConfigError {
    fn from_figment(err: figment::Error) -> Self {
        let key = err.path.join(".");
        let origin = err
            .metadata
            .as_ref()
            .map_or_else(|| "configuration".to_owned(), |m| m.name.to_string());
        Self::Value {
            key,
            origin,
            message: err.kind.to_string(),
        }
    }
}
