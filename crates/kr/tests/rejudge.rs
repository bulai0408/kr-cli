//! `kr rejudge --evidence`: result bundles from before kr (Krill's v0-evidence), read
//! once on a copy, then judged again from kr's cache.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use kr_exec::Recording;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn xcresult(name: &str) -> String {
    std::fs::read_to_string(root().join("tests/fixtures/xcresult").join(name)).unwrap()
}

/// xcresulttool reading the copy kr makes of a bundle.
fn read(stdout: String) -> Recording {
    Recording {
        program: "xcrun".into(),
        args: [
            "xcresulttool",
            "get",
            "test-results",
            "tests",
            "--path",
            "bundle.xcresult",
            "--compact",
        ]
        .map(String::from)
        .to_vec(),
        status: Some(0),
        stdout,
        stderr: String::new(),
        timed_out: false,
        duration_ms: None,
    }
}

/// Every file under `dir` with its contents.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all.extend(snapshot(&path));
        } else {
            all.push((path.clone(), std::fs::read(&path).unwrap()));
        }
    }
    all.sort();
    all
}

struct Home(tempfile::TempDir);

impl Home {
    /// An e2e.sh run directory: scenario 0 passed; scenario 1 exited 65 with its
    /// enumeration next to the bundle.
    fn new() -> Self {
        let home = Self(tempfile::tempdir().unwrap());
        let e2e = home.evidence().join("batch6-gate/e2e-1");
        for (n, status) in [(0, "0"), (1, "65")] {
            let dir = e2e.join(n.to_string());
            std::fs::create_dir_all(dir.join("results.xcresult/Data")).unwrap();
            std::fs::write(dir.join("results.xcresult/Info.plist"), "<plist/>").unwrap();
            std::fs::write(dir.join("status"), status).unwrap();
        }
        std::fs::write(
            e2e.join("1/enabled-tests.txt"),
            "Plan KrSample\n\tKrSampleTests/GreetingTests/greetsByName()\n\tKrSampleTests/GreetingTests/greetsEmptyName()\n\tKrSampleUITests/LaunchUITests/testFlakyOnce()\n\tKrSampleUITests/LaunchUITests/testShowsGreeting()\n",
        )
        .unwrap();
        home
    }

    fn evidence(&self) -> PathBuf {
        self.0.path().join("v0-evidence")
    }

    fn rejudge(&self, recordings: &[Recording]) -> serde_json::Value {
        serde_json::from_str(&self.kr(recordings, true)).unwrap()
    }

    fn kr(&self, recordings: &[Recording], json: bool) -> String {
        let files: Vec<String> = recordings
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let p = self.0.path().join(format!("rec-{i}.json"));
                std::fs::write(&p, serde_json::to_string(r).unwrap()).unwrap();
                p.display().to_string()
            })
            .collect();
        let out = Command::cargo_bin("kr")
            .unwrap()
            .env_clear()
            .env("HOME", self.0.path())
            .env("KR_REPLAY", files.join(":"))
            .current_dir(self.0.path())
            .args(json.then_some("--json"))
            .args(["rejudge", "--evidence"])
            .arg(self.evidence())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{text}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        text.replace(&format!("{}/", self.evidence().display()), "")
    }
}

#[test]
fn evidence_bundles_are_read_once_on_a_copy_and_then_judged_from_the_cache() {
    let home = Home::new();
    let before = snapshot(&home.evidence());
    let out = home.rejudge(&[
        read(xcresult("sample-r2-tests.json")),
        read(xcresult("sample-r1-tests.json")),
    ]);
    assert_eq!(out["evidence"]["bundles"], 2);
    assert_eq!(out["evidence"]["read"], 2);
    assert_eq!(out["changed"], 0);
    // xcresulttool writes a database into a bundle it reads; the evidence stays as is.
    assert_eq!(snapshot(&home.evidence()), before);

    // No recordings: reading a bundle again would fail.
    let out = home.rejudge(&[]);
    assert_eq!(out["evidence"]["bundles"], 2);
    assert_eq!(out["evidence"]["read"], 0);
    assert_eq!(out["changed"], 0);
}

#[test]
fn a_cached_verdict_from_older_rules_is_listed_as_changed() {
    let home = Home::new();
    home.rejudge(&[
        read(xcresult("sample-r2-tests.json")),
        read(xcresult("sample-r1-tests.json")),
    ]);
    // As if an older kr had passed scenario 1 although testFlakyOnce failed.
    let cache = home.0.path().join(".local/state/kr/rejudge/evidence");
    for entry in std::fs::read_dir(&cache).unwrap() {
        let file = entry.unwrap().path();
        let text = std::fs::read_to_string(&file).unwrap();
        if text.contains("e2e-1/1/") {
            std::fs::write(&file, text.replace("\"failed\"", "\"passed\"")).unwrap();
        }
    }
    let out = home.rejudge(&[]);
    assert_eq!(out["changed"], 1, "{out}");
    let change = &out["changes"][0];
    assert_eq!(change["source"], "evidence");
    assert!(
        change["bundle"]
            .as_str()
            .unwrap()
            .ends_with("batch6-gate/e2e-1/1/results.xcresult"),
        "{change}"
    );
    assert_eq!(change["before"]["passed"], true);
    assert_eq!(change["after"]["passed"], false);
    assert_eq!(
        change["tests"],
        serde_json::json!([{
            "id": "KrSampleUITests/LaunchUITests/testFlakyOnce()",
            "before": "passed",
            "after": "failed",
        }])
    );
}

#[test]
fn a_verdict_that_now_passes_says_which_reason_went_away() {
    let home = Home::new();
    home.rejudge(&[
        read(xcresult("sample-r2-tests.json")),
        read(xcresult("sample-r1-tests.json")),
    ]);
    // As if an older rule had failed scenario 0 for a reason today's rules drop.
    let cache = home.0.path().join(".local/state/kr/rejudge/evidence");
    for entry in std::fs::read_dir(&cache).unwrap() {
        let file = entry.unwrap().path();
        let text = std::fs::read_to_string(&file).unwrap();
        if text.contains("e2e-1/0/") {
            let text = text.replace(
                "\"reasons\":[]",
                "\"reasons\":[\"an old rule disliked this\"]",
            );
            std::fs::write(&file, text).unwrap();
        }
    }
    assert_eq!(
        home.kr(&[], false).lines().collect::<Vec<_>>(),
        [
            "CHANGED: batch6-gate/e2e-1/0/results.xcresult: FAIL → PASS",
            "  was: an old rule disliked this",
            "rejudge: 0 items in 0 runs, 2 evidence bundles (0 read now); 1 verdict changed",
        ]
    );
}
