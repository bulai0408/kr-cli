use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use serde_json::json;

use super::checked_git;
use crate::Ctx;

pub(super) fn run(ctx: &Ctx) -> Result<ExitCode> {
    let runner = ctx.runner()?;
    let root = PathBuf::from(checked_git(ctx, &*runner, &["rev-parse", "--show-toplevel"])?.trim());
    let list = checked_git(ctx, &*runner, &["worktree", "list", "--porcelain", "-z"])?;
    let worktrees: Vec<&str> = list
        .split('\0')
        .filter_map(|s| s.strip_prefix("worktree "))
        .collect();
    let attrs = root.join(".gitattributes");
    if attrs.is_symlink() {
        bail!(".gitattributes is a symlink; replace it with a regular file before installing");
    }
    let original = if attrs.exists() {
        std::fs::read_to_string(&attrs)?
    } else {
        String::new()
    };
    let executable = std::env::current_exe()?.display().to_string();
    let quoted = format!("'{}'", executable.replace('\'', "'\\''"));
    let driver = format!("{quoted} merge xcstrings %O %A %B");
    for worktree in &worktrees {
        checked_git(
            ctx,
            &*runner,
            &[
                "-C",
                worktree,
                "config",
                "--worktree",
                "rerere.enabled",
                "true",
            ],
        )?;
        checked_git(
            ctx,
            &*runner,
            &[
                "-C",
                worktree,
                "config",
                "--worktree",
                "merge.kr-xcstrings.name",
                "kr catalog key merge",
            ],
        )?;
        checked_git(
            ctx,
            &*runner,
            &[
                "-C",
                worktree,
                "config",
                "--worktree",
                "merge.kr-xcstrings.driver",
                &driver,
            ],
        )?;
        // A virtual merge ancestor must not use an arbitrary semantic resolution.
        checked_git(
            ctx,
            &*runner,
            &[
                "-C",
                worktree,
                "config",
                "--worktree",
                "merge.kr-xcstrings.recursive",
                "binary",
            ],
        )?;
    }
    let rule = "*.xcstrings merge=kr-xcstrings";
    let changed = original.lines().last() != Some(rule);
    if changed {
        let newline = if original.is_empty() || original.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::write(&attrs, format!("{original}{newline}{rule}\n"))?;
    }
    super::report(
        ctx,
        json!({"ok":true,"worktrees":worktrees,"rerere":true,
        "tracked_file_changed":if changed {Some(".gitattributes")} else {None},
        "next":"review and commit .gitattributes; rerun install in new worktrees with overrides"}),
        &format!(
            "Enabled rerere and the catalog driver in {} worktree(s). {} Review and commit .gitattributes.",
            worktrees.len(),
            if changed {
                "Changed tracked file .gitattributes."
            } else {
                "Tracked file .gitattributes already has the rule."
            }
        ),
        true,
    )
}
