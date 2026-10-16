//! `kr skill` (ADR 0011 §6): how to use this kr, for the agent that calls it. The text
//! ships with kr, so it always matches the version; a project (`.kr/skill.md`) and the
//! owner (`skill.md` next to the machine config) add to it without a new kr.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Args;
use serde_json::json;

use crate::Ctx;
use crate::output::BadArguments;

#[derive(Debug, Args)]
pub struct SkillArgs {
    /// Print one reference instead of the main text, e.g. `decisions`.
    #[arg(long, value_name = "NAME")]
    reference: Option<String>,
}

const MAIN: &str = include_str!("../../skill/SKILL.md");
/// Read on demand: the rare cases, at length.
const REFERENCES: &[(&str, &str)] = &[
    (
        "decisions",
        include_str!("../../skill/references/decisions.md"),
    ),
    ("json", include_str!("../../skill/references/json.md")),
    ("locks", include_str!("../../skill/references/locks.md")),
];

pub fn run(ctx: &Ctx, args: SkillArgs) -> Result<ExitCode> {
    let names: Vec<&str> = REFERENCES.iter().map(|(n, _)| *n).collect();
    let (name, text, additions) = match &args.reference {
        Some(wanted) => {
            let (name, text) = REFERENCES
                .iter()
                .find(|(n, _)| n == wanted)
                .ok_or_else(|| {
                    BadArguments(format!(
                        "no reference `{wanted}`; references: {}",
                        names.join(", ")
                    ))
                })?;
            (*name, (*text).to_owned(), Vec::new())
        }
        None => (
            "main",
            MAIN.replace("{version}", env!("CARGO_PKG_VERSION"))
                .replace("{references}", &names.join(", ")),
            additions(ctx),
        ),
    };
    if ctx.json {
        return crate::output::print_json(&json!({
            "kr_version": env!("CARGO_PKG_VERSION"),
            "name": name,
            "text": text,
            "references": names,
            "additions": additions
                .iter()
                .map(|(kind, path, text)| json!({ "kind": kind, "path": path, "text": text }))
                .collect::<Vec<_>>(),
        }))
        .map(|()| ExitCode::SUCCESS);
    }
    print!("{text}");
    for (kind, path, text) in &additions {
        println!("\n## Added by the {kind} ({})\n", path.display());
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The project's `.kr/skill.md`, then the owner's `skill.md` next to the machine
/// config, those that exist.
fn additions(ctx: &Ctx) -> Vec<(&'static str, PathBuf, String)> {
    let sources = kr_config::Sources::discover(&ctx.cwd, ctx.env.clone());
    let project = sources
        .project
        .as_deref()
        .and_then(|f| f.parent())
        .map(|root| root.join(".kr/skill.md"));
    let personal = sources
        .machine
        .as_deref()
        .and_then(|f| f.parent())
        .map(|dir| dir.join("skill.md"));
    [("project", project), ("owner", personal)]
        .into_iter()
        .filter_map(|(kind, path)| {
            let path = path?;
            let text = std::fs::read_to_string(&path).ok()?;
            Some((kind, path, text))
        })
        .collect()
}
