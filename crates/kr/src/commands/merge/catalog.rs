use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use kr_exec::Command;
use serde_json::{Map, Value, json};

use crate::Ctx;

fn read(path: &Path) -> Result<Map<String, Value>> {
    let value: Value = super::json::parse(&std::fs::read(path)?)
        .with_context(|| format!("invalid JSON in {}; repair the catalog", path.display()))?;
    let object = value.as_object().context("catalog must be a JSON object")?;
    if !object.get("strings").is_some_and(Value::is_object)
        || !object.get("sourceLanguage").is_some_and(Value::is_string)
        || !object.get("version").is_some_and(Value::is_string)
    {
        bail!(
            "{}: expected strings, sourceLanguage and version; repair the catalog",
            path.display()
        );
    }
    Ok(object.clone())
}

fn merge(
    base: &Map<String, Value>,
    ours: &Map<String, Value>,
    theirs: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let keys: BTreeSet<&String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let mut result = Map::new();
    for key in keys {
        let (b, o, t) = (base.get(key), ours.get(key), theirs.get(key));
        let value = if o == t || t == b {
            o
        } else if o == b {
            t
        } else {
            // Never print the entry: translations may contain private user data.
            bail!("catalog entry conflict; resolve the catalog manually (ours was not changed)");
        };
        if let Some(value) = value {
            result.insert(key.clone(), value.clone());
        }
    }
    Ok(result)
}

pub(super) fn run(ctx: &Ctx, base: &Path, ours: &Path, theirs: &Path) -> Result<ExitCode> {
    let (b, o, t) = (read(base)?, read(ours)?, read(theirs)?);
    let entries = merge(
        b["strings"].as_object().unwrap(),
        o["strings"].as_object().unwrap(),
        t["strings"].as_object().unwrap(),
    )?;
    let strip = |mut m: Map<String, Value>| {
        m.remove("strings");
        m
    };
    let mut result = merge(&strip(b), &strip(o), &strip(t))?;
    result.insert("strings".to_owned(), Value::Object(entries));
    let temp = ours.with_file_name(format!(
        "{}.kr-merge.xcstrings",
        ours.file_name()
            .context("ours must name a file")?
            .to_string_lossy()
    ));
    // create_new avoids clobbering an existing file or following a symlink.
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .with_context(|| {
            format!(
                "cannot create {}; remove a stale merge temp file",
                temp.display()
            )
        })?;
    let checked = (|| {
        file.set_permissions(std::fs::metadata(ours)?.permissions())?;
        file.write_all((serde_json::to_string_pretty(&result)? + "\n").as_bytes())?;
        let out = ctx.runner()?.run(
            &Command::new("xcrun")
                .args(["xcstringstool", "print", &temp.to_string_lossy()])
                .current_dir(&ctx.cwd),
        )?;
        if !out.success() {
            bail!(
                "xcstringstool rejected the merged catalog; inspect the catalog in Xcode (ours was not changed)"
            );
        }
        std::fs::rename(&temp, ours)?;
        Ok(())
    })();
    if temp.exists() {
        std::fs::remove_file(&temp)?;
    }
    checked?;
    super::report(
        ctx,
        json!({"ok":true,"validated":true,"ours":ours}),
        "Catalog merged and validated with xcstringstool.",
        true,
    )
}
