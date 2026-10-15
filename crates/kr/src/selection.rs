//! Deterministic scenario selection (ADR 0019). Unknown impact adds work, never
//! removes a scenario proved necessary by the reverse dependency closure.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use kr_config::ProjectConfig;
use kr_exec::{Command, Runner};
use kr_run::decision::{Affects, Choice, Decision, Suggestion};
use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::check::Selection;
use crate::{Ctx, Loaded};

#[derive(Debug, Default, Args)]
pub struct SelectionArgs {
    /// Select scenarios affected by changes from this git ref to the working tree,
    /// including staged, unstaged, deleted and untracked files.
    #[arg(long, value_name = "REF")]
    pub changed: Option<String>,
    /// Add a scenario; required scenarios cannot be removed (repeatable).
    #[arg(long, value_name = "SCENARIO")]
    pub also: Vec<String>,
    /// Show why each scenario runs and which changes were explicitly ignored.
    #[arg(long)]
    pub explain: bool,
}

impl SelectionArgs {
    pub fn flags(&self) -> Vec<String> {
        let mut flags = Vec::new();
        if let Some(base) = &self.changed {
            flags.extend(["--changed".to_owned(), base.clone()]);
        }
        for name in &self.also {
            flags.extend(["--also".to_owned(), name.clone()]);
        }
        if self.explain {
            flags.push("--explain".to_owned());
        }
        flags
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SelectionReport {
    pub changed_files: Vec<String>,
    pub ignored: BTreeMap<String, String>,
    pub affected_modules: BTreeMap<String, Vec<String>>,
    pub required: Vec<String>,
    pub reasons: BTreeMap<String, Vec<String>>,
    pub unknown: Vec<String>,
    pub decisions: Vec<Decision>,
    #[serde(skip)]
    pub explain: bool,
}

#[derive(Default)]
struct Node {
    paths: BTreeSet<PathBuf>,
    dependencies: BTreeSet<String>,
}
#[derive(Default)]
struct Graph {
    nodes: BTreeMap<String, Node>,
    unknown: Vec<String>,
}

fn output(runner: &dyn Runner, cmd: Command) -> Result<String> {
    let out = runner
        .run(&cmd)
        .with_context(|| format!("cannot read `{cmd}`"))?;
    if !out.success() {
        bail!("`{cmd}` failed: {}", out.stderr.trim());
    }
    Ok(out.stdout)
}

fn paths(text: &str) -> Vec<String> {
    text.split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Graph {
    fn load(
        project: &ProjectConfig,
        root: &Path,
        files: &[String],
        runner: &dyn Runner,
        state: &Path,
    ) -> Self {
        let mut g = Self::default();
        let mut products: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut product_dependencies = BTreeMap::<String, Vec<String>>::new();
        for file in files.iter().filter(|f| {
            Path::new(f)
                .file_name()
                .is_some_and(|n| n == "Package.swift")
        }) {
            let dir = Path::new(file).parent().unwrap_or(Path::new(""));
            let package_root = root.join(dir);
            let key = blake3::hash(package_root.to_string_lossy().as_bytes()).to_hex();
            let scratch = state.join("package-descriptions").join(key.as_str());
            let cmd = Command::new("swift")
                .args([
                    "package",
                    "--scratch-path",
                    &scratch.display().to_string(),
                    "describe",
                    "--type",
                    "json",
                ])
                .current_dir(package_root);
            match output(runner, cmd).and_then(|s| Ok(serde_json::from_str::<Value>(&s)?)) {
                Ok(v) => {
                    if !v["targets"].is_array() {
                        g.unknown
                            .push(format!("{file}: package description has no targets array"));
                    }
                    for t in v["targets"].as_array().into_iter().flatten() {
                        let (Some(name), Some(path)) = (t["name"].as_str(), t["path"].as_str())
                        else {
                            g.unknown
                                .push(format!("{file}: target has no name or path"));
                            continue;
                        };
                        let n = g.nodes.entry(name.to_owned()).or_default();
                        n.paths.insert(dir.join(path));
                        n.dependencies.extend(strings(&t["target_dependencies"]));
                        product_dependencies
                            .entry(name.to_owned())
                            .or_default()
                            .extend(strings(&t["product_dependencies"]));
                        for product in strings(&t["product_memberships"]) {
                            products.entry(product).or_default().insert(name.to_owned());
                        }
                    }
                }
                Err(e) => g
                    .unknown
                    .push(format!("{file}: {e:#}; repair the package description")),
            }
        }
        for (name, dependencies) in product_dependencies {
            for product in dependencies {
                if let Some(targets) = products.get(&product) {
                    g.nodes
                        .entry(name.clone())
                        .or_default()
                        .dependencies
                        .extend(targets.iter().cloned());
                }
            }
        }
        if files.iter().any(|p| p == "project.yml") {
            let cmd = Command::new("xcodegen")
                .args(["dump", "--type", "json", "--no-env"])
                .current_dir(root);
            match output(runner, cmd).and_then(|s| Ok(serde_json::from_str::<Value>(&s)?)) {
                Ok(v) => {
                    if let Some(targets) = v["targets"].as_object() {
                        for (name, t) in targets {
                            let n = g.nodes.entry(name.clone()).or_default();
                            for s in t["sources"].as_array().into_iter().flatten() {
                                if let Some(path) = s.as_str().or_else(|| s["path"].as_str()) {
                                    n.paths.insert(path.into());
                                }
                            }
                            for d in t["dependencies"].as_array().into_iter().flatten() {
                                if let Some(target) = d["target"].as_str() {
                                    n.dependencies.insert(target.to_owned());
                                }
                                if let Some(product) =
                                    d["product"].as_str().or_else(|| d["package"].as_str())
                                    && let Some(targets) = products.get(product)
                                {
                                    n.dependencies.extend(targets.iter().cloned());
                                    // An external product has no sources in this project.
                                }
                            }
                        }
                    } else {
                        g.unknown
                            .push("project.yml: no targets in resolved spec".to_owned());
                    }
                }
                Err(e) => g.unknown.push(format!(
                    "project.yml: {e:#}; repair the resolved project spec"
                )),
            }
        } else if project.project.xcodeproj.is_some() || project.project.workspace.is_some() {
            g.unknown.push("no project.yml: declare app/page modules in .kr.toml; Xcode project dependencies are unknown".to_owned());
        }
        for (name, node) in &g.nodes {
            for dependency in &node.dependencies {
                if !g.nodes.contains_key(dependency) && !project.modules.contains_key(dependency) {
                    g.unknown.push(format!(
                        "{name}: dependency `{dependency}` was not described"
                    ));
                }
            }
        }
        for (name, module) in &project.modules {
            let n = g.nodes.entry(name.clone()).or_default();
            n.paths.extend(module.paths.iter().cloned());
            n.dependencies.extend(module.depends_on.iter().cloned());
        }
        g
    }

    fn affected(&self, file: &str) -> BTreeMap<String, Vec<String>> {
        let path = Path::new(file);
        let mut affected: BTreeMap<String, Vec<String>> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.paths.iter().any(|p| path.starts_with(p)))
            .map(|(name, _)| (name.clone(), vec![file.to_owned(), name.clone()]))
            .collect();
        loop {
            let mut added = Vec::new();
            for (name, n) in &self.nodes {
                if !affected.contains_key(name)
                    && let Some(mut chain) =
                        n.dependencies.iter().find_map(|d| affected.get(d).cloned())
                {
                    chain.push(name.clone());
                    added.push((name.clone(), chain));
                }
            }
            if added.is_empty() {
                break;
            }
            affected.extend(added);
        }
        affected
    }
}

fn strings(v: &Value) -> impl Iterator<Item = String> + '_ {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
}

pub fn apply(
    ctx: &Ctx,
    loaded: &Loaded,
    sel: &mut Selection,
    explicit: &[String],
    args: &SelectionArgs,
) -> Result<()> {
    let (project, root) = loaded.require_project()?;
    for name in &args.also {
        if !project.scenarios.contains_key(name) {
            bail!(".kr.toml: --also `{name}` is not a scenario; use a name from [scenarios]");
        }
    }
    let Some(base) = &args.changed else {
        for name in &args.also {
            if !sel.scenarios.contains(name) {
                sel.scenarios.push(name.clone());
            }
        }
        if args.explain {
            sel.selection = Some(SelectionReport {
                explain: true,
                reasons: sel
                    .scenarios
                    .iter()
                    .map(|name| {
                        let why = if explicit.contains(name) || args.also.contains(name) {
                            "selected explicitly on the command line"
                        } else {
                            "selected by the gate"
                        };
                        (name.clone(), vec![why.to_owned()])
                    })
                    .collect(),
                ..Default::default()
            });
        }
        return Ok(());
    };
    let runner = ctx.runner()?;
    let files = paths(&output(
        runner.as_ref(),
        Command::new("git")
            .args(["ls-files", "-co", "--exclude-standard", "-z"])
            .current_dir(&root),
    )?);
    let mut changed = paths(&output(
        runner.as_ref(),
        Command::new("git")
            .args(["diff", "--name-only", "-z", "--no-renames", base, "--"])
            .current_dir(&root),
    )?);
    changed.extend(paths(&output(
        runner.as_ref(),
        Command::new("git")
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .current_dir(&root),
    )?));
    changed.sort();
    changed.dedup();
    let mut report = SelectionReport {
        changed_files: changed.clone(),
        explain: args.explain,
        ..Default::default()
    };
    let graph = Graph::load(project, &root, &files, runner.as_ref(), &loaded.state_dir);
    report.unknown.extend(graph.unknown.iter().cloned());
    for (name, module) in &project.modules {
        for dependency in &module.depends_on {
            if !graph.nodes.contains_key(dependency) && graph.unknown.is_empty() {
                bail!(
                    "{}: modules.{name}.depends_on: unknown module `{dependency}`; use a discovered module name or add [modules.{dependency}] with paths and depends_on",
                    root.join(".kr.toml").display()
                );
            }
        }
    }
    for (name, scenario) in &project.scenarios {
        for m in &scenario.modules {
            if !graph.nodes.contains_key(m) && graph.unknown.is_empty() {
                bail!(
                    "{}: scenarios.{name}.modules: unknown module `{m}`; use a SwiftPM/XcodeGen target name or add [modules.{m}] with paths and depends_on",
                    root.join(".kr.toml").display()
                );
            }
        }
    }
    let ignores: Vec<_> = project
        .selection
        .ignore
        .iter()
        .map(|p| {
            Ok((
                p,
                globset::GlobBuilder::new(p)
                    .literal_separator(true)
                    .build()?
                    .compile_matcher(),
            ))
        })
        .collect::<Result<_>>()?;
    for file in &changed {
        if let Some((pattern, _)) = ignores.iter().find(|(_, matcher)| matcher.is_match(file)) {
            report
                .ignored
                .insert(file.clone(), format!("ignored by {pattern}"));
            continue;
        }
        let affected = graph.affected(file);
        if affected.is_empty() {
            report
                .unknown
                .push(format!("{file}: no module owns this changed path"));
        } else if !project
            .scenarios
            .values()
            .any(|s| s.modules.iter().any(|m| affected.contains_key(m)))
        {
            report.unknown.push(format!(
                "{file}: affected modules have no declared scenario coverage"
            ));
        }
        for (m, chain) in affected {
            report.affected_modules.entry(m).or_insert(chain);
        }
    }
    // No non-ignored changes means there is no unknown impact to assess.
    if changed.iter().all(|p| report.ignored.contains_key(p)) {
        report.unknown.clear();
    }
    for (name, scenario) in &project.scenarios {
        for m in &scenario.modules {
            if let Some(chain) = report.affected_modules.get(m) {
                report
                    .reasons
                    .entry(name.clone())
                    .or_default()
                    .push(format!("required: {} -> {name}", chain.join(" -> ")));
            }
        }
        if scenario.modules.is_empty() && changed.iter().any(|p| !report.ignored.contains_key(p)) {
            report
                .unknown
                .push(format!("scenarios.{name}.modules: coverage not declared"));
        }
    }
    report.required = report.reasons.keys().cloned().collect();
    sel.scenarios = report.required.clone();
    for name in explicit
        .iter()
        .chain(&args.also)
        .filter(|n| project.scenarios.contains_key(*n))
    {
        report
            .reasons
            .entry(name.clone())
            .or_default()
            .push("explicitly added on the command line".to_owned());
        if !sel.scenarios.contains(name) {
            sel.scenarios.push(name.clone());
        }
    }
    if !report.unknown.is_empty() {
        let candidates: Vec<_> = project
            .scenarios
            .keys()
            .filter(|n| !sel.scenarios.contains(n))
            .cloned()
            .collect();
        if !candidates.is_empty() {
            let decision = Decision {
                id: "selection-1".to_owned(),
                kind: "additional_scenarios".to_owned(),
                affects: Affects::Efficiency,
                question: "Run additional scenarios because the change impact is uncertain?"
                    .to_owned(),
                item: "selection".to_owned(),
                test: None,
                options: vec![
                    Choice {
                        name: "add".to_owned(),
                        means: "run all candidate scenarios".to_owned(),
                    },
                    Choice {
                        name: "keep".to_owned(),
                        means: "keep only the required and explicit scenarios".to_owned(),
                    },
                ],
                evidence: json!({"unknown":report.unknown, "candidates":candidates, "history":crate::selection_history::read(loaded)?}),
                default: Suggestion {
                    option: "add".to_owned(),
                    reason: "unknown impact: run all candidates; pending review".to_owned(),
                },
            };
            for name in candidates {
                report.reasons.entry(name.clone()).or_default().push(
                    "default additional scenario: unknown impact (pending review)".to_owned(),
                );
                sel.scenarios.push(name);
            }
            report.decisions.push(decision);
        }
    }
    sel.scenarios.sort();
    sel.scenarios.dedup();
    sel.selection = Some(report);
    Ok(())
}
