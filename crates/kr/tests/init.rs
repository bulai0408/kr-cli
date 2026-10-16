use assert_cmd::Command;

struct Project {
    home: tempfile::TempDir,
}

impl Project {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("repo/.git")).unwrap();
        Self { home }
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.home.path().join("repo").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn kr(&self, args: &[&str]) -> assert_cmd::assert::Assert {
        self.command(args).assert()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::cargo_bin("kr").unwrap();
        cmd.env_clear()
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .current_dir(self.home.path().join("repo"))
            .args(args);
        cmd
    }

    fn init(&self, recordings: &[&str], args: &[&str]) -> assert_cmd::assert::Assert {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let files = recordings
            .iter()
            .map(|p| root.join("tests/fixtures").join(p).display().to_string())
            .collect::<Vec<_>>()
            .join(":");
        self.command(&["init"])
            .args(args)
            .env("KR_REPLAY", files)
            .assert()
    }

    fn config(&self) -> serde_json::Value {
        let out = self
            .kr(&["config", "--json"])
            .success()
            .get_output()
            .clone();
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()["config"]["project"]
            .clone()
    }

    fn init_edited(
        &self,
        recordings: &[&str],
        edit: impl Fn(kr_exec::Recording) -> Vec<kr_exec::Recording>,
    ) -> assert_cmd::assert::Assert {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
        let mut files = Vec::new();
        for file in recordings {
            for recording in edit(kr_exec::read_recording(&root.join(file)).unwrap()) {
                let path = self
                    .home
                    .path()
                    .join(format!("recording-{}.json", files.len()));
                std::fs::write(&path, serde_json::to_string(&recording).unwrap()).unwrap();
                files.push(path.display().to_string());
            }
        }
        self.command(&["init", "--json"])
            .env("KR_REPLAY", files.join(":"))
            .assert()
    }
}

#[test]
fn init_reads_an_xcode_project_and_generates_a_valid_minimal_config_without_building() {
    let p = Project::new();
    p.write("KrSample.xcodeproj/project.pbxproj", "");
    let out = p
        .init(
            &[
                "init/list.json",
                "init/settings.json",
                "simctl/list-runtimes.json",
            ],
            &[],
        )
        .success()
        .get_output()
        .clone();
    insta::assert_snapshot!(String::from_utf8(out.stdout).unwrap());
    let config = p.config();
    assert_eq!(config["project"]["name"], "KrSample");
    assert_eq!(config["project"]["xcodeproj"], "KrSample.xcodeproj");
    assert_eq!(config["project"]["scheme"], "KrSample");
    assert_eq!(config["project"]["configuration"], "Debug");
    assert_eq!(
        config["suites"]["unit"]["only"],
        serde_json::json!(["KrSampleTests"])
    );
    assert_eq!(
        config["scenarios"]["smoke"]["only"],
        serde_json::json!(["KrSampleUITests"])
    );
    assert_eq!(
        config["gates"]["default"]["scenarios"],
        serde_json::json!(["smoke"])
    );
    assert_eq!(config["simulators"]["runtime"], "iOS 27.1");
    assert_eq!(config["simulators"]["devices"]["default"]["count"], 1);
    assert!(!p.home.path().join("repo/build").exists());
    assert!(!p.home.path().join(".local/state/kr/resources").exists());
}

#[test]
fn init_never_overwrites_an_existing_config() {
    let p = Project::new();
    let original = "# Keep my settings\n[project]\nname = \"Existing\"\n";
    p.write(".kr.toml", original);
    let out = p.kr(&["init", "--json"]).code(2).get_output().clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already exists"),
        "{json}"
    );
    assert_eq!(
        std::fs::read_to_string(p.home.path().join("repo/.kr.toml")).unwrap(),
        original
    );
}

#[test]
fn init_reads_xcodegen_without_generating_or_building_the_project() {
    let p = Project::new();
    p.write("project.yml", "name: KrSample\n");
    let out = p
        .init(
            &["init/dump.json", "simctl/list-runtimes.json"],
            &["--json"],
        )
        .success()
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["ok"], true);
    let config = p.config();
    assert_eq!(config["project"]["generate"], "xcodegen generate --quiet");
    assert_eq!(config["project"]["xcodeproj"], "KrSample.xcodeproj");
    assert_eq!(config["project"]["scheme"], "KrSample");
    assert_eq!(
        config["suites"]["unit"]["only"],
        serde_json::json!(["KrSampleTests"])
    );
    assert_eq!(
        config["scenarios"]["smoke"]["only"],
        serde_json::json!(["KrSampleUITests"])
    );
    assert!(!p.home.path().join("repo/KrSample.xcodeproj").exists());
}

#[test]
fn init_accepts_a_standalone_swift_package_without_a_simulator() {
    let p = Project::new();
    p.write("Package.swift", "// Recorded SampleKit manifest\n");
    let out = p
        .init(&["init/package.json"], &["--json"])
        .success()
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert!(
        json["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["kind"] == "init_ui_target")
    );
    let config = p.config();
    assert_eq!(config["project"]["name"], "SampleKit");
    assert_eq!(config["project"]["platforms"], serde_json::json!(["macOS"]));
    assert_eq!(config["suites"]["unit"]["type"], "command");
    assert_eq!(config["suites"]["unit"]["run"], "swift test");
    assert_eq!(
        config["gates"]["default"]["suites"],
        serde_json::json!(["unit"])
    );
    assert_eq!(config["simulators"]["devices"], serde_json::json!({}));
    assert_eq!(config["scenarios"], serde_json::json!({}));
}

#[test]
fn init_reports_multiple_app_schemes_and_ui_targets_with_evidence_and_defaults() {
    let p = Project::new();
    p.write("KrSample.xcodeproj/project.pbxproj", "");
    let out = p
        .init_edited(
            &[
                "init/list.json",
                "init/settings.json",
                "simctl/list-runtimes.json",
            ],
            |mut r| {
                let mut v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
                if r.args.first().map(String::as_str) == Some("-list") {
                    v["project"]["schemes"] = serde_json::json!(["Other", "KrSample"]);
                } else if r.args.first().map(String::as_str) == Some("-showBuildSettings") {
                    let mut second = v[2].clone();
                    second["target"] = serde_json::json!("SecondUITests");
                    v.as_array_mut().unwrap().push(second);
                    r.stdout = serde_json::to_string(&v).unwrap();
                    let mut other = r.clone();
                    other.args[5] = "Other".into();
                    return vec![r, other];
                }
                r.stdout = serde_json::to_string(&v).unwrap();
                vec![r]
            },
        )
        .success()
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    insta::assert_json_snapshot!(json);
    assert_eq!(p.config()["project"]["scheme"], "KrSample");
    assert_eq!(
        p.config()["scenarios"]["smoke"]["only"],
        serde_json::json!(["KrSampleUITests"])
    );
}

#[test]
fn commands_without_a_project_explain_init_and_the_embedded_config_reference() {
    let p = Project::new();
    for command in ["check", "pool"] {
        let out = p.kr(&[command, "--json"]).failure().get_output().clone();
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let message = json["error"]["message"].as_str().unwrap();
        assert!(message.contains("kr init"), "{command}: {message}");
        assert!(
            message.contains("kr skill --reference config"),
            "{command}: {message}"
        );
        assert!(!message.contains("examples/"), "{command}: {message}");
    }
}

#[test]
fn init_reads_an_xcode_workspace_and_keeps_the_workspace_path() {
    let p = Project::new();
    p.write("KrSample.xcworkspace/contents.xcworkspacedata", "");
    p.init_edited(
        &[
            "init/list.json",
            "init/settings.json",
            "simctl/list-runtimes.json",
        ],
        |mut r| {
            if r.program == "xcodebuild" {
                r.args[2] = "-workspace".into();
                r.args[3] = "KrSample.xcworkspace".into();
                if r.args[0] == "-list" {
                    let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
                    r.stdout = serde_json::json!({ "workspace": v["project"] }).to_string();
                }
            }
            vec![r]
        },
    )
    .success();
    let config = p.config();
    assert_eq!(config["project"]["workspace"], "KrSample.xcworkspace");
    assert_eq!(config["project"]["xcodeproj"], serde_json::Value::Null);
}

#[test]
fn init_does_not_write_a_config_when_installed_runtimes_are_older_than_the_deployment_target() {
    let p = Project::new();
    p.write("KrSample.xcodeproj/project.pbxproj", "");
    let out = p
        .init_edited(
            &[
                "init/list.json",
                "init/settings.json",
                "simctl/list-runtimes.json",
            ],
            |mut r| {
                if r.args.first().map(String::as_str) == Some("-showBuildSettings") {
                    let mut v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
                    for t in v.as_array_mut().unwrap() {
                        t["buildSettings"]["IPHONEOS_DEPLOYMENT_TARGET"] =
                            serde_json::json!("99.0");
                    }
                    r.stdout = v.to_string();
                }
                vec![r]
            },
        )
        .failure()
        .get_output()
        .clone();
    assert!(!p.home.path().join("repo/.kr.toml").exists());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("runtime"),
        "{json}"
    );
}

#[test]
fn init_uses_a_macos_test_suite_for_native_ui_tests_without_inventing_a_runtime() {
    let p = Project::new();
    p.write("KrSample.xcodeproj/project.pbxproj", "");
    p.init_edited(&["init/list.json", "init/settings.json"], |mut r| {
        if r.args[0] == "-showBuildSettings" {
            let mut v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
            for t in v.as_array_mut().unwrap() {
                t["buildSettings"]["SUPPORTED_PLATFORMS"] = serde_json::json!("macosx");
            }
            r.stdout = v.to_string();
        }
        vec![r]
    })
    .success();
    let config = p.config();
    assert_eq!(config["project"]["platforms"], serde_json::json!(["macOS"]));
    assert_eq!(config["suites"]["smoke"]["destination"], "macos");
    assert_eq!(
        config["suites"]["smoke"]["only"],
        serde_json::json!(["KrSampleUITests"])
    );
    assert_eq!(
        config["gates"]["default"]["suites"],
        serde_json::json!(["unit", "smoke"])
    );
    assert_eq!(config["scenarios"], serde_json::json!({}));
}

#[test]
fn init_does_not_expose_private_build_settings_in_decision_evidence() {
    let p = Project::new();
    p.write("KrSample.xcodeproj/project.pbxproj", "");
    let out = p
        .init_edited(
            &[
                "init/list.json",
                "init/settings.json",
                "simctl/list-runtimes.json",
            ],
            |mut r| {
                if r.args[0] == "-showBuildSettings" {
                    let mut v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
                    v.as_array_mut().unwrap().pop();
                    v[0]["buildSettings"]["PRIVATE_TOKEN"] =
                        serde_json::json!("private-build-setting");
                    v[0]["buildSettings"]["HOME"] =
                        serde_json::json!("/Users/private-build-setting");
                    r.stdout = v.to_string();
                }
                vec![r]
            },
        )
        .success()
        .get_output()
        .clone();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains("private-build-setting"), "{text}");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let d = json["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["kind"] == "init_ui_target")
        .unwrap();
    assert_eq!(d["default"]["option"], "suite_only");
    assert_eq!(p.config()["scenarios"], serde_json::json!({}));
}

#[test]
fn init_respects_the_xcodegen_schemes_test_configuration() {
    let p = Project::new();
    p.write("project.yml", "name: KrSample\n");
    p.init_edited(&["init/dump.json", "simctl/list-runtimes.json"], |mut r| {
        if r.program == "xcodegen" {
            let mut v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap();
            v["configs"] = serde_json::json!({ "Acceptance": "debug", "Production": "release" });
            v["schemes"]["KrSample"]["test"]["config"] = serde_json::json!("Acceptance");
            r.stdout = v.to_string();
        }
        vec![r]
    })
    .success();
    assert_eq!(p.config()["project"]["configuration"], "Acceptance");
}
