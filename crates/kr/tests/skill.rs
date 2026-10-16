//! `kr skill` (ADR 0011 §6): the usage text ships with kr; a project and the owner add
//! to it without a new kr.

use assert_cmd::Command;

struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("repo/.git")).unwrap();
        Self { home }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.home.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn kr(&self, args: &[&str]) -> (Option<i32>, String, String) {
        let out = Command::cargo_bin("kr")
            .unwrap()
            .env_clear()
            .env("HOME", self.home.path())
            .env("NO_COLOR", "1")
            .current_dir(self.home.path().join("repo"))
            .args(args)
            .output()
            .unwrap();
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }
}

#[test]
fn the_main_text_matches_this_version_and_names_its_references() {
    let (code, out, _) = Sandbox::new().kr(&["skill"]);
    assert_eq!(code, Some(0));
    assert!(
        out.starts_with("# kr: build and test Apple projects\n"),
        "{out}"
    );
    assert!(
        out.contains(&format!("(version {})", env!("CARGO_PKG_VERSION"))),
        "{out}"
    );
    assert!(
        out.contains("`kr skill --reference <name>`: decisions, json, locks."),
        "{out}"
    );
    assert!(out.contains("4 no verdict yet"), "{out}");
    assert!(!out.contains('{'), "unfilled placeholder: {out}");
    // Short: the rare cases are in the references.
    assert!(out.lines().count() < 70, "{}", out.lines().count());
}

#[test]
fn a_reference_is_read_on_demand_and_an_unknown_one_exits_2() {
    let s = Sandbox::new();
    let (code, out, _) = s.kr(&["skill", "--reference", "decisions"]);
    assert_eq!(code, Some(0));
    assert!(out.starts_with("# Decision points\n"), "{out}");
    assert!(out.contains("## failure_kind"), "{out}");
    let (code, out, _) = s.kr(&["skill", "--reference", "locks"]);
    assert_eq!(code, Some(0));
    assert!(
        out.contains("kr lock build --task legacy-build -- bash scripts/build.sh"),
        "{out}"
    );
    assert!(
        out.contains("including 4") && out.contains("128+n"),
        "{out}"
    );

    let (code, _, err) = s.kr(&["skill", "--reference", "nope"]);
    assert_eq!(code, Some(2));
    assert!(
        err.contains("no reference `nope`; references: decisions, json, locks"),
        "{err}"
    );
}

#[test]
fn project_and_owner_additions_follow_the_main_text() {
    let s = Sandbox::new();
    s.write("repo/.kr.toml", "[project]\nname = \"X\"\n");
    s.write(
        "repo/.kr/skill.md",
        "Krill: never judge CloudKit failures yourself.\n",
    );
    s.write(
        ".config/kr/skill.md",
        "Kami: ask before a second flaky call.",
    );
    let (code, out, _) = s.kr(&["skill"]);
    assert_eq!(code, Some(0));
    let project = out.find("## Added by the project (").expect(&out);
    let owner = out.find("## Added by the owner (").expect(&out);
    assert!(project < owner, "{out}");
    assert!(out.contains("Krill: never judge CloudKit failures yourself.\n"));
    assert!(
        out.ends_with("Kami: ask before a second flaky call.\n"),
        "{out}"
    );
    // A reference has no additions.
    let (_, out, _) = s.kr(&["skill", "--reference", "json"]);
    assert!(!out.contains("Added by"), "{out}");

    let (_, out, _) = s.kr(&["--json", "skill"]);
    let json: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["name"], "main");
    assert_eq!(
        json["references"],
        serde_json::json!(["decisions", "json", "locks"])
    );
    assert_eq!(json["additions"][0]["kind"], "project");
    assert_eq!(json["additions"][1]["kind"], "owner");
}
