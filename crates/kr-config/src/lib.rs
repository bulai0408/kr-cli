//! Layered configuration (ADR 0001).
//!
//! Layers, later wins: machine `~/.config/kr/config.toml` → project `.kr.toml` →
//! `KR_*` environment variables → command-line overrides.
//!
//! Each TOML file is first parsed on its own with `toml`, so errors point at a file,
//! line and column. figment then merges the layers.

use std::path::{Path, PathBuf};

use figment::value::{Dict, Map, Value};
use figment::{Figment, Metadata, Profile, Provider};
use miette::{Diagnostic, NamedSource, SourceSpan};
use serde::{Deserialize, Serialize};

/// The project configuration file name, at the repo root.
pub const PROJECT_FILE: &str = ".kr.toml";

/// `KR_*` variables that are not configuration keys.
const RESERVED_ENV: &[&str] = &["KR_LOG", "KR_CONFIG", "KR_HELD"];

/// The merged configuration.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Where kr keeps events and other state. Default: `~/.local/state/kr`.
    pub state_dir: Option<PathBuf>,
    /// Manual resource capacities. Unset values come from the machine profile.
    pub resources: Resources,
    pub project: Option<Project>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Resources {
    pub build: Option<u32>,
    pub sim: Option<u32>,
    pub light: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
}

impl Config {
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
        for path in [&self.machine, &self.project].into_iter().flatten() {
            if let Some(text) = read_optional(path)? {
                check_file(path, &text)?;
                figment = figment.merge(Layer::new(path.display().to_string(), toml_dict(&text)));
            }
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
        figment.extract().map_err(ConfigError::from_figment)
    }
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

fn check_file(path: &Path, text: &str) -> Result<(), ConfigError> {
    let Err(err) = toml::from_str::<Config>(text) else {
        return Ok(());
    };
    let span = err.span().unwrap_or(0..0);
    let (line, column) = line_column(text, span.start);
    Err(ConfigError::File(Box::new(FileError {
        path: path.to_owned(),
        line,
        column,
        message: err.message().to_owned(),
        src: NamedSource::new(path.display().to_string(), text.to_owned()),
        span: span.into(),
    })))
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
#[error("{}:{line}:{column}: {message}", path.display())]
#[diagnostic(code(kr::config::file), help("fix the marked item in {}", path.display()))]
pub struct FileError {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub message: String,
    #[source_code]
    src: NamedSource<String>,
    #[label("{message}")]
    span: SourceSpan,
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
