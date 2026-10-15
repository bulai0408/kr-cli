//! `kr run --remote <name>` / `kr gate --remote <name>` (spec phase 2 §6): check, then
//! run the same kr on a declared SSH host and bring its `--json` result back. Events
//! are written on both machines: the host's kr writes its own, this kr writes the
//! items and the end of the run under its own run ID.
//!
//! Also the SSH-session rule: work that needs a developer certificate is refused in an
//! SSH session, where the login keychain's signing identities are not available.

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use kr_config::{ProjectConfig, Remote};
use kr_events::{Event, EventKind, RunId};
use kr_remote::{Checkout, Probe, Problem};
use serde_json::{Value, json};

use super::check::{self, Selection};
use super::human_duration;
use crate::output::Refused;
use crate::{Ctx, Loaded};

const SCHEMA_VERSION: u32 = 1;

/// Environment variables sshd sets in an SSH session.
const SSH_SESSION: &[&str] = &["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"];

/// In an SSH session, refuses the selected items that need a developer certificate.
pub fn refuse_signing_in_ssh_session(
    ctx: &Ctx,
    project: &ProjectConfig,
    sel: &Selection,
) -> Result<()> {
    let Some(var) = SSH_SESSION
        .iter()
        .find(|v| ctx.env_var(v).is_some_and(|x| !x.is_empty()))
    else {
        return Ok(());
    };
    let problems = signing_problems(
        project,
        sel,
        &format!("this is an SSH session ({var} is set)"),
        "run it in a login session on the Mac itself (a local terminal, not SSH), or leave it out of this run",
    )?;
    if problems.is_empty() {
        return Ok(());
    }
    Err(Refused {
        code: "kr::signing::ssh_session",
        what: "refused: work that needs a developer certificate cannot sign in an SSH session"
            .to_owned(),
        problems,
    }
    .into())
}

/// Items that build the app for a device or for macOS: Xcode signs those with a
/// developer certificate from the login keychain, which an SSH session cannot use.
/// Simulator builds and packages built on their own (`dir`) sign without one. A
/// project that turns signing off (`CODE_SIGNING_ALLOWED = "NO"` in
/// `[project].build_settings`) needs no certificate.
fn signing_problems(
    project: &ProjectConfig,
    sel: &Selection,
    why: &str,
    next: &str,
) -> Result<Vec<Problem>> {
    if project
        .project
        .build_settings
        .get("CODE_SIGNING_ALLOWED")
        .is_some_and(|v| v.eq_ignore_ascii_case("NO"))
    {
        return Ok(Vec::new());
    }
    let one = |suites: &[String], scenarios: &[String]| Selection {
        suites: suites.to_vec(),
        scenarios: scenarios.to_vec(),
    };
    let items = sel
        .suites
        .iter()
        .map(|s| (s, one(std::slice::from_ref(s), &[])))
        .chain(
            sel.scenarios
                .iter()
                .map(|s| (s, one(&[], std::slice::from_ref(s)))),
        );
    let mut problems = Vec::new();
    for (name, only) in items {
        let signed: Vec<String> = check::build_jobs(project, &only)?
            .into_iter()
            .filter(|j| j.dir.as_os_str().is_empty() && !j.destination.ends_with(" Simulator"))
            .map(|j| j.destination)
            .collect();
        if !signed.is_empty() {
            problems.push(Problem::new(
                "needs_signing",
                format!(
                    "`{name}` builds the app for {}, which needs a developer certificate; {why}",
                    signed.join(", ")
                ),
                next,
            ));
        }
    }
    Ok(problems)
}

/// Runs `kr <kr_args>` on the remote machine `name` and reports its result as this
/// run's result.
pub fn execute(
    ctx: &Ctx,
    loaded: &Loaded,
    project: &ProjectConfig,
    root: &Path,
    sel: &Selection,
    name: &str,
    kr_args: Vec<String>,
) -> Result<ExitCode> {
    let label = format!("{} on {name}", label_of(&kr_args));
    let Some(remote) = loaded.config.machine.remotes.get(name) else {
        let declared: Vec<&str> = loaded
            .config
            .machine
            .remotes
            .keys()
            .map(String::as_str)
            .collect();
        let file = kr_config::machine_file(&ctx.env).map_or_else(
            || "the machine config".to_owned(),
            |p| p.display().to_string(),
        );
        bail!(
            "no remote machine `{name}` is declared; declared: {}. Declare it in {file}:\n  [remotes.{name}]\n  ssh = \"<host or ~/.ssh/config alias>\"",
            if declared.is_empty() {
                "none".to_owned()
            } else {
                declared.join(", ")
            }
        );
    };
    let run = RemoteRun {
        ctx,
        loaded,
        project: &project.project.name,
        name,
        remote,
        run_id: RunId::new(),
        label,
        items: sel
            .suites
            .iter()
            .map(|s| (s.clone(), "suite"))
            .chain(sel.scenarios.iter().map(|s| (s.clone(), "scenario")))
            .collect(),
        started: Instant::now(),
    };
    let runner = ctx.runner()?;

    let mut problems = signing_problems(
        project,
        sel,
        "a remote run goes through SSH, where the login keychain's signing identities are not available",
        "run it on a Mac directly (without --remote); remote runs can do simulator work",
    )?;
    let (co, git_problems) = kr_remote::checkout(runner.as_ref(), root)?;
    problems.extend(git_problems);
    problems.extend(run.probe(runner.as_ref())?);
    if !problems.is_empty() {
        run.end(
            json!({ "ok": false, "error": "refused", "problems": problems }),
            &co,
        )?;
        return Err(Refused {
            code: "kr::remote::preflight",
            what: format!("cannot run on {}", kr_remote::describe(name, remote)),
            problems,
        }
        .into());
    }

    for (item, kind) in &run.items {
        run.event(
            EventKind::ItemStart,
            json!({ "item": item, "kind": kind, "device": null, "host": name }),
        )?;
    }
    let script = kr_remote::run_script(remote, &project.project.name, &co, &kr_args);
    let log = loaded.run_dir(run.run_id).join("remote.log");
    if !ctx.json {
        eprintln!(
            "{}: {} at {} on {name}; log: {}",
            run.label,
            co.branch,
            co.commit.get(..7).unwrap_or(&co.commit),
            log.display()
        );
    }
    kr_exec::catch_stop_signals()?;
    let result = runner.run_logged(&kr_remote::ssh(remote, &script, true), &log);
    if let Some(signal) = kr_exec::stop_signal() {
        return run.aborted(signal, &co);
    }
    let out = match result {
        Ok(out) => out,
        Err(e) => {
            run.end(json!({ "ok": false, "error": format!("{e:#}") }), &co)?;
            return Err(e);
        }
    };
    let text = std::fs::read_to_string(&log).unwrap_or(out.stdout);
    let tail = kr_remote::last_lines(&text, 3);
    match (kr_remote::json_result(&text), out.status) {
        (Some(result), _) if result.get("error").is_some() => {
            let err = &result["error"];
            let message = err["message"].as_str().unwrap_or("no message");
            run.end(
                json!({ "ok": false, "error": message, "remote_error": err }),
                &co,
            )?;
            let help = err["help"]
                .as_str()
                .map(|h| format!("\n  help: {h}"))
                .unwrap_or_default();
            bail!("kr on {name} failed: {message}{help}");
        }
        (Some(result), status) => run.finish(result, status, &co, &log),
        (None, Some(kr_remote::SSH_LOST)) => run.lost(&tail, &co, &log),
        (None, status) => {
            let status = status.map_or_else(|| "a signal".to_owned(), |s| format!("exit {s}"));
            let what = if text.contains(kr_remote::STARTING_KR) {
                format!("kr on {name} ended without a result ({status})")
            } else {
                format!("preparing the checkout on {name} failed ({status})")
            };
            run.end(json!({ "ok": false, "error": what }), &co)?;
            let next = if text.contains(kr_remote::STARTING_KR) {
                format!("fix the cause on {name} (the log has the whole output), then run again")
            } else {
                format!(
                    "{name} fetches {} itself, in an SSH session where a keychain credential helper does not work: give that host a way to fetch without a prompt (e.g. an SSH key for the git server), then run again",
                    co.url
                )
            };
            bail!(
                "{what}:\n  {}\n  log: {}\n  next: {next}",
                tail.join("\n  "),
                log.display()
            );
        }
    }
}

/// "run" or "gate <name>", as a local run would be labelled.
fn label_of(kr_args: &[String]) -> String {
    match kr_args.first().map(String::as_str) {
        Some("gate") => format!("gate {}", kr_args.get(1).map_or("default", String::as_str)),
        _ => "run".to_owned(),
    }
}

struct RemoteRun<'a> {
    ctx: &'a Ctx,
    loaded: &'a Loaded,
    project: &'a str,
    name: &'a str,
    remote: &'a Remote,
    run_id: RunId,
    label: String,
    /// Selected items and their kind ("suite" / "scenario").
    items: Vec<(String, &'static str)>,
    started: Instant,
}

impl RemoteRun<'_> {
    fn event(&self, kind: EventKind, fields: Value) -> Result<()> {
        self.loaded.events().append(&Event::now(
            self.run_id,
            self.loaded.project(),
            kind,
            fields,
        ))
    }

    fn end(&self, mut fields: Value, co: &Checkout) -> Result<()> {
        fields["label"] = json!(self.label);
        fields["host"] = json!(self.name);
        fields["branch"] = json!(co.branch);
        fields["commit"] = json!(co.commit);
        fields["duration_ms"] = json!(self.started.elapsed().as_millis() as u64);
        self.event(EventKind::RunEnd, fields)
    }

    /// Problems that keep the host from running this kr.
    fn probe(&self, runner: &dyn kr_exec::Runner) -> Result<Vec<Problem>> {
        let cmd = kr_remote::ssh(
            self.remote,
            &kr_remote::probe_script(self.remote.kr()),
            false,
        );
        let out = runner.run(&cmd)?;
        if out.status == Some(kr_remote::SSH_LOST) || !out.success() {
            let why = kr_remote::last_lines(&out.stderr, 1).join("");
            return Ok(vec![Problem::new(
                "unreachable",
                format!(
                    "cannot reach {} over ssh: {why}",
                    kr_remote::describe(self.name, self.remote)
                ),
                format!(
                    "make `ssh {} true` work without a prompt (kr runs ssh with BatchMode=yes: keys or an agent, no passwords; kr does not handle SSH credentials), then run again",
                    self.remote.ssh
                ),
            )]);
        }
        Ok(Probe::parse(&out.stdout).problems(self.name, self.remote, env!("CARGO_PKG_VERSION")))
    }

    /// The host's kr gave its result: record it here and pass it on.
    fn finish(
        &self,
        mut result: Value,
        status: Option<i32>,
        co: &Checkout,
        log: &Path,
    ) -> Result<ExitCode> {
        let items = result["items"].as_array().cloned().unwrap_or_default();
        for item in &items {
            let mut fields = json!({ "item": item["name"], "host": self.name });
            for key in [
                "kind",
                "passed",
                "duration_ms",
                "device",
                "tests",
                "flaky",
                "load",
                "high_load",
                "unverified_reason",
            ] {
                if let Some(v) = item.get(key) {
                    fields[key] = v.clone();
                }
            }
            self.event(EventKind::ItemEnd, fields)?;
        }
        let ok = result["ok"] == true;
        // A failed build is reported as the list of build steps, and nothing ran.
        let failed_step = result["build"]
            .as_array()
            .and_then(|steps| steps.iter().find(|s| s["ok"] == false))
            .filter(|_| !ok)
            .cloned();
        let mut fields = json!({
            "ok": ok,
            "passed": items.iter().filter(|i| i["passed"] == true).count(),
            "total": items.len(),
            "remote_exit": status,
        });
        if failed_step.is_some() {
            fields["build_failed"] = json!(true);
        }
        self.end(fields, co)?;
        let code = match status {
            Some(code @ 0..=255) => ExitCode::from(code as u8),
            _ => ExitCode::FAILURE,
        };
        if self.ctx.json {
            let remote_label = result["label"].clone();
            let obj = result.as_object_mut().expect("json_result returns objects");
            obj.entry("schema_version").or_insert(json!(SCHEMA_VERSION));
            obj.insert("label".into(), json!(self.label));
            obj.insert("host".into(), json!(self.name));
            obj.insert(
                "remote".into(),
                self.remote_json(co, log, status, remote_label),
            );
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(code);
        }
        if let Some(step) = failed_step {
            println!(
                "FAIL: {} · {} failed on {}; nothing ran",
                self.label,
                step["step"].as_str().unwrap_or("build"),
                self.name
            );
            for line in step["summary"].as_array().into_iter().flatten() {
                println!("  {}", line.as_str().unwrap_or_default());
            }
            if let Some(l) = step["log"].as_str() {
                println!("  log on {}: {l}", self.name);
            }
            return Ok(ExitCode::FAILURE);
        }
        self.print_items(&items);
        if let Some(signal) = result["aborted"]["signal"].as_str() {
            println!("ABORTED: {} ({signal} on {})", self.label, self.name);
            return Ok(code);
        }
        println!(
            "{}: {}/{} PASS · {}",
            self.label,
            items.iter().filter(|i| i["passed"] == true).count(),
            items.len(),
            human_duration(self.started.elapsed())
        );
        Ok(code)
    }

    /// The connection to the host was lost: no item has a result, so every item is
    /// unverified. The test count is unknown, so each item counts as one.
    fn lost(&self, tail: &[String], co: &Checkout, log: &Path) -> Result<ExitCode> {
        let why = tail
            .last()
            .cloned()
            .unwrap_or_else(|| "ssh exited with 255".to_owned());
        let reason = format!("lost the connection to {} during the run: {why}", self.name);
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|(item, kind)| {
                json!({
                    "name": item,
                    "kind": kind,
                    "passed": false,
                    "duration_ms": 0,
                    "device": null,
                    "tests": { "unverified": 1 },
                    "unverified_reason": reason,
                    "summary": [reason],
                })
            })
            .collect();
        for item in &items {
            self.event(
                EventKind::ItemEnd,
                json!({
                    "item": item["name"],
                    "kind": item["kind"],
                    "passed": false,
                    "duration_ms": 0,
                    "device": null,
                    "tests": { "unverified": 1 },
                    "unverified_reason": reason,
                    "host": self.name,
                }),
            )?;
        }
        self.end(
            json!({ "ok": false, "passed": 0, "total": items.len(), "host_lost": reason }),
            co,
        )?;
        let next = format!(
            "check that `ssh {} true` works, then run again; the log is {}",
            self.remote.ssh,
            log.display()
        );
        if self.ctx.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": false,
                    "label": self.label,
                    "host": self.name,
                    "passed": 0,
                    "total": items.len(),
                    "items": items,
                    "duration_ms": self.started.elapsed().as_millis() as u64,
                    "host_lost": { "reason": reason, "next": next },
                    "remote": self.remote_json(co, log, Some(kr_remote::SSH_LOST), Value::Null),
                }))?
            );
        } else {
            for (item, _) in &self.items {
                println!("UNVERIFIED: {item} ({reason})");
            }
            println!(
                "{}: 0/{} PASS, host lost · next: {next}",
                self.label,
                items.len()
            );
        }
        Ok(ExitCode::FAILURE)
    }

    /// A stop signal ended the run here; the host's kr gets SIGHUP and stops too.
    fn aborted(&self, signal: i32, co: &Checkout) -> Result<ExitCode> {
        let name = match signal {
            libc::SIGINT => "SIGINT".to_owned(),
            libc::SIGTERM => "SIGTERM".to_owned(),
            libc::SIGHUP => "SIGHUP".to_owned(),
            n => format!("signal {n}"),
        };
        let interrupted: Vec<&String> = self.items.iter().map(|(i, _)| i).collect();
        self.event(
            EventKind::RunAborted,
            json!({
                "signal": name,
                "interrupted": interrupted,
                "not_started": [],
                "finished": [],
                "host": self.name,
                "branch": co.branch,
                "commit": co.commit,
            }),
        )?;
        if self.ctx.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "schema_version": SCHEMA_VERSION,
                    "ok": false,
                    "label": self.label,
                    "host": self.name,
                    "items": [],
                    "aborted": { "signal": name, "interrupted": interrupted, "not_started": [] },
                }))?
            );
        } else {
            let list: Vec<&str> = interrupted.iter().map(|s| s.as_str()).collect();
            println!(
                "ABORTED: {} ({name}) · interrupted: {}",
                self.label,
                list.join(", ")
            );
        }
        Ok(ExitCode::from(u8::try_from(128 + signal).unwrap_or(1)))
    }

    fn remote_json(&self, co: &Checkout, log: &Path, status: Option<i32>, label: Value) -> Value {
        let (_, workdir) = kr_remote::workdir(self.remote, self.project, &co.branch);
        json!({
            "ssh": self.remote.ssh,
            "branch": co.branch,
            "commit": co.commit,
            "workdir": workdir,
            "exit_code": status,
            "label": label,
            "log": log,
        })
    }

    fn print_items(&self, items: &[Value]) {
        for r in items {
            let name = r["name"].as_str().unwrap_or_default();
            let dur = human_duration(Duration::from_millis(
                r["duration_ms"].as_u64().unwrap_or(0),
            ));
            let counts = r["tests"].as_object().map(|t| {
                let n = |k: &str| t.get(k).and_then(Value::as_u64).unwrap_or(0);
                let total: u64 = t.values().filter_map(Value::as_u64).sum();
                let mut parts = vec![format!("{total} tests")];
                for k in ["flaky", "skipped", "unverified"] {
                    if n(k) > 0 {
                        parts.push(format!("{} {k}", n(k)));
                    }
                }
                parts.join(", ")
            });
            let detail = [counts, r["device"].as_str().map(str::to_owned), Some(dur)]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");
            let unverified = r["tests"]["unverified"].as_u64().unwrap_or(0) > 0;
            let verdict = match (r["passed"] == true, unverified) {
                (true, _) => "PASS",
                (false, true) => "UNVERIFIED",
                (false, false) => "FAIL",
            };
            println!("{verdict}: {name} ({detail})");
            for f in r["flaky"].as_array().into_iter().flatten() {
                println!("  flaky: {}", f.as_str().unwrap_or_default());
            }
            if r["passed"] != true {
                for line in r["summary"].as_array().into_iter().flatten().take(3) {
                    println!("  {}", line.as_str().unwrap_or_default());
                }
                if let Some(x) = r["xcresult"].as_str() {
                    println!("  xcresult on {}: {x}", self.name);
                } else if let Some(l) = r["log"].as_str() {
                    println!("  log on {}: {l}", self.name);
                }
            }
        }
    }
}
