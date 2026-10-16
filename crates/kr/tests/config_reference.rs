use std::collections::BTreeSet;

use assert_cmd::Command;

fn reference() -> String {
    let home = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("kr")
        .unwrap()
        .env_clear()
        .env("HOME", home.path())
        .args(["skill", "--reference", "config", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["name"], "config");
    json["text"].as_str().unwrap().to_owned()
}

/// Read the actual Deserialize definitions, including optional or skipped fields:
/// serializing an example alone would miss a new skip_serializing_if field.
#[test]
fn the_embedded_config_reference_documents_every_configuration_field() {
    let text = reference();
    let mut keys = BTreeSet::new();
    for source in [
        include_str!("../../kr-config/src/lib.rs"),
        include_str!("../../kr-config/src/project.rs"),
    ] {
        for item in syn::parse_file(source).unwrap().items {
            let (name, attrs, fields) = match item {
                syn::Item::Struct(s) => (s.ident, s.attrs, vec![s.fields]),
                syn::Item::Enum(e) => (
                    e.ident,
                    e.attrs,
                    e.variants.into_iter().map(|v| v.fields).collect(),
                ),
                _ => continue,
            };
            if !attrs.iter().any(|attr| {
                attr.path().is_ident("derive")
                    && attr
                        .parse_args_with(
                            syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
                        )
                        .is_ok_and(|p| p.iter().any(|p| p.is_ident("Deserialize")))
            }) {
                continue;
            }
            let prefix = match name.to_string().as_str() {
                "MachineConfig" | "ProjectConfig" => "",
                "ProjectInfo" => "project.",
                "Simulators" => "simulators.",
                "DeviceKind" => "simulators.devices.<kind>.",
                "Suite" => "suites.<name>.",
                "Scenario" => "scenarios.<name>.",
                "Prepare" => "scenarios.<name>.prepare[].",
                "Module" => "modules.<name>.",
                "SelectionConfig" => "selection.",
                "Gate" => "gates.<name>.",
                "Isolation" => "isolation.",
                "Signing" => "signing.",
                "Remote" => "remotes.<name>.",
                "Legacy" => "legacy.",
                "LoadCheck" => "load.",
                "MachineOverride" => "machines.<host>.",
                "Resources" => "resources.",
                "Platform" | "Destination" => continue,
                other => panic!("add a config reference mapping for new type {other}"),
            };
            for field in fields.into_iter().flat_map(|fields| fields.into_iter()) {
                keys.insert(format!("{prefix}{}", field.ident.unwrap()));
            }
        }
    }
    keys.insert("suites.<name>.type".into());
    keys.extend(["build", "sim", "light"].map(|r| format!("machines.<host>.resources.{r}")));
    let missing: Vec<_> = keys
        .iter()
        .filter(|key| !text.contains(&format!("`{key}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "undocumented configuration fields: {missing:?}"
    );
}

#[test]
fn the_complete_commented_examples_load_through_kr_config() {
    let text = reference();
    let examples: Vec<_> = text
        .split("```toml\n")
        .skip(1)
        .map(|s| s.split("```").next().unwrap())
        .collect();
    assert_eq!(
        examples.len(),
        2,
        "one project example and one machine example"
    );
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let machine = home.path().join("config.toml");
    std::fs::write(repo.join(".kr.toml"), examples[0]).unwrap();
    std::fs::write(&machine, examples[1]).unwrap();
    let out = Command::cargo_bin("kr")
        .unwrap()
        .env_clear()
        .env("HOME", home.path())
        .env("KR_CONFIG", machine)
        .current_dir(repo)
        .args(["config", "--json"])
        .assert()
        .success()
        .get_output()
        .clone();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["config"]["project"]["project"]["name"], "Atlas");
    assert_eq!(
        json["config"]["project"]["suites"]["build"]["type"],
        "build"
    );
    assert_eq!(json["config"]["project"]["suites"]["unit"]["type"], "test");
    assert_eq!(
        json["config"]["project"]["suites"]["lint"]["type"],
        "command"
    );
    assert_eq!(json["config"]["machine"]["load"]["per_core"], 4.0);
}
