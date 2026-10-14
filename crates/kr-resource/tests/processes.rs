//! Tests that spawn processes. Kept apart from pool.rs, and run one at a time: a
//! process forked by one test thread briefly holds copies of another thread's lock
//! files (with their locks) until it execs.

use jiff::Timestamp;
use std::sync::Mutex;

use kr_resource::{Holder, Pool, Resource};

static SERIAL: Mutex<()> = Mutex::new(());

fn holder(task: &str) -> Holder {
    Holder {
        project: Some("krill".into()),
        task: task.into(),
        pid: std::process::id(),
        child_pid: None,
        started: Timestamp::now(),
    }
}

/// Writes a slot record as a crashed kr would leave it: no lock, but a command group.
fn leave_orphan(dir: &std::path::Path, pgid: u32) {
    let mut h = holder("crashed");
    h.pid = 999_999;
    h.child_pid = Some(pgid);
    std::fs::create_dir_all(dir.join("build")).unwrap();
    std::fs::write(
        dir.join("build/slot-1.lock"),
        serde_json::to_vec(&h).unwrap(),
    )
    .unwrap();
}

#[test]
fn a_slot_whose_command_group_still_runs_stays_held() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(dir.path());
    let mut orphan = std::process::Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    leave_orphan(dir.path(), orphan.id());

    let status = pool.status(Resource::Build, 1).unwrap();
    assert_eq!(status.holders.len(), 1);
    assert!(status.holders[0].orphan);
    assert!(pool.is_held(Resource::Build, 1));
    assert!(
        pool.try_acquire(Resource::Build, 1, &holder("next"))
            .unwrap()
            .is_none()
    );

    orphan.kill().unwrap();
    orphan.wait().unwrap();
    assert!(
        pool.try_acquire(Resource::Build, 1, &holder("next"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_slot_left_by_a_dead_holder_with_no_command_is_free() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(dir.path());
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    leave_orphan(dir.path(), gone.id());

    assert!(pool.status(Resource::Build, 1).unwrap().holders.is_empty());
    assert!(
        pool.try_acquire(Resource::Build, 1, &holder("next"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn the_current_process_parent_is_an_ancestor_and_a_child_is_not() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let parent = std::os::unix::process::parent_id();
    assert!(kr_resource::is_ancestor(parent));
    assert!(kr_resource::is_ancestor(std::process::id()));
    let mut child = std::process::Command::new("sleep")
        .arg("5")
        .spawn()
        .unwrap();
    assert!(!kr_resource::is_ancestor(child.id()));
    child.kill().unwrap();
    child.wait().unwrap();
}
