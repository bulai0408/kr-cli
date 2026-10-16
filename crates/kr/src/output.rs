//! Error output: miette diagnostics on stderr for people, one JSON object on stdout with `--json`.

use kr_config::ConfigError;
use serde::Serialize;
use serde_json::json;

/// Version of every `--json` document kr prints. Fields are only added, never changed
/// or removed, so it stays 1 (ADR 0011 supplement).
pub const SCHEMA_VERSION: u32 = 1;

/// Exit code for bad arguments, as clap uses it.
pub const BAD_ARGUMENTS: u8 = 2;
/// Exit code of `kr check`, `kr run` and `kr gate` (and `kr decide`) when the run has no
/// verdict yet: its decision points wait for the calling agent (ADR 0011).
pub const DECISIONS_PENDING: u8 = 4;

/// Prints a `--json` document. An object gets `schema_version` and `decisions` (the
/// decision points; empty for most commands) unless it has them already.
pub fn print_json(value: &impl Serialize) -> anyhow::Result<()> {
    let mut value = serde_json::to_value(value)?;
    if let Some(obj) = value.as_object_mut() {
        obj.entry("schema_version").or_insert(json!(SCHEMA_VERSION));
        obj.entry("decisions").or_insert(json!([]));
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// An argument kr could not use, found after parsing (e.g. an unknown decision ID):
/// exits with [`BAD_ARGUMENTS`].
#[derive(Debug)]
pub struct BadArguments(pub String);

impl std::fmt::Display for BadArguments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BadArguments {}

/// kr refused to start: every reason it found, each with what to do.
#[derive(Debug)]
pub struct Refused {
    /// e.g. "kr::remote::preflight".
    pub code: &'static str,
    /// What could not start, e.g. "cannot run on mini".
    pub what: String,
    pub problems: Vec<kr_remote::Problem>,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.what)?;
        for p in &self.problems {
            write!(f, "\n  - {}\n    next: {}", p.message, p.next)?;
        }
        Ok(())
    }
}

impl std::error::Error for Refused {}

pub fn report_error(err: &anyhow::Error, json: bool) {
    if let Some(refused) = err.downcast_ref::<Refused>() {
        if json {
            println!(
                "{}",
                json!({ "schema_version": SCHEMA_VERSION, "error": {
                    "code": refused.code,
                    "message": refused.what,
                    "problems": refused.problems,
                }})
            );
        } else {
            eprintln!("error: {refused}");
        }
        return;
    }
    let config = err.downcast_ref::<ConfigError>();
    if json {
        let mut obj = json!({
            "code": if err.downcast_ref::<BadArguments>().is_some() { "kr::bad_arguments" } else { "kr::error" },
            "message": format!("{err:#}"),
        });
        if let Some(config) = config {
            obj["code"] = json!(config_code(config));
            obj["message"] = json!(config.to_string());
            if let Some(help) = miette::Diagnostic::help(config) {
                obj["help"] = json!(help.to_string());
            }
            if let ConfigError::File(file) = config {
                obj["file"] = json!(file.path);
                obj["line"] = json!(file.line);
                obj["column"] = json!(file.column);
            }
        }
        println!(
            "{}",
            json!({ "schema_version": SCHEMA_VERSION, "error": obj })
        );
        return;
    }
    match config {
        Some(config) => {
            let mut out = String::new();
            let handler = miette::GraphicalReportHandler::new_themed(theme());
            if handler.render_report(&mut out, config).is_ok() {
                eprint!("{out}");
            } else {
                eprintln!("error: {config}");
            }
        }
        None => eprintln!("error: {err:#}"),
    }
}

fn config_code(err: &ConfigError) -> String {
    miette::Diagnostic::code(err).map_or_else(|| "kr::config".to_owned(), |c| c.to_string())
}

fn theme() -> miette::GraphicalTheme {
    if std::env::var_os("NO_COLOR").is_some() {
        miette::GraphicalTheme::unicode_nocolor()
    } else {
        miette::GraphicalTheme::unicode()
    }
}
