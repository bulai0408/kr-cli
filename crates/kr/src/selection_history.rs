//! Project-scoped, run-level counts for selection evidence. The EventLog reader is
//! shared with `kr report`; automatic reruns are not independent observations.
use std::collections::BTreeMap;

use anyhow::Result;
use jiff::{SignedDuration, Timestamp};
use kr_events::EventKind;
use serde_json::{Value, json};

use crate::Loaded;

pub fn read(loaded: &Loaded) -> Result<Value> {
    let since = Timestamp::now() - SignedDuration::from_hours(24 * 90);
    let events = loaded.events().read_since(since)?;
    let mut runs = BTreeMap::<_, BTreeMap<String, Value>>::new();
    for e in events
        .iter()
        .filter(|e| e.kind == EventKind::ItemEnd && e.project.as_deref() == loaded.project())
    {
        let Some(item) = e.fields["item"].as_str() else {
            continue;
        };
        if !loaded
            .config
            .project
            .as_ref()
            .is_some_and(|p| p.scenarios.contains_key(item))
        {
            continue;
        }
        runs.entry(e.run_id)
            .or_default()
            .insert(item.to_owned(), e.fields.clone());
    }
    let mut counts = BTreeMap::<String, (u64, u64, u64)>::new();
    let mut pairs = BTreeMap::<(String, String), (u64, u64)>::new();
    for items in runs.values() {
        for (item, f) in items {
            let c = counts.entry(item.clone()).or_default();
            c.0 += 1;
            c.1 += u64::from(failed(f));
            c.2 += u64::from(
                f["flaky"].as_array().is_some_and(|a| !a.is_empty())
                    || f["tests"]["flaky"].as_u64().unwrap_or(0) > 0,
            );
        }
        let items: Vec<_> = items
            .iter()
            .filter(|(_, f)| {
                f["passed"].is_boolean() && f["tests"]["unverified"].as_u64().unwrap_or(0) == 0
            })
            .collect();
        for (i, (a, af)) in items.iter().enumerate() {
            for (b, bf) in &items[i + 1..] {
                let c = pairs.entry(((*a).clone(), (*b).clone())).or_default();
                c.0 += 1;
                c.1 += u64::from(failed(af) && failed(bf));
            }
        }
    }
    let project = loaded
        .config
        .project
        .as_ref()
        .expect("selection requires a project");
    let scenarios: Vec<_> = project.scenarios.keys().map(|name| {
        let (runs, failed, flaky) = counts.get(name).copied().unwrap_or_default();
        json!({"item":name,"runs":runs,"failed":failed,"flaky":flaky,"flaky_rate":ratio(flaky,runs)})
    }).collect();
    let cofailures: Vec<_> = pairs.into_iter().map(|((a,b),(together,failed))| json!({"items":[a,b],"together":together,"failed_together":failed,"rate":ratio(failed,together)})).collect();
    Ok(
        json!({"days":90,"project":loaded.project(),"scenarios":scenarios,"cofailures":cofailures,"unit":"one final item outcome per run; automatic reruns are not independent samples"}),
    )
}
fn failed(f: &Value) -> bool {
    f["passed"] == false && f["tests"]["unverified"].as_u64().unwrap_or(0) == 0
}
fn ratio(n: u64, d: u64) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}
