//! Remote machines (spec phase 2 §6): run kr on an SSH host the user declared.
//!
//! kr calls the system `ssh` with the declared destination and nothing else: it does not
//! check the address, and never stores or reads credentials. Before a run, the local
//! branch must be pushed and the host must be a Mac with Xcode and the same kr. The
//! host then fetches the branch with git and runs its own kr, which queues for its own
//! resources; that kr's `--json` output is the result.

use std::path::Path;

use anyhow::Result;
use kr_config::Remote;
use kr_exec::{Command, Runner};
use serde::Serialize;

/// `ssh` exits with this when the connection failed or was lost.
pub const SSH_LOST: i32 = 255;

/// Printed by the run script on the host right before it starts kr.
pub const STARTING_KR: &str = "kr-remote: starting kr";

/// A reason a remote run cannot start, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub code: &'static str,
    pub message: String,
    pub next: String,
}

impl Problem {
    pub fn new(code: &'static str, message: impl Into<String>, next: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            next: next.into(),
        }
    }
}

/// The remote's name for messages, with its SSH destination when that differs:
/// `mini (mini.local)`.
pub fn describe(name: &str, remote: &Remote) -> String {
    if remote.ssh == name {
        name.to_owned()
    } else {
        format!("{name} ({})", remote.ssh)
    }
}

/// Quotes `s` for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c))
    {
        return s.to_owned();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A path for a POSIX shell on the host; a leading `~/` becomes `"$HOME"/`.
pub fn sh_path(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/{}", sh_quote(rest)),
        None if path == "~" => "\"$HOME\"".to_owned(),
        None => sh_quote(path),
    }
}

/// `ssh` running `script` with `sh` on the host. `BatchMode` makes ssh fail instead of
/// asking for a password; the keep-alive ends the call about a minute after the host
/// stops answering. With `tty`, the host's kr gets SIGHUP (and stops cleanly) when the
/// connection goes away.
pub fn ssh(remote: &Remote, script: &str, tty: bool) -> Command {
    let mut cmd = Command::new("ssh").args([
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=4",
    ]);
    if tty {
        cmd = cmd.arg("-tt");
    }
    cmd.arg(remote.ssh.clone())
        .arg(format!("sh -c {}", sh_quote(script)))
}

/// What the host says about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Probe {
    /// `uname -s`, e.g. "Darwin" or "Linux".
    pub os: String,
    /// First line of `xcodebuild -version`; empty without Xcode.
    pub xcode: String,
    /// `kr --version`; empty when kr is not found.
    pub kr: String,
}

/// The script that prints a [`Probe`].
pub fn probe_script(kr: &str) -> String {
    format!(
        "echo \"os=$(uname -s)\"; echo \"xcode=$(xcodebuild -version 2>/dev/null | head -n 1)\"; echo \"kr=$({} --version 2>/dev/null)\"",
        sh_path(kr)
    )
}

impl Probe {
    pub fn parse(stdout: &str) -> Self {
        let mut probe = Self::default();
        for line in stdout.lines() {
            let line = line.trim_end_matches('\r');
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim().to_owned();
                match key {
                    "os" => probe.os = value,
                    "xcode" => probe.xcode = value,
                    "kr" => probe.kr = value,
                    _ => {}
                }
            }
        }
        probe
    }

    /// Why this host cannot run kr `version` (this kr's version).
    pub fn problems(&self, name: &str, remote: &Remote, version: &str) -> Vec<Problem> {
        if self.os != "Darwin" {
            let os = if self.os.is_empty() {
                "an unknown OS"
            } else {
                &self.os
            };
            return vec![Problem::new(
                "not_macos",
                format!(
                    "{} runs {os}, not macOS: remote runs need a Mac with Xcode",
                    describe(name, remote)
                ),
                "run on a Mac: declare one in [remotes] of the machine config (~/.config/kr/config.toml)",
            )];
        }
        let mut problems = Vec::new();
        if self.xcode.is_empty() {
            problems.push(Problem::new(
                "no_xcode",
                format!("{name} is a Mac without Xcode (`xcodebuild -version` failed there)"),
                format!(
                    "install Xcode on {name} and select it with `sudo xcode-select -s /Applications/Xcode.app`, then run `kr doctor` there"
                ),
            ));
        }
        let want = format!("kr {version}");
        if self.kr.is_empty() {
            problems.push(Problem::new(
                "kr_missing",
                format!("kr is not found on {name} (`{} --version` printed nothing)", remote.kr()),
                format!(
                    "install kr {version} on {name}, or set `kr` in [remotes.{name}] of the machine config to its path (an SSH command has a short PATH)"
                ),
            ));
        } else if self.kr != want {
            problems.push(Problem::new(
                "kr_version",
                format!("{name} has {}, this is {want}", self.kr),
                format!("install the same kr on both machines: kr {version} on {name}, or this kr's version here"),
            ));
        }
        problems
    }
}

/// What the host checks out: the pushed branch at the local commit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Checkout {
    /// Where the host fetches from (`git remote get-url origin`).
    pub url: String,
    pub branch: String,
    pub commit: String,
    /// The project root inside the repo (`git rev-parse --show-prefix`), "" at the top.
    pub prefix: String,
}

/// Reads the local branch and checks that the host can fetch exactly this code: a
/// branch is checked out, nothing tracked is uncommitted, origin has the branch at
/// HEAD. Problems found are returned with what was read.
pub fn checkout(runner: &dyn Runner, root: &Path) -> Result<(Checkout, Vec<Problem>)> {
    let git = |args: &[&str]| -> Result<(bool, String)> {
        let out = runner.run(
            &Command::new("git")
                .args(args.iter().copied())
                .current_dir(root),
        )?;
        Ok((out.success(), out.stdout.trim().to_owned()))
    };
    let mut problems = Vec::new();
    let mut co = Checkout::default();
    let (_, branch) = git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    let (_, commit) = git(&["rev-parse", "HEAD"])?;
    co.commit = commit;
    co.prefix = git(&["rev-parse", "--show-prefix"])?.1;
    if branch.is_empty() || branch == "HEAD" {
        problems.push(Problem::new(
            "detached_head",
            "no branch is checked out (detached HEAD); the remote host fetches a pushed branch",
            "check out a branch and push it: `git switch -c <name>` then `git push -u origin <name>`",
        ));
    } else {
        co.branch = branch;
    }
    let (_, dirty) = git(&["status", "--porcelain", "--untracked-files=no"])?;
    if !dirty.is_empty() {
        let files: Vec<&str> = dirty
            .lines()
            .take(3)
            .map(|l| l.get(3..).unwrap_or(l))
            .collect();
        problems.push(Problem::new(
            "uncommitted",
            format!(
                "uncommitted changes would not be tested on the remote host: {}{}",
                files.join(", "),
                if dirty.lines().count() > 3 {
                    ", …"
                } else {
                    ""
                }
            ),
            "commit them and push the branch, or run without --remote",
        ));
    }
    let (has_origin, url) = git(&["remote", "get-url", "origin"])?;
    if !has_origin || url.is_empty() {
        problems.push(Problem::new(
            "no_origin",
            "this repo has no `origin` remote to fetch the branch from",
            "add one: `git remote add origin <url>`, then push the branch",
        ));
        return Ok((co, problems));
    }
    co.url = url;
    if co.branch.is_empty() {
        return Ok((co, problems));
    }
    let (reached, heads) = git(&["ls-remote", "origin", &format!("refs/heads/{}", co.branch)])?;
    let pushed = heads.split_whitespace().next().unwrap_or_default();
    let push = format!("`git push -u origin {}`", co.branch);
    if !reached {
        problems.push(Problem::new(
            "origin_unreachable",
            format!("cannot read branch {} from origin ({})", co.branch, co.url),
            "check that `git ls-remote origin` works here, then run again",
        ));
    } else if pushed.is_empty() {
        problems.push(Problem::new(
            "not_pushed",
            format!(
                "branch {} is not on origin; the remote host fetches it from there",
                co.branch
            ),
            format!("push it: {push}"),
        ));
    } else if pushed != co.commit {
        problems.push(Problem::new(
            "not_pushed",
            format!(
                "origin has {} at {}, HEAD is {}; the remote host would test other code",
                co.branch,
                short(pushed),
                short(&co.commit)
            ),
            format!("push HEAD: {push} (pull first if origin is ahead)"),
        ));
    }
    Ok((co, problems))
}

fn short(commit: &str) -> &str {
    commit.get(..7).unwrap_or(commit)
}

/// The host's working copy of `project` at `branch`: a worktree of one clone per
/// project, so runs of different branches do not move each other's checkout.
pub fn workdir(remote: &Remote, project: &str, branch: &str) -> (String, String) {
    let dir = remote.dir().trim_end_matches('/');
    let base = format!("{dir}/{project}");
    let tree = format!("{dir}/{project}@{}", branch.replace('/', "-"));
    (base, tree)
}

/// The script that brings the host's working copy to `co` and runs kr there with
/// `kr_args` (e.g. `["run", "launch"]`). kr's `--json` result is the last thing printed.
pub fn run_script(remote: &Remote, project: &str, co: &Checkout, kr_args: &[String]) -> String {
    let (base, tree) = workdir(remote, project, &co.branch);
    let (base, tree) = (sh_path(&base), sh_path(&tree));
    let commit = sh_quote(&co.commit);
    let mut lines = vec![
        "set -e".to_owned(),
        // git must fail, not wait at a credential prompt nobody sees.
        "export GIT_TERMINAL_PROMPT=0".to_owned(),
        format!(
            "if [ ! -d {base}/.git ]; then echo 'kr-remote: cloning'; git clone --quiet {} {base}; fi",
            sh_quote(&co.url)
        ),
        "echo 'kr-remote: fetching'".to_owned(),
        format!(
            "git -C {base} fetch --quiet origin {}",
            sh_quote(&co.branch)
        ),
        format!(
            "if [ ! -d {tree} ]; then git -C {base} worktree add --quiet --detach {tree} {commit}; fi"
        ),
        format!("cd {tree}"),
        format!("git checkout --quiet --force --detach {commit}"),
    ];
    if !co.prefix.is_empty() {
        lines.push(format!("cd {}", sh_quote(&co.prefix)));
    }
    lines.push(format!("echo '{STARTING_KR}'"));
    let args: Vec<String> = kr_args.iter().map(|a| sh_quote(a)).collect();
    lines.push(format!(
        "exec {} --json {}",
        sh_path(remote.kr()),
        args.join(" ")
    ));
    lines.join("\n")
}

/// The last top-level JSON object in the output of the host's kr, if any. Lines may
/// end in `\r\n` (the session has a terminal), and with a terminal ssh adds its own
/// "Connection to <host> closed." after it.
pub fn json_result(output: &str) -> Option<serde_json::Value> {
    let text = output.replace("\r\n", "\n");
    let starts: Vec<usize> = text
        .match_indices('\n')
        .map(|(i, _)| i + 1)
        .chain(std::iter::once(0))
        .filter(|&i| text[i..].starts_with('{'))
        .collect();
    let mut starts = starts;
    starts.sort_unstable();
    starts.into_iter().rev().find_map(|i| {
        let mut stream =
            serde_json::Deserializer::from_str(&text[i..]).into_iter::<serde_json::Value>();
        match stream.next() {
            Some(Ok(v)) if v.is_object() && only_ssh_notes(&text[i + stream.byte_offset()..]) => {
                Some(v)
            }
            _ => None,
        }
    })
}

/// Whether `rest` is empty or only what ssh prints when the session ends.
fn only_ssh_notes(rest: &str) -> bool {
    rest.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .all(|l| {
            (l.starts_with("Connection to ") || l.starts_with("Shared connection to "))
                && l.ends_with(" closed.")
        })
}

/// The last lines of `output` that say something, for error messages.
pub fn last_lines(output: &str, n: usize) -> Vec<String> {
    let lines: Vec<String> = output
        .lines()
        .map(|l| l.trim_end_matches('\r').trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(n)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_keeps_simple_words_and_escapes_the_rest() {
        assert_eq!(sh_quote("feature/x-1"), "feature/x-1");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_path("~/kr-remote/Krill"), "\"$HOME\"/kr-remote/Krill");
    }

    #[test]
    fn the_result_is_the_last_json_object_after_other_output() {
        let out = "kr-remote: starting kr\r\nwarn: {not json}\r\n{\r\n  \"ok\": true,\r\n  \"items\": []\r\n}\r\n";
        assert_eq!(
            json_result(out).unwrap(),
            serde_json::json!({ "ok": true, "items": [] })
        );
        assert_eq!(
            json_result("{\"error\":{\"code\":\"x\"}}\n").unwrap()["error"]["code"],
            "x"
        );
        assert!(json_result("kr-remote: starting kr\n{\n  \"ok\": tr").is_none());
    }

    #[test]
    fn the_note_ssh_prints_after_the_result_is_ignored() {
        // Real `ssh -tt localhost` output, 2026-10-15.
        let out = "kr-remote: starting kr\r\n{\r\n  \"ok\": true\r\n}\r\nConnection to localhost closed.\r\n";
        assert_eq!(json_result(out).unwrap()["ok"], true);
        assert!(json_result("{\"ok\": true}\nsomething else\n").is_none());
    }
}
