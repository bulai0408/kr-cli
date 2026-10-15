pub mod check;
pub mod config;
pub mod deliver;
pub mod doctor;
pub mod lock;
pub mod plan;
pub mod pool;
pub mod queue;
pub mod rejudge;
pub mod remote;
pub mod run;
pub mod simulate;

use clap::builder::TypedValueParser;
use kr_resource::Resource;

/// clap parser for a resource name.
pub fn resource_parser() -> impl TypedValueParser<Value = Resource> {
    clap::builder::PossibleValuesParser::new(["build", "sim", "light"])
        .map(|s| s.parse::<Resource>().expect("listed above"))
}

/// Short human duration: `45s`, `12m`, `1h05m`.
pub fn human_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
    }
}
