//! The commit a task queued on, and whether the worktree had uncommitted changes (#67).

use kr_exec::{Command, Runner};
use serde_json::{Value, json};

/// `{commit, dirty}` of the repository around the working directory; null outside one,
/// before its first commit, or when git fails. Untracked files count as changes.
pub fn state(runner: &dyn Runner) -> Value {
    // No optional index refresh: parallel agents must not contend for index.lock.
    let status = Command::new("git")
        .args(["status", "--porcelain=v2", "--branch"])
        .env("GIT_OPTIONAL_LOCKS", "0");
    let Some(out) = runner.run(&status).ok().filter(|o| o.success()) else {
        return Value::Null;
    };
    match out
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("# branch.oid "))
    {
        Some(commit) if commit != "(initial)" => json!({
            "commit": commit,
            "dirty": out.stdout.lines().any(|l| !l.starts_with('#')),
        }),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kr_exec::ReplayRunner;

    fn replay(name: &str) -> ReplayRunner {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/git")
            .join(name);
        ReplayRunner::from_files([path]).unwrap()
    }

    #[test]
    fn a_clean_tree_has_its_commit_and_is_not_dirty() {
        assert_eq!(
            state(&replay("status-clean.json")),
            json!({ "commit": "c9131b9dd4a85e4941f8462fa5ae22ecd5b31a29", "dirty": false })
        );
    }

    #[test]
    fn an_untracked_file_makes_the_tree_dirty() {
        assert_eq!(
            state(&replay("status-dirty.json")),
            json!({ "commit": "c9131b9dd4a85e4941f8462fa5ae22ecd5b31a29", "dirty": true })
        );
    }

    #[test]
    fn outside_a_repository_there_is_nothing_to_record() {
        assert_eq!(state(&replay("status-outside.json")), Value::Null);
    }
}
