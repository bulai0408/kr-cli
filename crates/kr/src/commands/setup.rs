//! Install the small discovery entry; the versioned instructions stay in `kr skill`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::json;

use crate::Ctx;

const ENTRY: &str = "---\nname: kr\ndescription: Build and test Apple projects that use kr.\n---\n\nFirst run `kr skill` and read the instructions from the installed version.\n";
const MARKER: &str = "<!-- kr -->";
const RULE: &str = "<!-- kr -->\n\nWhen building or testing Apple projects that use kr, use kr for all Apple builds and tests. First run `kr skill` and read its instructions.\n<!-- kr -->";

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Install for Claude Code (default: both agents).
    #[arg(long)]
    claude: bool,
    /// Install for Codex (default: both agents).
    #[arg(long)]
    codex: bool,
    /// Install in the current directory instead of globally.
    #[arg(short, long)]
    project: bool,
    /// Remove the discovery entry and marked rules.
    #[arg(long)]
    remove: bool,
}

struct Edit {
    path: PathBuf,
    before: Option<String>,
    after: Option<String>,
    skill: bool,
}

fn read(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| {
            format!(
                "cannot read {}; check its permissions and encoding",
                path.display()
            )
        }),
    }
}

fn rule_edit(path: PathBuf, remove: bool) -> Result<Edit> {
    let before = read(&path)?;
    let text = before.as_deref().unwrap_or("");
    let markers: Vec<_> = text.match_indices(MARKER).map(|(i, _)| i).collect();
    let after = match markers.as_slice() {
        [] if remove => text.to_owned(),
        [] => format!("{text}{RULE}"),
        [start, end] => format!(
            "{}{}{}",
            &text[..*start],
            if remove { "" } else { RULE },
            &text[end + MARKER.len()..]
        ),
        _ => bail!(
            "unpaired or repeated kr markers in {}; keep one pair of `<!-- kr -->` markers, then run `kr setup` again",
            path.display()
        ),
    };
    Ok(Edit {
        path,
        before,
        after: (!after.is_empty()).then_some(after),
        skill: false,
    })
}

fn skill_edit(path: PathBuf, remove: bool) -> Result<Edit> {
    let before = read(&path)?;
    if before.as_deref().is_some_and(|s| s != ENTRY) {
        bail!(
            "{} contains a different skill; move it aside before running `kr setup`{}",
            path.display(),
            if remove { " --remove" } else { "" }
        );
    }
    Ok(Edit {
        path,
        before,
        after: (!remove).then(|| ENTRY.to_owned()),
        skill: true,
    })
}

fn imports(text: &str, target: &Path, rules: &Path, home: &Path) -> bool {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix('@'))
        .any(|s| {
            let path = if let Some(rest) = s.strip_prefix("~/") {
                home.join(rest)
            } else {
                let p = PathBuf::from(s);
                if p.is_absolute() {
                    p
                } else {
                    rules.parent().unwrap().join(p)
                }
            };
            path == target
                || path
                    .canonicalize()
                    .ok()
                    .zip(target.canonicalize().ok())
                    .is_some_and(|(a, b)| a == b)
        })
}

pub fn run(ctx: &Ctx, args: SetupArgs) -> Result<ExitCode> {
    let home = PathBuf::from(
        ctx.env_var("HOME")
            .context("cannot find HOME; set HOME before running `kr setup`")?,
    );
    let both = !args.claude && !args.codex || args.claude && args.codex;
    let claude = args.claude || both;
    let codex = args.codex || both;
    let base = if args.project { &ctx.cwd } else { &home };
    let codex_home = ctx
        .env_var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let mut codex_rules = if args.project {
        base.join("AGENTS.md")
    } else {
        codex_home.join("AGENTS.md")
    };
    let override_file = codex_rules.with_file_name("AGENTS.override.md");
    if codex && read(&override_file)?.is_some_and(|s| !s.trim().is_empty()) {
        codex_rules = override_file;
    }
    let claude_rules = if args.project {
        base.join("CLAUDE.md")
    } else {
        base.join(".claude/CLAUDE.md")
    };
    let mut edits = Vec::new();
    let mut skipped = Vec::new();
    if codex {
        edits.push(skill_edit(
            base.join(".agents/skills/kr/SKILL.md"),
            args.remove,
        )?);
        edits.push(rule_edit(codex_rules.clone(), args.remove)?);
    }
    if claude {
        edits.push(skill_edit(
            base.join(".claude/skills/kr/SKILL.md"),
            args.remove,
        )?);
        if both
            && !args.remove
            && imports(
                &read(&claude_rules)?.unwrap_or_default(),
                &codex_rules,
                &claude_rules,
                &home,
            )
        {
            skipped.push(
                json!({ "path": claude_rules, "reason": "already imports the Codex rule file" }),
            );
            // Remove a previous duplicate block if this file acquired an import later.
            edits.push(rule_edit(claude_rules, true)?);
        } else {
            edits.push(rule_edit(claude_rules, args.remove)?);
        }
    }
    // Validate every destination before changing any file.
    let mut written = Vec::new();
    let mut removed = Vec::new();
    for edit in edits {
        if edit.before == edit.after {
            continue;
        }
        if let Some(text) = edit.after {
            std::fs::create_dir_all(edit.path.parent().unwrap()).with_context(|| {
                format!(
                    "cannot create the directory for {}; check its permissions",
                    edit.path.display()
                )
            })?;
            std::fs::write(&edit.path, text).with_context(|| {
                format!(
                    "cannot write {}; check its permissions",
                    edit.path.display()
                )
            })?;
            written.push(edit.path);
        } else if edit.before.is_some() {
            std::fs::remove_file(&edit.path).with_context(|| {
                format!(
                    "cannot remove {}; check its permissions",
                    edit.path.display()
                )
            })?;
            if edit.skill {
                // Only remove empty directories belonging to this discovery entry.
                for dir in edit.path.ancestors().skip(1).take(3) {
                    if std::fs::remove_dir(dir).is_err() {
                        break;
                    }
                }
            }
            removed.push(edit.path);
        }
    }
    if ctx.json {
        crate::output::print_json(
            &json!({ "written": written, "removed": removed, "skipped": skipped }),
        )?;
    } else {
        println!(
            "kr setup: {} written, {} removed",
            written.len(),
            removed.len()
        );
        for path in &written {
            println!("  wrote: {}", path.display());
        }
        for path in &removed {
            println!("  removed: {}", path.display());
        }
        for item in skipped {
            println!(
                "  skipped: {} ({})",
                item["path"].as_str().unwrap(),
                item["reason"].as_str().unwrap()
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
