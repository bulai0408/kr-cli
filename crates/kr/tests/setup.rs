use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

struct Sandbox {
    home: tempfile::TempDir,
    project: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            project: tempfile::tempdir().unwrap(),
        }
    }

    fn kr(&self) -> Command {
        let mut cmd = Command::cargo_bin("kr").unwrap();
        cmd.env_clear()
            .env("HOME", self.home.path())
            .current_dir(self.project.path());
        cmd
    }

    fn setup(&self, args: &[&str]) -> Value {
        let out = self
            .kr()
            .args(["setup", "--json"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn paths(&self, project: bool, claude: bool) -> (PathBuf, PathBuf) {
        let base = if project {
            self.project.path().canonicalize().unwrap()
        } else {
            self.home.path().to_owned()
        };
        let skill = base.join(if claude {
            ".claude/skills/kr/SKILL.md"
        } else {
            ".agents/skills/kr/SKILL.md"
        });
        let rules = base.join(match (project, claude) {
            (true, true) => "CLAUDE.md",
            (true, false) => "AGENTS.md",
            (false, true) => ".claude/CLAUDE.md",
            (false, false) => ".codex/AGENTS.md",
        });
        (skill, rules)
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn setup_installs_only_a_discovery_entry_is_idempotent_and_removes_both_scopes() {
    for project in [false, true] {
        for claude in [false, true] {
            let s = Sandbox::new();
            let (skill, rules) = s.paths(project, claude);
            let mut args = vec![if claude { "--claude" } else { "--codex" }];
            if project {
                args.push("--project");
            }
            let installed = s.setup(&args);
            assert_eq!(installed["schema_version"], 1);
            assert_eq!(installed["decisions"], serde_json::json!([]));
            let written = installed["written"].as_array().unwrap();
            assert_eq!(written.len(), 2);
            assert!(written.contains(&serde_json::json!(skill)));
            assert!(written.contains(&serde_json::json!(rules)));
            let entry = std::fs::read_to_string(&skill).unwrap();
            assert!(
                entry.contains("name: kr")
                    && entry.contains("description:")
                    && entry.contains("`kr skill`")
            );
            assert!(entry.lines().count() < 10, "{entry}");
            let rule_text = std::fs::read_to_string(&rules).unwrap();
            assert!(rule_text.contains("<!-- kr -->") && rule_text.contains("`kr skill`"));
            let repeat = s.setup(&args);
            assert_eq!(repeat["written"], serde_json::json!([]));
            assert_eq!(std::fs::read_to_string(&skill).unwrap(), entry);
            assert_eq!(std::fs::read_to_string(&rules).unwrap(), rule_text);
            let other = s.paths(!project, claude);
            assert!(!other.0.exists() && !other.1.exists());
            args.push("--remove");
            let removed = s.setup(&args);
            assert_eq!(removed["removed"].as_array().unwrap().len(), 2);
            assert!(!skill.exists() && !rules.exists());
            assert!(!skill.parent().unwrap().exists());
            assert_eq!(s.setup(&args)["removed"], serde_json::json!([]));
        }
    }
}

#[test]
fn setup_preserves_every_byte_outside_the_markers_and_remove_restores_it() {
    for original in ["# Owner rules", "# Owner rules\r\n\r\n", "前置规则\n"] {
        let s = Sandbox::new();
        let (_, rules) = s.paths(true, false);
        write(&rules, original);
        s.setup(&["--project", "--codex"]);
        assert!(
            std::fs::read_to_string(&rules)
                .unwrap()
                .starts_with(original)
        );
        s.setup(&["--project", "--codex", "--remove"]);
        assert_eq!(std::fs::read_to_string(&rules).unwrap(), original);
        write(
            &rules,
            "prefix\r\n<!-- kr -->old content<!-- kr -->\r\nsuffix",
        );
        s.setup(&["--project", "--codex"]);
        let installed = std::fs::read_to_string(&rules).unwrap();
        assert!(installed.starts_with("prefix\r\n<!-- kr -->"));
        assert!(installed.ends_with("<!-- kr -->\r\nsuffix"));
        assert!(!installed.contains("old content"));
        assert_eq!(
            s.setup(&["--project", "--codex"])["written"],
            serde_json::json!([])
        );
        s.setup(&["--project", "--codex", "--remove"]);
        assert_eq!(
            std::fs::read_to_string(&rules).unwrap(),
            "prefix\r\n\r\nsuffix"
        );
    }
}

#[test]
fn both_agents_share_imported_rules_instead_of_installing_a_duplicate_block() {
    for project in [false, true] {
        let s = Sandbox::new();
        let (_, claude_rules) = s.paths(project, true);
        let (_, codex_rules) = s.paths(project, false);
        let import = if project {
            "# Owner\n@AGENTS.md\n"
        } else {
            "# Owner\n@~/.codex/AGENTS.md\n"
        };
        write(&claude_rules, import);
        let args = if project { vec!["--project"] } else { vec![] };
        let out = s.setup(&args);
        assert_eq!(out["written"].as_array().unwrap().len(), 3);
        assert_eq!(out["skipped"][0]["path"], serde_json::json!(claude_rules));
        assert_eq!(
            out["skipped"][0]["reason"],
            "already imports the Codex rule file"
        );
        assert_eq!(std::fs::read_to_string(&claude_rules).unwrap(), import);
        assert!(
            std::fs::read_to_string(&codex_rules)
                .unwrap()
                .contains("<!-- kr -->")
        );
        let human = s.kr().arg("setup").args(&args).output().unwrap();
        assert!(human.status.success());
        assert!(
            String::from_utf8_lossy(&human.stdout).contains("already imports the Codex rule file")
        );
        let mut remove = args;
        remove.push("--remove");
        s.setup(&remove);
        assert_eq!(std::fs::read_to_string(claude_rules).unwrap(), import);
        assert!(!codex_rules.exists());
    }
}

#[test]
fn setup_uses_codex_home_and_the_effective_override_file() {
    let s = Sandbox::new();
    let custom = s.home.path().join("custom-codex");
    for (project, rules) in [
        (false, custom.join("AGENTS.override.md")),
        (true, s.project.path().join("AGENTS.override.md")),
    ] {
        write(&rules, "# Effective rules");
        let regular = rules.with_file_name("AGENTS.md");
        write(&regular, "# Inactive rules");
        let mut cmd = s.kr();
        cmd.env("CODEX_HOME", &custom)
            .args(["setup", "--codex", "--json"]);
        if project {
            cmd.arg("--project");
        }
        cmd.assert().success();
        assert!(
            std::fs::read_to_string(&rules)
                .unwrap()
                .contains("<!-- kr -->")
        );
        assert_eq!(
            std::fs::read_to_string(&regular).unwrap(),
            "# Inactive rules"
        );
        let mut cmd = s.kr();
        cmd.env("CODEX_HOME", &custom)
            .args(["setup", "--codex", "--remove"]);
        if project {
            cmd.arg("--project");
        }
        cmd.assert().success();
        assert_eq!(
            std::fs::read_to_string(&rules).unwrap(),
            "# Effective rules"
        );
    }
    assert!(!s.home.path().join(".codex").exists());
}

#[test]
fn setup_refuses_malformed_markers_or_a_foreign_skill_before_writing_anything() {
    for foreign_skill in [false, true] {
        let s = Sandbox::new();
        let (skill, rules) = s.paths(true, false);
        let (claude_skill, _) = s.paths(true, true);
        if foreign_skill {
            write(&skill, "owner's custom skill");
        } else {
            write(&rules, "# Owner\n<!-- kr -->unfinished");
        }
        let out = s.kr().args(["setup", "--project"]).output().unwrap();
        assert!(!out.status.success());
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(
            error.contains(if foreign_skill {
                "different skill"
            } else {
                "markers"
            }),
            "{error}"
        );
        assert!(!claude_skill.exists());
        if foreign_skill {
            assert_eq!(
                std::fs::read_to_string(&skill).unwrap(),
                "owner's custom skill"
            );
        } else {
            assert!(!skill.exists());
        }
    }
}
