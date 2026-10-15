//! Error output: miette diagnostics on stderr for people, one JSON object on stdout with `--json`.

use kr_config::ConfigError;
use serde_json::json;

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
                json!({ "error": {
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
            "code": "kr::error",
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
        println!("{}", json!({ "error": obj }));
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
