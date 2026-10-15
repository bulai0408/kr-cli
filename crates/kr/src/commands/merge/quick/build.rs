use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use kr_config::{Platform, Suite};
use kr_events::{Event, EventKind, RunId};
use kr_exec::Runner;
use kr_resource::Resource;
use serde_json::{Value, json};

use super::{problem, slots};
use crate::Ctx;
use crate::commands::check::{self, BuildJob, Selection};

pub(super) fn run(
    ctx: &Ctx,
    runner: &dyn Runner,
    changed: &BTreeSet<String>,
    problems: &mut Vec<Value>,
) -> Result<Vec<Value>> {
    let loaded = ctx.load()?;
    let Some(project) = &loaded.config.project else {
        problem(
            problems,
            "build_unverified",
            ".kr.toml",
            1,
            "no build configuration for changed source",
            "add build suites or project.scheme to .kr.toml",
        );
        return Ok(Vec::new());
    };
    let source: Vec<&str> = changed
        .iter()
        .filter(|f| super::native_input(f))
        .map(String::as_str)
        .collect();
    let directories: Vec<&Path> = project
        .suites
        .values()
        .filter_map(|s| match s {
            Suite::Build { dir: Some(dir), .. } => Some(dir.as_path()),
            _ => None,
        })
        .collect();
    let root_suite = project
        .suites
        .values()
        .any(|s| matches!(s, Suite::Build { dir: None, .. }));
    let uncovered = !root_suite
        && source
            .iter()
            .any(|f| !directories.iter().any(|d| Path::new(f).starts_with(d)));
    let suites: Vec<String> = project
        .suites
        .iter()
        .filter_map(|(name, s)| match s {
            Suite::Build { dir, .. }
                if dir.as_ref().is_none_or(|d| {
                    uncovered || source.iter().any(|f| Path::new(f).starts_with(d))
                }) =>
            {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    let mut jobs = check::build_jobs(
        project,
        &Selection {
            suites,
            scenarios: Vec::new(),
            on_device: false,
            device: None,
            selection: None,
        },
    )?;
    if uncovered {
        if let Some(scheme) = &project.project.scheme {
            if project.project.platforms.is_empty() {
                problem(
                    problems,
                    "build_unverified",
                    ".kr.toml",
                    1,
                    "the fallback App scheme has no platforms",
                    "add project.platforms or a root build suite that covers this source",
                );
                return Ok(Vec::new());
            }
            for platform in &project.project.platforms {
                let destination = if *platform == Platform::Ios {
                    "generic/platform=iOS Simulator".to_owned()
                } else {
                    format!("generic/platform={platform}")
                };
                let job = BuildJob {
                    dir: PathBuf::new(),
                    action: "build",
                    scheme: scheme.clone(),
                    destination,
                    filters: Vec::new(),
                    scenarios: false,
                };
                if !jobs.iter().any(|j| {
                    j.dir == job.dir && j.scheme == job.scheme && j.destination == job.destination
                }) {
                    jobs.push(job);
                }
            }
        } else if !directories.is_empty() {
            problem(
                problems,
                "build_unverified",
                ".kr.toml",
                1,
                "changed source is outside declared module build directories",
                "add an App scheme and platforms, or a build suite for this source",
            );
            return Ok(Vec::new());
        }
    }
    if jobs.is_empty() {
        problem(
            problems,
            "build_unverified",
            ".kr.toml",
            1,
            "no incremental build matches changed source",
            "add project.scheme and platforms or build suites",
        );
        return Ok(Vec::new());
    }
    let developer_dir = loaded.developer_dir(ctx)?;
    let run = RunId::new();
    let taken = slots::take(ctx, &loaded, runner, run, Resource::Build, "merge-check")?;
    let result = (|| {
        let mut builds = Vec::new();
        let started = Instant::now();
        for (i, job) in jobs.iter().enumerate() {
            let log = loaded.run_dir(run).join(format!("merge-build-{i}.log"));
            let remaining = Duration::from_secs(120).saturating_sub(started.elapsed());
            let cmd = check::with_xcode(job.command(project, &ctx.cwd), developer_dir.as_deref())
                .timeout(remaining);
            let out = runner.run_logged(&cmd, &log)?;
            let ok = out.success() && !out.timed_out;
            loaded.events().append(&Event::now(
                run,
                loaded.project(),
                EventKind::StepEnd,
                json!({
                    "command":"merge check","step":"incremental_build","scheme":job.scheme,
                    "ok":ok,"log":log,"timed_out":out.timed_out,
                }),
            ))?;
            builds.push(json!({"scheme":job.scheme,"dir":job.dir,"destination":job.destination,"ok":ok,"log":log,"timed_out":out.timed_out}));
            if !ok {
                problem(
                    problems,
                    "incremental_build",
                    &log.display().to_string(),
                    1,
                    if out.timed_out {
                        "incremental build exceeded 120 seconds"
                    } else {
                        "incremental build failed"
                    },
                    "read the full build log, fix all affected failures, then rerun merge check",
                );
            }
        }
        Ok(builds)
    })();
    slots::give_back(
        &loaded,
        run,
        taken,
        Some(if result.is_ok() && problems.is_empty() {
            0
        } else {
            1
        }),
    )?;
    result
}
