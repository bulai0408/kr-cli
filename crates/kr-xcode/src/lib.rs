//! xcodebuild and the products it leaves behind: command lines with kr's fixed flags,
//! error summaries, `.xctestrun` files, test symbols, and the identity of the app
//! under test.

mod results;

pub use results::{
    Enumeration, TestCase, TestOutcome, only_testing_id, parse_test_results, results_command,
};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use kr_exec::Command;

/// Builds an `xcodebuild` command line. Flags kr always adds (pitfalls B3, B4, T3):
/// `-quiet -skipMacroValidation`, and for test actions
/// `-collect-test-diagnostics never -parallel-testing-enabled NO`.
#[derive(Debug, Clone, Default)]
pub struct Xcodebuild {
    action: String,
    project: Option<PathBuf>,
    workspace: Option<PathBuf>,
    scheme: Option<String>,
    configuration: Option<String>,
    destination: Option<String>,
    derived_data: Option<PathBuf>,
    xctestrun: Option<PathBuf>,
    extra: Vec<String>,
    settings: Vec<(String, String)>,
}

impl Xcodebuild {
    pub fn new(action: &str) -> Self {
        Self {
            action: action.to_owned(),
            ..Self::default()
        }
    }

    pub fn project(mut self, project: Option<PathBuf>, workspace: Option<PathBuf>) -> Self {
        self.project = project;
        self.workspace = workspace;
        self
    }

    pub fn scheme(mut self, scheme: &str) -> Self {
        self.scheme = Some(scheme.to_owned());
        self
    }

    pub fn configuration(mut self, configuration: Option<&str>) -> Self {
        self.configuration = configuration.map(str::to_owned);
        self
    }

    pub fn destination(mut self, destination: &str) -> Self {
        self.destination = Some(destination.to_owned());
        self
    }

    pub fn derived_data(mut self, path: impl Into<PathBuf>) -> Self {
        self.derived_data = Some(path.into());
        self
    }

    pub fn xctestrun(mut self, path: impl Into<PathBuf>) -> Self {
        self.xctestrun = Some(path.into());
        self
    }

    /// Extra arguments, e.g. `-only-testing:…`.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.extra.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn settings<I, K, V>(mut self, settings: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.settings
            .extend(settings.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    fn is_test(&self) -> bool {
        matches!(self.action.as_str(), "test" | "test-without-building")
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new("xcodebuild").args([
            self.action.as_str(),
            "-quiet",
            "-skipMacroValidation",
        ]);
        let path = |p: &PathBuf| p.display().to_string();
        if let Some(p) = &self.project {
            cmd = cmd.args(["-project".to_owned(), path(p)]);
        }
        if let Some(w) = &self.workspace {
            cmd = cmd.args(["-workspace".to_owned(), path(w)]);
        }
        if let Some(s) = &self.scheme {
            cmd = cmd.args(["-scheme", s.as_str()]);
        }
        if let Some(c) = &self.configuration {
            cmd = cmd.args(["-configuration", c.as_str()]);
        }
        if let Some(d) = &self.destination {
            cmd = cmd.args(["-destination", d.as_str()]);
        }
        if let Some(dd) = &self.derived_data {
            cmd = cmd.args(["-derivedDataPath".to_owned(), path(dd)]);
        }
        if let Some(x) = &self.xctestrun {
            cmd = cmd.args(["-xctestrun".to_owned(), path(x)]);
        }
        if self.is_test() {
            cmd = cmd.args([
                "-collect-test-diagnostics",
                "never",
                "-parallel-testing-enabled",
                "NO",
            ]);
        }
        cmd = cmd.args(self.extra.iter().cloned());
        cmd.args(self.settings.iter().map(|(k, v)| format!("{k}={v}")))
    }
}

/// At most three lines that explain a failure: distinct compiler errors first,
/// else the last non-empty lines.
pub fn error_summary(log: &str) -> Vec<String> {
    let distinct = |pred: &dyn Fn(&str) -> bool| -> Vec<String> {
        let mut seen = BTreeSet::new();
        log.lines()
            .map(str::trim)
            .filter(|l| pred(l))
            .filter(|l| seen.insert(l.to_string()))
            .take(3)
            .map(str::to_owned)
            .collect()
    };
    // `path:line:col: error: …` from the compiler.
    let located = distinct(&|l: &str| {
        l.split_once(": error:")
            .is_some_and(|(loc, _)| loc.matches(':').count() >= 2 && !loc.contains(' '))
    });
    if !located.is_empty() {
        return located;
    }
    let errors = distinct(&|l: &str| l.starts_with("error:") || l.contains(": error:"));
    if !errors.is_empty() {
        return errors;
    }
    let lines: Vec<&str> = log
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(3)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

/// One test target of an `.xctestrun` file. Paths are relative to the products dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestTarget {
    pub name: String,
    pub module: String,
    /// The `.xctest` bundle.
    pub bundle: PathBuf,
    /// The app under test (UI tests) or the host app (unit tests), if any.
    pub app: Option<PathBuf>,
    pub ui: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XcTestRun {
    pub path: PathBuf,
    pub targets: Vec<TestTarget>,
}

impl XcTestRun {
    /// The one `.xctestrun` in a products dir.
    pub fn find(products: &Path) -> Result<PathBuf> {
        let mut found: Vec<PathBuf> = std::fs::read_dir(products)
            .with_context(|| format!("no build products in {}", products.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "xctestrun"))
            .collect();
        found.sort();
        found.pop().with_context(|| {
            format!(
                "no .xctestrun in {}; build-for-testing did not finish",
                products.display()
            )
        })
    }

    pub fn read(path: &Path) -> Result<Self> {
        let root: plist::Dictionary =
            plist::from_file(path).with_context(|| format!("cannot read {}", path.display()))?;
        let entries: Vec<plist::Dictionary> = match root.get("TestConfigurations") {
            // Format version 2: TestConfigurations[].TestTargets[].
            Some(configs) => configs
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c.as_dictionary()?.get("TestTargets")?.as_array().cloned())
                .flatten()
                .filter_map(|t| t.into_dictionary())
                .collect(),
            // Format version 1: one entry per target.
            None => root
                .iter()
                .filter(|(k, _)| !k.starts_with("__"))
                .filter_map(|(_, v)| v.as_dictionary().cloned())
                .collect(),
        };
        let mut targets: Vec<TestTarget> = entries.iter().filter_map(target).collect();
        targets.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self {
            path: path.to_owned(),
            targets,
        })
    }

    pub fn target(&self, name: &str) -> Option<&TestTarget> {
        self.targets.iter().find(|t| t.name == name)
    }
}

fn target(entry: &plist::Dictionary) -> Option<TestTarget> {
    let text = |k: &str| entry.get(k).and_then(|v| v.as_string()).map(str::to_owned);
    let name = text("BlueprintName")?;
    let resolve = |p: &str| PathBuf::from(p.trim_start_matches("__TESTROOT__/"));
    let host = text("TestHostPath").map(|h| resolve(&h));
    let bundle_raw = text("TestBundlePath")?;
    let bundle = match (&host, bundle_raw.strip_prefix("__TESTHOST__/")) {
        (Some(host), Some(rest)) => host.join(rest),
        _ => resolve(&bundle_raw),
    };
    let ui = entry
        .get("IsUITestBundle")
        .and_then(|v| v.as_boolean())
        .unwrap_or(false);
    let app = match text("UITargetAppPath") {
        Some(app) => Some(resolve(&app)),
        None => host.filter(|h| h.extension().is_some_and(|x| x == "app")),
    };
    Some(TestTarget {
        module: text("ProductModuleName").unwrap_or_else(|| name.clone()),
        name,
        bundle,
        app,
        ui,
    })
}

/// Test classes and methods found in built test bundles (`nm -U | swift-demangle`).
#[derive(Debug, Default, Clone)]
pub struct TestSymbols {
    /// target → class → methods. Free functions are under class "".
    targets: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

impl TestSymbols {
    /// Shell command that prints the demangled symbols of a test bundle's binary.
    pub fn command(bundle_binary: &Path) -> Command {
        Command::new("sh").args([
            "-c".to_owned(),
            format!("nm -U '{}' | xcrun swift-demangle", bundle_binary.display()),
        ])
    }

    /// Adds the demangled `nm` output of `target`'s bundle; `target` is also its module.
    pub fn add(&mut self, target: &str, demangled: &str) {
        let classes = self.targets.entry(target.to_owned()).or_default();
        let prefix = format!("{target}.");
        for line in demangled.lines() {
            let mut fields = line.splitn(3, ' ');
            let (Some(_), Some(_), Some(rest)) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let rest = rest.strip_prefix("@objc ").unwrap_or(rest);
            let Some(path) = rest.strip_prefix(&prefix) else {
                continue;
            };
            let Some((qualified, _)) = path.split_once('(') else {
                continue;
            };
            let parts: Vec<&str> = qualified.split('.').collect();
            let (class, method) = match parts.as_slice() {
                [method] => ("", *method),
                [class, .., method] => (*class, *method),
                [] => continue,
            };
            if method.is_empty()
                || method.starts_with("__")
                || matches!(method, "init" | "deinit")
                || method.contains(' ')
            {
                continue;
            }
            classes
                .entry(class.to_owned())
                .or_default()
                .insert(method.to_owned());
        }
    }

    /// True if the `-only-testing` style filter "Target[/Class[/method]]" names
    /// something that exists.
    pub fn matches(&self, filter: &str) -> bool {
        let mut parts = filter.splitn(3, '/');
        let Some(classes) = parts.next().and_then(|t| self.targets.get(t)) else {
            return false;
        };
        let strip = |s: &str| s.trim_end_matches("()").to_owned();
        match (parts.next(), parts.next()) {
            (None, _) => classes.values().any(|m| !m.is_empty()),
            (Some(class), None) => {
                classes.contains_key(class)
                    || classes.get("").is_some_and(|f| f.contains(&strip(class)))
            }
            (Some(class), Some(method)) => classes
                .get(class)
                .is_some_and(|m| m.contains(&strip(method))),
        }
    }

    pub fn has_target(&self, target: &str) -> bool {
        self.targets.contains_key(target)
    }
}

/// The app's bundle identifier from its built Info.plist.
pub fn bundle_identifier(app: &Path) -> Result<String> {
    let info: plist::Dictionary = plist::from_file(app.join("Info.plist"))
        .with_context(|| format!("cannot read {}/Info.plist", app.display()))?;
    info.get("CFBundleIdentifier")
        .and_then(|v| v.as_string())
        .map(str::to_owned)
        .with_context(|| format!("{}/Info.plist has no CFBundleIdentifier", app.display()))
}

/// The app's executable from its built Info.plist.
pub fn executable(app: &Path) -> Result<PathBuf> {
    let info: plist::Dictionary = plist::from_file(app.join("Info.plist"))
        .with_context(|| format!("cannot read {}/Info.plist", app.display()))?;
    let name = info
        .get("CFBundleExecutable")
        .and_then(|v| v.as_string())
        .with_context(|| format!("{}/Info.plist has no CFBundleExecutable", app.display()))?;
    Ok(app.join(name))
}

/// Command that dumps a simulator build's entitlements: Xcode embeds them as XML in
/// the executable's `__TEXT,__entitlements` section (codesign shows none there).
pub fn entitlements_command(executable: &Path) -> Command {
    Command::new("xcrun").args([
        "otool".to_owned(),
        "-X".to_owned(),
        "-s".to_owned(),
        "__TEXT".to_owned(),
        "__entitlements".to_owned(),
        executable.display().to_string(),
    ])
}

/// Decodes `otool -X -s __TEXT __entitlements` output into the entitlements plist.
pub fn parse_entitlements_dump(dump: &str) -> Result<plist::Dictionary> {
    let mut bytes = Vec::new();
    let mut last_addr = 0u64;
    for line in dump.lines() {
        let mut tokens = line.split_whitespace();
        let Some(addr) = tokens.next() else { continue };
        let Ok(addr) = u64::from_str_radix(addr, 16) else {
            continue;
        };
        // A universal binary lists one section per architecture, each starting at a
        // lower address again; the first one is enough.
        if addr < last_addr {
            break;
        }
        last_addr = addr;
        for token in tokens {
            match token.len() {
                2 => bytes.push(u8::from_str_radix(token, 16)?),
                // Some otool builds print little-endian 32-bit words.
                8 => bytes.extend(u32::from_str_radix(token, 16)?.to_le_bytes()),
                _ => bail!("unexpected otool token `{token}`"),
            }
        }
    }
    if bytes.is_empty() {
        return Ok(plist::Dictionary::new());
    }
    let end = bytes.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    plist::from_bytes(&bytes[..end]).context("the __entitlements section is not a plist")
}
