//! The bridge to the old `/tmp/native-build.lock` protocol, on temp directories.

use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use kr_resource::legacy::{LegacyLock, Owner};

// These tests spawn processes and use process-wide reentrancy state: one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn bridge(dir: &Path) -> LegacyLock {
    LegacyLock::new(
        dir.join("native-build.lock"),
        dir.join("native-build.queue"),
    )
    .with_poll(Duration::from_millis(20))
}

fn old_script_takes(dir: &Path, pid: u32) -> bool {
    let lock = dir.join("native-build.lock");
    if std::fs::create_dir(&lock).is_err() {
        return false;
    }
    std::fs::write(
        lock.join("owner"),
        format!("old-e2e session {pid} 2026-10-14T00:00:00Z\n"),
    )
    .unwrap();
    true
}

#[test]
fn while_kr_holds_it_an_old_script_cannot_take_it() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let guard = bridge(dir.path()).acquire("kr-check", |_| {}).unwrap();

    assert!(!old_script_takes(dir.path(), std::process::id()));
    let owner = std::fs::read_to_string(dir.path().join("native-build.lock/owner")).unwrap();
    let fields: Vec<&str> = owner.split_whitespace().collect();
    assert_eq!(fields[0], "kr-check");
    assert_eq!(fields[2], std::process::id().to_string());

    drop(guard);
    assert!(!dir.path().join("native-build.lock").exists());
    assert_eq!(
        std::fs::read_dir(dir.path().join("native-build.queue"))
            .unwrap()
            .count(),
        0
    );
    assert!(old_script_takes(dir.path(), std::process::id()));
}

#[test]
fn kr_waits_while_an_old_script_holds_it() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let mut script = Command::new("sleep").arg("30").spawn().unwrap();
    assert!(old_script_takes(dir.path(), script.id()));

    let lock = bridge(dir.path());
    let path = dir.path().to_owned();
    let waiter = std::thread::spawn(move || {
        let mut seen: Vec<Option<Owner>> = Vec::new();
        let guard = lock
            .acquire("kr-run", |owner| seen.push(owner.cloned()))
            .unwrap();
        (guard.is_owned(), seen)
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(!waiter.is_finished(), "kr must wait for the old script");

    // The old script finishes normally.
    std::fs::remove_file(path.join("native-build.lock/owner")).unwrap();
    std::fs::remove_dir(path.join("native-build.lock")).unwrap();
    let (owned, seen) = waiter.join().unwrap();
    assert!(owned);
    assert_eq!(seen[0].as_ref().unwrap().task, "old-e2e");
    script.kill().unwrap();
    script.wait().unwrap();
}

#[test]
fn a_lock_whose_owner_died_is_taken_over() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let mut dead = Command::new("true").spawn().unwrap();
    dead.wait().unwrap();
    assert!(old_script_takes(dir.path(), dead.id()));

    let guard = bridge(dir.path()).acquire("kr-check", |_| {}).unwrap();
    assert!(guard.is_owned());
}

#[test]
fn an_ownerless_lock_is_stale_only_after_a_minute() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let lock_dir = dir.path().join("native-build.lock");
    std::fs::create_dir(&lock_dir).unwrap();

    // Fresh and ownerless: someone is between mkdir and writing owner. Wait.
    let fresh = bridge(dir.path())
        .with_max_wait(Duration::from_millis(100))
        .acquire("kr", |_| {});
    assert!(fresh.is_err());

    let old = filetime(&lock_dir, "202610140000");
    assert!(old);
    assert!(bridge(dir.path()).acquire("kr", |_| {}).unwrap().is_owned());
}

fn filetime(path: &Path, stamp: &str) -> bool {
    Command::new("touch")
        .args(["-t", stamp])
        .arg(path)
        .status()
        .unwrap()
        .success()
}

#[test]
fn queue_files_of_dead_waiters_are_removed_and_order_is_kept() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let queue = dir.path().join("native-build.queue");
    std::fs::create_dir_all(&queue).unwrap();
    // An older ticket from a dead process must not block us.
    std::fs::write(queue.join("1-old-e2e-999999"), "").unwrap();

    let guard = bridge(dir.path()).acquire("kr", |_| {}).unwrap();
    assert!(guard.is_owned());
    assert!(!queue.join("1-old-e2e-999999").exists());
}

#[test]
fn an_older_live_waiter_goes_first() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let queue = dir.path().join("native-build.queue");
    std::fs::create_dir_all(&queue).unwrap();
    let mut other = Command::new("sleep").arg("30").spawn().unwrap();
    std::fs::write(queue.join(format!("1-old-e2e-{}", other.id())), "").unwrap();

    let waited = bridge(dir.path())
        .with_max_wait(Duration::from_millis(100))
        .acquire("kr", |_| {});
    assert!(waited.is_err(), "the older waiter is first in line");
    other.kill().unwrap();
    other.wait().unwrap();
}

#[test]
fn one_process_takes_it_once_for_several_slots() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let lock = bridge(dir.path());
    let build = lock.acquire("kr-build", |_| {}).unwrap();
    let sim = lock.acquire("kr-sim", |_| {}).unwrap();
    assert!(build.is_owned() && !sim.is_owned());

    drop(build);
    assert!(
        dir.path().join("native-build.lock").exists(),
        "sim still needs it"
    );
    drop(sim);
    assert!(!dir.path().join("native-build.lock").exists());
}

#[test]
fn a_lock_held_by_an_ancestor_counts_as_held() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    // e.g. e2e.sh holds the lock and calls kr: the owner is our parent.
    assert!(old_script_takes(
        dir.path(),
        std::os::unix::process::parent_id()
    ));

    let guard = bridge(dir.path())
        .with_max_wait(Duration::ZERO)
        .acquire("kr", |_| {})
        .unwrap();
    assert!(!guard.is_owned());
    drop(guard);
    assert!(
        dir.path().join("native-build.lock/owner").exists(),
        "the ancestor's lock stays"
    );
}

#[test]
fn threads_of_one_process_acquiring_together_share_the_lock() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let lock = bridge(dir.path()).with_max_wait(Duration::from_secs(2));
    let start = std::time::Instant::now();
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| lock.acquire("lane-a", |_| {}).unwrap());
        let b = s.spawn(|| lock.acquire("lane-b", |_| {}).unwrap());
        (a.join().unwrap(), b.join().unwrap())
    });
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    );
    assert!(
        a.is_owned() != b.is_owned(),
        "exactly one of them took it: {a:?} {b:?}"
    );
    drop(a);
    assert!(dir.path().join("native-build.lock").exists());
    drop(b);
    assert!(!dir.path().join("native-build.lock").exists());
}

#[test]
fn rewriting_the_owner_pid_never_leaves_a_partial_file() {
    let _s = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let guard = bridge(dir.path()).acquire("kr", |_| {}).unwrap();
    let owner = dir.path().join("native-build.lock/owner");
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            for pid in 0..2000 {
                kr_resource::legacy::set_owner_pid(1000 + pid);
            }
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            let text = std::fs::read_to_string(&owner).unwrap_or_default();
            assert_eq!(text.split_whitespace().count(), 4, "{text:?}");
        }
    });
    drop(guard);
}
