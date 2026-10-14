//! `kr check`: the pre-check before the simulator queue (spec §5, ADR 0005). Holds
//! only a `build` slot: generate the project, compile with the app's own settings,
//! confirm every test filter names real tests, check data isolation.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Args;
use kr_config::{Destination, Platform, ProjectConfig, Suite};
use kr_events::{Event, EventKind, RunId};
use kr_exec::{Command, Runner};
use kr_resource::Resource;
use kr_xcode::{TestSymbols, XcTestRun, Xcodebuild, error_summary};
use serde::Serialize;
use serde_json::json;

use super::human_duration;
use crate::{Ctx, Loaded, slots};

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Scenarios or suites to check. Default: the `default` gate, else everything.
    targets: Vec<String>,

    /// Check the scenarios and suites of this gate.
    #[arg(long, conflicts_with = "targets")]
    gate: Option<String>,
}

/// Where kr builds, relative to the directory xcodebuild runs in.
pub const DERIVED_DATA: &str = "build/kr/DerivedData";
const GENERATE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// What to check.
pub struct Selection {
    pub scenarios: Vec<String>,
    pub suites: Vec<String>,
}

pub fn select(
    project: &ProjectConfig,
    targets: &[String],
    gate: Option<&str>,
) -> Result<Selection> {
    if !targets.is_empty() {
        let mut sel = Selection {
            scenarios: Vec::new(),
            suites: Vec::new(),
        };
        for t in targets {
            if project.scenarios.contains_key(t) {
                sel.scenarios.push(t.clone());
            } else if project.suites.contains_key(t) {
                sel.suites.push(t.clone());
            } else {
                let known: Vec<&str> = project
                    .scenarios
                    .keys()
                    .chain(project.suites.keys())
                    .map(String::as_str)
                    .collect();
                bail!(
                    "`{t}` is neither a scenario nor a suite in .kr.toml; known: {}",
                    known.join(", ")
                );
            }
        }
        return Ok(sel);
    }
    let gate_name = gate.unwrap_or("default");
    match project.gates.get(gate_name) {
        Some(g) => Ok(Selection {
            scenarios: project.gate_scenarios(g),
            suites: g.suites.clone(),
        }),
        None if gate.is_some() => {
            let known: Vec<&str> = project.gates.keys().map(String::as_str).collect();
            bail!(
                "no gate `{gate_name}` in .kr.toml; known: {}",
                known.join(", ")
            )
        }
        None => Ok(Selection {
            scenarios: project.scenarios.keys().cloned().collect(),
            suites: project.suites.keys().cloned().collect(),
        }),
    }
}

/// One xcodebuild invocation, shared by every scenario or suite that needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildJob {
    /// Working directory relative to the repo root; empty for the root.
    pub dir: PathBuf,
    pub action: &'static str,
    pub scheme: String,
    pub destination: String,
    /// Filters to confirm against the built tests, with where they are written.
    pub filters: Vec<Filter>,
    /// Holds the app the scenarios drive (isolation is checked on it).
    pub scenarios: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub test: String,
    /// Key path in .kr.toml, e.g. ["scenarios", "ratings", "only"], and the index.
    pub path: Vec<String>,
    pub index: usize,
}

/// "iOS 18.6" → the generic simulator destination of its platform.
pub fn simulator_destination(runtime: &str) -> String {
    let platform = runtime.split_whitespace().next().unwrap_or("iOS");
    format!("generic/platform={platform} Simulator")
}

fn runtime_of<'a>(project: &'a ProjectConfig, kind: Option<&str>) -> Option<&'a str> {
    let kind = project
        .simulators
        .devices
        .get(kind.unwrap_or(kr_config::DEFAULT_DEVICE))?;
    kind.runtime
        .as_deref()
        .or(project.simulators.runtime.as_deref())
}

fn filters(kind: &str, name: &str, only: &[String], skip: &[String]) -> Vec<Filter> {
    let mut out: Vec<Filter> = only
        .iter()
        .enumerate()
        .map(|(i, t)| Filter {
            test: t.clone(),
            path: vec![kind.to_owned(), name.to_owned(), "only".to_owned()],
            index: i,
        })
        .collect();
    out.extend(skip.iter().enumerate().map(|(i, t)| Filter {
        test: t.clone(),
        path: vec![kind.to_owned(), name.to_owned(), "skip".to_owned()],
        index: i,
    }));
    out
}

/// The builds needed for `sel`, merged when they are the same invocation.
pub fn build_jobs(project: &ProjectConfig, sel: &Selection) -> Result<Vec<BuildJob>> {
    let mut jobs: Vec<BuildJob> = Vec::new();
    let add = |job: BuildJob, jobs: &mut Vec<BuildJob>| match jobs.iter_mut().find(|j| {
        j.dir == job.dir
            && j.action == job.action
            && j.scheme == job.scheme
            && j.destination == job.destination
    }) {
        Some(existing) => {
            existing.filters.extend(job.filters);
            existing.scenarios |= job.scenarios;
        }
        None => jobs.push(job),
    };
    for name in &sel.scenarios {
        let scenario = &project.scenarios[name];
        let Some(scheme) = &project.project.scheme else {
            bail!("scenarios need [project].scheme in .kr.toml");
        };
        let runtime = runtime_of(project, scenario.device.as_deref()).unwrap_or("iOS");
        add(
            BuildJob {
                dir: PathBuf::new(),
                action: "build-for-testing",
                scheme: scheme.clone(),
                destination: simulator_destination(runtime),
                filters: filters("scenarios", name, &scenario.only, &scenario.skip),
                scenarios: true,
            },
            &mut jobs,
        );
    }
    for name in &sel.suites {
        match &project.suites[name] {
            Suite::Build {
                dir,
                scheme,
                platforms,
                ..
            } => {
                let platforms = if platforms.is_empty() {
                    &project.project.platforms
                } else {
                    platforms
                };
                for p in platforms {
                    add(
                        BuildJob {
                            dir: dir.clone().unwrap_or_default(),
                            action: "build",
                            scheme: scheme.clone(),
                            destination: format!("generic/platform={}", p.as_str()),
                            filters: Vec::new(),
                            scenarios: false,
                        },
                        &mut jobs,
                    );
                }
            }
            Suite::Test {
                dir,
                scheme,
                only,
                skip,
                destination,
                device,
                ..
            } => {
                let destination = match destination {
                    Destination::Simulator => simulator_destination(
                        runtime_of(project, device.as_deref()).unwrap_or("iOS"),
                    ),
                    Destination::Macos => format!("generic/platform={}", Platform::Macos.as_str()),
                };
                add(
                    BuildJob {
                        dir: dir.clone().unwrap_or_default(),
                        action: "build-for-testing",
                        scheme: scheme.clone(),
                        destination,
                        filters: filters("suites", name, only, skip),
                        scenarios: false,
                    },
                    &mut jobs,
                );
            }
            Suite::Command { .. } => {}
        }
    }
    Ok(jobs)
}

impl BuildJob {
    /// The xcodebuild command; the project file, configuration and build settings of
    /// [project] apply only to jobs at the repo root.
    pub fn command(&self, project: &ProjectConfig, root: &Path) -> Command {
        let info = &project.project;
        let at_root = self.dir.as_os_str().is_empty();
        let mut xb = Xcodebuild::new(self.action)
            .scheme(&self.scheme)
            .destination(&self.destination)
            .derived_data(DERIVED_DATA);
        if at_root {
            xb = xb
                .project(info.xcodeproj.clone(), info.workspace.clone())
                .configuration(info.configuration.as_deref())
                .settings(info.build_settings.clone());
        }
        xb.command().current_dir(root.join(&self.dir))
    }

    pub fn products(&self, root: &Path) -> PathBuf {
        root.join(&self.dir)
            .join(DERIVED_DATA)
            .join("Build/Products")
    }

    fn label(&self) -> String {
        let dir = if self.dir.as_os_str().is_empty() {
            String::new()
        } else {
            format!("{}: ", self.dir.display())
        };
        format!(
            "{dir}{} {} ({})",
            self.action, self.scheme, self.destination
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StepResult {
    pub step: String,
    pub ok: bool,
    pub duration_ms: u64,
    /// At most three lines on failure.
    pub summary: Vec<String>,
    pub log: Option<PathBuf>,
}

/// Records each step as an event and keeps the results for the report.
pub struct Steps<'a> {
    pub loaded: &'a Loaded,
    pub run_id: RunId,
    pub command: &'static str,
    pub results: Vec<StepResult>,
}

impl Steps<'_> {
    pub fn finish(
        &mut self,
        step: &str,
        started: Instant,
        ok: bool,
        summary: Vec<String>,
        log: Option<PathBuf>,
    ) -> Result<bool> {
        let duration_ms = started.elapsed().as_millis() as u64;
        self.loaded.events().append(&Event::now(
            self.run_id,
            self.loaded.project(),
            EventKind::StepEnd,
            json!({ "command": self.command, "step": step, "ok": ok, "duration_ms": duration_ms }),
        ))?;
        self.results.push(StepResult {
            step: step.to_owned(),
            ok,
            duration_ms,
            summary,
            log,
        });
        Ok(ok)
    }

    /// Runs a long command into `<run dir>/<step>.log` and records the step.
    pub fn run_logged(&mut self, runner: &dyn Runner, step: &str, cmd: &Command) -> Result<bool> {
        let started = Instant::now();
        let log = self
            .loaded
            .run_dir(self.run_id)
            .join(format!("{}.log", file_name(step)));
        let out = runner.run_logged(cmd, &log)?;
        let ok = out.success();
        let summary = if ok {
            Vec::new()
        } else if out.timed_out {
            vec![format!(
                "timed out after {}",
                human_duration(cmd.timeout.unwrap_or_default())
            )]
        } else {
            error_summary(&format!("{}{}", out.stdout, out.stderr))
        };
        self.finish(step, started, ok, summary, Some(log))
    }
}

fn file_name(step: &str) -> String {
    step.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn run(ctx: &Ctx, args: CheckArgs) -> Result<ExitCode> {
    let loaded = ctx.load()?;
    let (project, root) = loaded.require_project()?;
    let sel = select(project, &args.targets, args.gate.as_deref())?;
    let jobs = build_jobs(project, &sel)?;
    let runner = ctx.runner()?;
    let run_id = RunId::new();
    let developer_dir = loaded.developer_dir(ctx)?;
    let task = format!("check {}", describe(&sel));
    let taken = slots::take(
        ctx,
        &loaded,
        runner.as_ref(),
        run_id,
        Resource::Build,
        &task,
    )?;
    let mut steps = Steps {
        loaded: &loaded,
        run_id,
        command: "check",
        results: Vec::new(),
    };
    let result = check(
        project,
        &root,
        &jobs,
        runner.as_ref(),
        developer_dir.as_deref(),
        &loaded,
        &mut steps,
    );
    let ok = matches!(result, Ok(true));
    slots::give_back(&loaded, run_id, taken, Some(if ok { 0 } else { 1 }))?;
    result?;
    report(ctx, &steps.results, ok)?;
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn describe(sel: &Selection) -> String {
    let mut names: Vec<&str> = sel
        .scenarios
        .iter()
        .chain(&sel.suites)
        .map(String::as_str)
        .collect();
    if names.len() > 3 {
        let more = names.len() - 3;
        names.truncate(3);
        return format!("{} +{more}", names.join(","));
    }
    names.join(",")
}

fn with_xcode(cmd: Command, developer_dir: Option<&Path>) -> Command {
    match developer_dir {
        Some(dir) => cmd.env("DEVELOPER_DIR", dir.display().to_string()),
        None => cmd,
    }
}

/// The check steps; returns whether all passed. Stops at the first failing step.
pub fn check(
    project: &ProjectConfig,
    root: &Path,
    jobs: &[BuildJob],
    runner: &dyn Runner,
    developer_dir: Option<&Path>,
    loaded: &Loaded,
    steps: &mut Steps<'_>,
) -> Result<bool> {
    if let Some(generate) = &project.project.generate {
        let cmd = Command::new("sh")
            .args(["-c", generate.as_str()])
            .current_dir(root)
            .timeout(GENERATE_TIMEOUT);
        if !steps.run_logged(runner, "generate", &cmd)? {
            return Ok(false);
        }
    }
    for job in jobs {
        let cmd = with_xcode(job.command(project, root), developer_dir).timeout(BUILD_TIMEOUT);
        if !steps.run_logged(runner, &format!("build {}", job.label()), &cmd)? {
            return Ok(false);
        }
    }
    let project_file = loaded.project_file.as_deref();
    let started = Instant::now();
    let mut problems = Vec::new();
    for job in jobs.iter().filter(|j| !j.filters.is_empty()) {
        problems.extend(check_filters(
            job,
            root,
            runner,
            developer_dir,
            project_file,
        )?);
    }
    if !steps.finish(
        "filters",
        started,
        problems.is_empty(),
        first_three(&problems),
        None,
    )? {
        return Ok(false);
    }
    let iso = &project.isolation;
    if iso.bundle_ids.is_empty() && iso.entitlements.is_empty() {
        return Ok(true);
    }
    let started = Instant::now();
    let mut problems = Vec::new();
    for job in jobs.iter().filter(|j| j.scenarios) {
        problems.extend(check_isolation(
            project,
            job,
            root,
            runner,
            developer_dir,
            project_file,
        )?);
    }
    steps.finish(
        "isolation",
        started,
        problems.is_empty(),
        first_three(&problems),
        None,
    )
}

fn first_three(problems: &[String]) -> Vec<String> {
    problems.iter().take(3).cloned().collect()
}

fn at(project_file: Option<&Path>, path: &[String], index: Option<usize>) -> String {
    let Some(file) = project_file else {
        return String::new();
    };
    let segs: Vec<&str> = path.iter().map(String::as_str).collect();
    match kr_config::locate(file, &segs, index) {
        Some((line, col)) => format!(" ({}:{line}:{col})", file.display()),
        None => format!(" ({})", file.display()),
    }
}

fn check_filters(
    job: &BuildJob,
    root: &Path,
    runner: &dyn Runner,
    developer_dir: Option<&Path>,
    project_file: Option<&Path>,
) -> Result<Vec<String>> {
    let products = job.products(root);
    let run = XcTestRun::read(&XcTestRun::find(&products)?)?;
    let mut symbols = TestSymbols::default();
    let mut problems = Vec::new();
    let wanted: std::collections::BTreeSet<&str> = job
        .filters
        .iter()
        .map(|f| f.test.split('/').next().unwrap_or(""))
        .collect();
    for target in run
        .targets
        .iter()
        .filter(|t| wanted.contains(t.name.as_str()))
    {
        let binary = target
            .bundle
            .join(target.bundle.file_stem().unwrap_or_default());
        let cmd = with_xcode(TestSymbols::command(&binary), developer_dir).current_dir(&products);
        let out = runner.run(&cmd)?;
        if !out.success() {
            problems.push(format!(
                "cannot read the symbols of {}: {}",
                target.bundle.display(),
                out.stderr.trim()
            ));
            continue;
        }
        symbols.add(&target.module, &out.stdout);
    }
    for f in &job.filters {
        let target = f.test.split('/').next().unwrap_or("");
        let module = run
            .target(target)
            .map_or(target.to_owned(), |t| t.module.clone());
        let as_module = f.test.replacen(target, &module, 1);
        let where_ = at(project_file, &f.path, Some(f.index));
        if run.target(target).is_none() {
            let known: Vec<&str> = run.targets.iter().map(|t| t.name.as_str()).collect();
            problems.push(format!(
                "`{}` names test target `{target}`, which scheme {} does not build (it has {}){where_}",
                f.test,
                job.scheme,
                known.join(", ")
            ));
        } else if !symbols.matches(&as_module) {
            problems.push(format!("`{}` matches no test{where_}", f.test));
        }
    }
    Ok(problems)
}

fn check_isolation(
    project: &ProjectConfig,
    job: &BuildJob,
    root: &Path,
    runner: &dyn Runner,
    developer_dir: Option<&Path>,
    project_file: Option<&Path>,
) -> Result<Vec<String>> {
    let iso = &project.isolation;
    let products = job.products(root);
    let run = XcTestRun::read(&XcTestRun::find(&products)?)?;
    let mut apps: Vec<&Path> = run
        .targets
        .iter()
        .filter_map(|t| t.app.as_deref())
        .collect();
    apps.sort();
    apps.dedup();
    let mut problems = Vec::new();
    for app in apps {
        let bundle_id = kr_xcode::bundle_identifier(&products.join(app))?;
        if !iso.bundle_ids.is_empty() && !iso.bundle_ids.contains(&bundle_id) {
            problems.push(format!(
                "{} has bundle id `{bundle_id}`, not one of the synthetic ids {:?}; tests could touch real user data{}",
                app.display(),
                iso.bundle_ids,
                at(project_file, &["isolation".into(), "bundle_ids".into()], None)
            ));
        }
        if iso.entitlements.is_empty() {
            continue;
        }
        let exe = kr_xcode::executable(&products.join(app))?;
        let exe = exe.strip_prefix(&products).unwrap_or(&exe).to_owned();
        let out = runner.run(
            &with_xcode(kr_xcode::entitlements_command(&exe), developer_dir).current_dir(&products),
        )?;
        let entitlements = kr_xcode::parse_entitlements_dump(&out.stdout)?;
        for (key, allowed) in &iso.entitlements {
            let Some(value) = entitlements.get(key) else {
                continue;
            };
            let values: Vec<String> = match value {
                plist::Value::String(s) => vec![s.clone()],
                plist::Value::Array(a) => a
                    .iter()
                    .filter_map(|v| v.as_string().map(str::to_owned))
                    .collect(),
                other => vec![format!("{other:?}")],
            };
            for v in values.iter().filter(|v| !allowed.contains(v)) {
                problems.push(format!(
                    "{} has entitlement {key} = `{v}`, allowed: {allowed:?}{}",
                    app.display(),
                    at(
                        project_file,
                        &["isolation".into(), "entitlements".into(), key.clone()],
                        None
                    )
                ));
            }
        }
    }
    Ok(problems)
}

fn report(ctx: &Ctx, steps: &[StepResult], ok: bool) -> Result<()> {
    if ctx.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "ok": ok, "steps": steps }))?
        );
        return Ok(());
    }
    let times: Vec<String> = steps
        .iter()
        .map(|s| {
            format!(
                "{} {}",
                short(&s.step),
                human_duration(Duration::from_millis(s.duration_ms))
            )
        })
        .collect();
    if ok {
        println!("PASS: check · {}", times.join(" · "));
        return Ok(());
    }
    let failed = steps.iter().find(|s| !s.ok).expect("a step failed");
    println!(
        "FAIL: check · {} failed after {}",
        failed.step,
        human_duration(Duration::from_millis(failed.duration_ms))
    );
    for line in &failed.summary {
        println!("  {line}");
    }
    if let Some(log) = &failed.log {
        println!("  log: {}", log.display());
    }
    Ok(())
}

fn short(step: &str) -> &str {
    step.split_whitespace().next().unwrap_or(step)
}
