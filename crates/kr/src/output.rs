//! Error output: miette diagnostics on stderr for people, one JSON object on stdout with `--json`.

use kr_config::ConfigError;
use serde_json::json;

pub fn report_error(err: &anyhow::Error, json: bool) {
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
