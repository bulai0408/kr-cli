use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use kr_config::ProjectConfig;
use kr_exec::{Command, Runner};
use kr_run::decision::{Affects, Choice, Decision, Suggestion};
use serde_json::{Value, json};

use crate::Ctx;
use crate::output::BadArguments;

pub fn run(ctx: &Ctx) -> Result<ExitCode> {
    let path = ctx.cwd.join(kr_config::PROJECT_FILE);
    if path.symlink_metadata().is_ok() {
        return Err(BadArguments(
            ".kr.toml already exists; edit it with `kr skill --reference config`; init never overwrites it"
                .into(),
        )
        .into());
    }
    let runner = ctx.runner()?;
    let mut decisions = Vec::new();
    let metadata = discover(&ctx.cwd, runner.as_ref(), &mut decisions)?;
    let mut config = json!({
        "project": {
            "name": metadata.name,
            "scheme": metadata.scheme,
            "configuration": metadata.configuration,
            "platforms": [metadata.platform],
        },
        "gates": { "default": { "suites": [], "scenarios": [] } },
    });
    if let Some((key, value)) = &metadata.source {
        config["project"][key] = json!(value);
    }
    if let Some(generate) = &metadata.generate {
        config["project"]["generate"] = json!(generate);
    }
    let unit = metadata.tests("bundle.unit-test");
    let ui = metadata.tests("bundle.ui-testing");
    if metadata.package {
        config["suites"] = json!({
            "unit": { "type": "command", "run": if unit.is_empty() { "swift build" } else { "swift test" } },
        });
        config["gates"]["default"]["suites"] = json!(["unit"]);
    } else if !unit.is_empty() {
        config["suites"] = json!({
            "unit": {
                "type": "test", "scheme": metadata.scheme, "only": unit,
                "destination": if metadata.platform == "macOS" { "macos" } else { "simulator" },
            },
        });
        config["gates"]["default"]["suites"] = json!(["unit"]);
    }
    if let Some(first) = ui.first() {
        if ui.len() > 1 {
            question(
                &mut decisions,
                "ui_target",
                if metadata.platform == "macOS" {
                    "suites.smoke.only"
                } else {
                    "scenarios.smoke.only"
                },
                "Which UI test target should smoke cover?",
                &ui,
                first,
                json!({ "targets": ui, "scheme": metadata.scheme }),
            );
        }
        if metadata.platform == "macOS" {
            config["suites"]["smoke"] = json!({
                "type": "test", "scheme": metadata.scheme,
                "only": [first], "destination": "macos",
            });
            config["gates"]["default"]["suites"]
                .as_array_mut()
                .unwrap()
                .push(json!("smoke"));
        } else {
            config["scenarios"] = json!({ "smoke": { "only": [first] } });
            config["gates"]["default"]["scenarios"] = json!(["smoke"]);
        }
        question(
            &mut decisions,
            "isolation",
            "isolation",
            "Which synthetic bundle IDs and entitlements may UI tests use?",
            &["configure_isolation".into()],
            "configure_isolation",
            json!({ "file": ".kr.toml", "ui_targets": ui, "isolation": "not inferred from production settings" }),
        );
    } else {
        question(
            &mut decisions,
            "ui_target",
            "scenarios",
            "No UI tests found; add a UI test target or keep suite-only verification?",
            &["suite_only".into(), "add_ui_tests".into()],
            "suite_only",
            json!({
                "scheme": metadata.scheme,
                "targets": metadata.targets.as_array().into_iter().flatten()
                    .filter_map(|t| t["target"].as_str()).collect::<Vec<_>>(),
            }),
        );
    }
    if unit.is_empty() && ui.is_empty() && !metadata.package {
        config["suites"] = json!({
            "build": { "type": "build", "scheme": metadata.scheme },
        });
        config["gates"]["default"]["suites"] = json!(["build"]);
    }
    if metadata.platform != "macOS" {
        let text = read(
            runner.as_ref(),
            Command::new("xcrun").args(["simctl", "list", "runtimes", "-j"]),
            &ctx.cwd,
        )?;
        let runtime = kr_machine::parse_runtimes(&text)?
            .into_iter()
            .filter(|r| r.is_available && r.name.starts_with(&format!("{} ", metadata.platform)))
            .filter(|r| metadata.deployment.as_ref().is_none_or(|minimum| version(&r.version) >= version(minimum)))
            .filter(|r| !r.supported_device_types.is_empty())
            .max_by_key(|r| version(&r.version))
            .context("no compatible simulator runtime installed; install one in Xcode Settings > Components, then run `kr init` again")?;
        let device = runtime
            .supported_device_types
            .iter()
            .find(|d| d.name.starts_with("iPhone"))
            .or_else(|| runtime.supported_device_types.first())
            .expect("filtered nonempty device types");
        config["simulators"] = json!({
            "runtime": runtime.name,
            "devices": { "default": { "device_type": device.name, "count": 1 } },
        });
    }
    let parsed: ProjectConfig = serde_json::from_value(config.clone())?;
    let problems = parsed.problems();
    if !problems.is_empty() {
        bail!(
            "cannot generate .kr.toml: {}; see `kr skill --reference config`",
            problems[0].message
        );
    }
    let text = format!(
        "# Generated by kr init. Review decisions before running tests.\n\
         # Pool devices: {}-Verify-<N>. Configure isolation before UI tests.\n\
         # All configuration: kr skill --reference config\n\n{}",
        parsed.project.name,
        toml::to_string_pretty(&config)?,
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| "cannot create .kr.toml; init never overwrites an existing file")?;
    file.write_all(text.as_bytes())?;
    kr_config::Sources {
        project: Some(path),
        ..Default::default()
    }
    .load()?;
    if ctx.json {
        crate::output::print_json(&json!({
            "ok": true, "written": [".kr.toml"], "decisions": decisions,
        }))?;
    } else {
        println!("Created .kr.toml; configuration valid.");
        for d in &decisions {
            println!(
                "Please confirm: {} (default: {})",
                d.question, d.default.option
            );
        }
        println!(
            "Next: review .kr.toml with `kr skill --reference config`, then run `kr doctor` and `kr check`."
        );
    }
    Ok(ExitCode::SUCCESS)
}

struct Metadata {
    name: String,
    source: Option<(&'static str, String)>,
    scheme: String,
    configuration: String,
    platform: String,
    targets: Value,
    generate: Option<String>,
    package: bool,
    deployment: Option<String>,
}

impl Metadata {
    fn tests(&self, product: &str) -> Vec<String> {
        self.targets
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| {
                t["buildSettings"]["PRODUCT_TYPE"]
                    .as_str()
                    .is_some_and(|s| s.ends_with(product))
            })
            .filter_map(|t| t["target"].as_str().map(str::to_owned))
            .collect()
    }
}

fn discover(root: &Path, runner: &dyn Runner, decisions: &mut Vec<Decision>) -> Result<Metadata> {
    if root.join("project.yml").is_file() {
        return xcodegen(root, runner, decisions);
    }
    let mut projects = std::fs::read_dir(root)?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.ends_with(".xcworkspace") || name.ends_with(".xcodeproj"))
        .collect::<Vec<_>>();
    projects.sort_by_key(|s| (!s.ends_with(".xcworkspace"), s.clone()));
    if projects.is_empty() && root.join("Package.swift").is_file() {
        let spec = read_json(
            runner,
            Command::new("swift").args(["package", "dump-package"]),
            root,
        )?;
        let name = spec["name"]
            .as_str()
            .context("Package.swift has no package name")?;
        return Ok(Metadata {
            name: name.into(),
            source: None,
            scheme: format!("{name}-Package"),
            configuration: "Debug".into(),
            platform: "macOS".into(),
            generate: None,
            package: true,
            deployment: None,
            targets: json!(spec["targets"].as_array().into_iter().flatten()
                .filter(|t| t["type"] == "test").map(|t| json!({
                    "target": t["name"], "buildSettings": { "PRODUCT_TYPE": "bundle.unit-test" },
                })).collect::<Vec<_>>()),
        });
    }
    let file = projects.first().ok_or_else(|| {
        BadArguments("no Apple project found; run `kr init` in a directory with an .xcodeproj, .xcworkspace, project.yml or Package.swift".into())
    })?;
    if projects.len() > 1 {
        question(
            decisions,
            "project",
            "project",
            "Which Xcode project should kr open?",
            &projects,
            file,
            json!({ "files": projects }),
        );
    }
    let source = if file.ends_with(".xcworkspace") {
        "workspace"
    } else {
        "xcodeproj"
    };
    let flag = if source == "workspace" {
        "-workspace"
    } else {
        "-project"
    };
    let list = read_json(
        runner,
        Command::new("xcodebuild").args(["-list", "-json", flag, file]),
        root,
    )?;
    let list = &list[if source == "workspace" {
        "workspace"
    } else {
        "project"
    }];
    let configuration = strings(&list["configurations"])
        .into_iter()
        .find(|s| s == "Debug")
        .or_else(|| strings(&list["configurations"]).into_iter().next())
        .unwrap_or_else(|| "Debug".into());
    let mut schemes = strings(&list["schemes"]);
    schemes.sort();
    let mut candidates = Vec::new();
    for scheme in &schemes {
        let settings = read_json(
            runner,
            Command::new("xcodebuild").args([
                "-showBuildSettings",
                "-json",
                flag,
                file,
                "-scheme",
                scheme,
                "-configuration",
                &configuration,
                "build-for-testing",
            ]),
            root,
        )?;
        candidates.push((scheme.clone(), settings));
    }
    let apps: Vec<_> =
        candidates
            .iter()
            .filter(|(_, settings)| {
                settings.as_array().into_iter().flatten().any(|t| {
                    t["buildSettings"]["PRODUCT_TYPE"] == "com.apple.product-type.application"
                })
            })
            .collect();
    let (scheme, targets) = apps
        .first()
        .copied()
        .or_else(|| candidates.first())
        .context("no shared scheme found; share an Xcode scheme, then run `kr init` again")?;
    let choices: Vec<_> = if apps.is_empty() {
        schemes
    } else {
        apps.iter().map(|(s, _)| s.clone()).collect()
    };
    if choices.len() > 1 {
        question(
            decisions,
            "scheme",
            "project.scheme",
            "Which App scheme should kr use?",
            &choices,
            scheme,
            json!({ "schemes": choices, "file": file }),
        );
    }
    let platform = targets
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["buildSettings"]["SUPPORTED_PLATFORMS"].as_str())
        .find_map(|s| {
            [
                ("iphone", "iOS"),
                ("appletv", "tvOS"),
                ("xros", "visionOS"),
                ("watch", "watchOS"),
                ("macosx", "macOS"),
            ]
            .into_iter()
            .find(|(p, _)| s.contains(p))
            .map(|(_, p)| p)
        })
        .unwrap_or("iOS");
    Ok(Metadata {
        name: list["name"].as_str().unwrap_or(scheme).to_owned(),
        source: Some((source, file.clone())),
        scheme: scheme.clone(),
        configuration,
        platform: platform.into(),
        targets: targets.clone(),
        generate: None,
        package: false,
        deployment: targets
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                t["buildSettings"][deployment_key(platform)]
                    .as_str()
                    .map(str::to_owned)
            })
            .max_by_key(|v| version(v)),
    })
}

fn xcodegen(root: &Path, runner: &dyn Runner, decisions: &mut Vec<Decision>) -> Result<Metadata> {
    let spec = read_json(
        runner,
        Command::new("xcodegen").args(["dump", "--type", "json", "--no-env"]),
        root,
    )?;
    let name = spec["name"]
        .as_str()
        .context("project.yml needs a project name")?;
    let targets = spec["targets"]
        .as_object()
        .context("project.yml has no targets")?;
    let apps: Vec<_> = targets
        .iter()
        .filter(|(_, t)| t["type"] == "application")
        .map(|(name, _)| name.clone())
        .collect();
    let mut schemes: Vec<_> = spec["schemes"]
        .as_object()
        .into_iter()
        .flat_map(|s| s.iter())
        .filter(|(_, s)| apps.iter().any(|a| !s["build"]["targets"][a].is_null()))
        .map(|(name, _)| name.clone())
        .collect();
    if schemes.is_empty() {
        schemes = apps.clone();
    }
    if schemes.is_empty() {
        schemes = spec["schemes"]
            .as_object()
            .into_iter()
            .flat_map(|s| s.keys().cloned())
            .collect();
    }
    let scheme = schemes
        .first()
        .context("project.yml has no scheme; declare schemes, then run `kr init` again")?;
    if schemes.len() > 1 {
        question(
            decisions,
            "scheme",
            "project.scheme",
            "Which App scheme should kr use?",
            &schemes,
            scheme,
            json!({ "schemes": schemes, "file": "project.yml" }),
        );
    }
    let included: Vec<_> = spec["schemes"][scheme]["test"]["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().or_else(|| v["name"].as_str()).map(str::to_owned))
        .collect();
    let relevant: Vec<_> = targets
        .iter()
        .filter(|(name, target)| {
            included.contains(name)
                || spec["schemes"][scheme]["build"]["targets"]
                    .get(*name)
                    .is_some()
                || (*name == scheme)
                || (spec["schemes"][scheme].is_null()
                    && target["dependencies"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|d| d["target"] == *scheme))
        })
        .collect();
    let platform = relevant
        .iter()
        .find(|(_, t)| t["type"] == "application")
        .or_else(|| relevant.first())
        .and_then(|(_, t)| t["platform"].as_str())
        .unwrap_or("iOS");
    let configuration = spec["schemes"][scheme]["test"]["config"]
        .as_str()
        .or_else(|| {
            spec["configs"].as_object().and_then(|configs| {
                configs
                    .iter()
                    .find(|(_, kind)| *kind == "debug")
                    .or_else(|| configs.iter().next())
                    .map(|(name, _)| name.as_str())
            })
        })
        .unwrap_or("Debug");
    Ok(Metadata {
        name: name.into(),
        source: Some(("xcodeproj", format!("{name}.xcodeproj"))),
        scheme: scheme.clone(),
        configuration: configuration.into(),
        platform: platform.into(),
        targets: json!(
            relevant
                .iter()
                .map(|(name, t)| json!({
                    "target": name, "buildSettings": { "PRODUCT_TYPE": t["type"] },
                }))
                .collect::<Vec<_>>()
        ),
        generate: Some("xcodegen generate --quiet".into()),
        package: false,
        deployment: relevant
            .iter()
            .filter_map(|(_, t)| {
                t["deploymentTarget"]
                    .as_str()
                    .or_else(|| t["settings"]["base"][deployment_key(platform)].as_str())
                    .or_else(|| spec["options"]["deploymentTarget"][platform].as_str())
            })
            .map(str::to_owned)
            .max_by_key(|v| version(v)),
    })
}

fn deployment_key(platform: &str) -> &str {
    match platform {
        "tvOS" => "TVOS_DEPLOYMENT_TARGET",
        "visionOS" => "XROS_DEPLOYMENT_TARGET",
        "watchOS" => "WATCHOS_DEPLOYMENT_TARGET",
        "macOS" => "MACOSX_DEPLOYMENT_TARGET",
        _ => "IPHONEOS_DEPLOYMENT_TARGET",
    }
}

fn read(runner: &dyn Runner, command: Command, root: &Path) -> Result<String> {
    let out = runner.run(&command.clone().current_dir(root))?;
    if !out.success() {
        bail!(
            "{command} failed; fix the project/tool installation and run `kr init` again: {}",
            out.stderr.trim()
        );
    }
    Ok(out.stdout)
}

fn read_json(runner: &dyn Runner, command: Command, root: &Path) -> Result<Value> {
    serde_json::from_str(&read(runner, command.clone(), root)?)
        .with_context(|| format!("cannot read JSON from {command}"))
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect()
}

fn version(value: &str) -> Vec<u32> {
    value.split('.').map(|s| s.parse().unwrap_or(0)).collect()
}

fn question(
    decisions: &mut Vec<Decision>,
    kind: &str,
    item: &str,
    text: &str,
    options: &[String],
    default: &str,
    evidence: Value,
) {
    decisions.push(Decision {
        id: format!("init-{}", decisions.len() + 1),
        kind: format!("init_{kind}"),
        affects: Affects::Efficiency,
        question: text.into(),
        item: item.into(),
        test: None,
        options: options
            .iter()
            .map(|s| Choice {
                name: s.clone(),
                means: format!("set {item} to {s}"),
            })
            .collect(),
        evidence,
        default: Suggestion {
            option: default.into(),
            reason: "stable minimal configuration; review and edit .kr.toml before running tests"
                .into(),
        },
    });
}
