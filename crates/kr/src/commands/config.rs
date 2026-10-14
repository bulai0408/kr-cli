use std::process::ExitCode;

use anyhow::Result;
use kr_config::Sources;
use serde_json::json;

use crate::Ctx;

pub fn run(ctx: &Ctx) -> Result<ExitCode> {
    let sources = Sources::discover(&ctx.cwd, ctx.env.clone());
    let config = sources.load()?;
    let machine = sources.machine.filter(|p| p.is_file());
    let project = sources.project;
    let state_dir = config.state_dir(&ctx.env);

    if ctx.json {
        let out = json!({
            "sources": { "machine": machine, "project": project },
            "state_dir": state_dir,
            "config": config,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        let show = |p: &Option<std::path::PathBuf>| {
            p.as_ref()
                .map_or_else(|| "(none)".to_owned(), |p| p.display().to_string())
        };
        println!("machine config: {}", show(&machine));
        println!("project config: {}", show(&project));
        println!("state dir:      {}", show(&state_dir));
        println!();
        print!("{}", toml::to_string(&config)?);
    }
    Ok(ExitCode::SUCCESS)
}
