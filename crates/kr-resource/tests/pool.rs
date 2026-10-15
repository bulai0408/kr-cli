use std::time::Duration;

use jiff::Timestamp;
use kr_resource::{Holder, Pool, Resource, estimate_waits};

fn holder(task: &str) -> Holder {
    Holder {
        project: Some("krill".into()),
        task: task.into(),
        pid: std::process::id(),
        child_pid: None,
        started: Timestamp::now(),
    }
}

#[test]
fn an_acquired_slot_shows_its_holder_until_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(dir.path());

    let slot = pool
        .try_acquire(Resource::Build, 1, &holder("gate default"))
        .unwrap()
        .unwrap();
    assert_eq!(slot.index(), 1);

    let status = pool.status(Resource::Build, 1).unwrap();
    assert_eq!(status.holders.len(), 1);
    assert_eq!(status.holders[0].slot, 1);
    assert_eq!(
        status.holders[0].holder.as_ref().unwrap().task,
        "gate default"
    );
    assert!(pool.is_held(Resource::Build, 1));

    drop(slot);
    assert!(pool.status(Resource::Build, 1).unwrap().holders.is_empty());
    assert!(!pool.is_held(Resource::Build, 1));
}

#[test]
fn capacity_limits_concurrent_slots() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(dir.path());

    let a = pool.try_acquire(Resource::Sim, 2, &holder("a")).unwrap();
    let b = pool.try_acquire(Resource::Sim, 2, &holder("b")).unwrap();
    let c = pool.try_acquire(Resource::Sim, 2, &holder("c")).unwrap();

    assert_eq!((a.unwrap().index(), b.unwrap().index()), (1, 2));
    assert!(c.is_none());
}

#[test]
fn waiters_are_listed_in_arrival_order_and_leave_when_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(dir.path());
    let _held = pool
        .try_acquire(Resource::Build, 1, &holder("holder"))
        .unwrap()
        .unwrap();

    let first = pool.enqueue(Resource::Build, &holder("first")).unwrap();
    let second = pool.enqueue(Resource::Build, &holder("second")).unwrap();

    let tasks = |pool: &Pool| -> Vec<String> {
        pool.status(Resource::Build, 1)
            .unwrap()
            .waiters
            .into_iter()
            .map(|w| w.holder.task)
            .collect()
    };
    assert_eq!(tasks(&pool), ["first", "second"]);
    assert_eq!(first.position(1).unwrap(), 0);
    assert_eq!(second.position(1).unwrap(), 1);

    drop(first);
    assert_eq!(tasks(&pool), ["second"]);
}

#[test]
fn only_the_first_waiter_may_take_a_freed_slot() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(dir.path());
    let held = pool
        .try_acquire(Resource::Build, 1, &holder("holder"))
        .unwrap()
        .unwrap();
    let first = pool.enqueue(Resource::Build, &holder("first")).unwrap();
    let second = pool.enqueue(Resource::Build, &holder("second")).unwrap();

    assert!(first.try_acquire(1).unwrap().is_none());
    drop(held);

    assert!(
        second.try_acquire(1).unwrap().is_none(),
        "second must not jump the queue"
    );
    let slot = first.try_acquire(1).unwrap().unwrap();
    assert_eq!(slot.index(), 1);
}

#[test]
fn a_ticket_left_by_a_dead_waiter_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(dir.path());
    let queue = dir.path().join("build/queue");
    std::fs::create_dir_all(&queue).unwrap();
    // Nobody holds a lock on this ticket: its owner is gone.
    std::fs::write(
        queue.join("1-99999.json"),
        serde_json::to_string(&holder("ghost")).unwrap(),
    )
    .unwrap();

    assert!(pool.status(Resource::Build, 1).unwrap().waiters.is_empty());
    assert!(!queue.join("1-99999.json").exists());
}

#[test]
fn estimates_simulate_slots_freeing_up_in_order() {
    let min = |m: u64| Duration::from_secs(m * 60);
    // Two slots, held for 2 and 9 minutes; average hold 10 minutes; three waiters.
    let waits = estimate_waits(min(10), 2, &[min(2), min(9)], 3);
    assert_eq!(waits, vec![min(1), min(8), min(11)]);
}

#[test]
fn an_overdue_holder_is_expected_to_finish_now() {
    let waits = estimate_waits(Duration::from_secs(60), 1, &[Duration::from_secs(300)], 1);
    assert_eq!(waits, vec![Duration::ZERO]);
}

#[test]
fn free_slots_mean_no_wait() {
    let waits = estimate_waits(Duration::from_secs(60), 2, &[], 2);
    assert_eq!(waits, vec![Duration::ZERO, Duration::ZERO]);
}

#[test]
fn lanes_of_one_process_joining_the_queue_at_once_each_get_a_ticket() {
    // Krill #125: two lanes of one `kr run` joined the sim queue in the same
    // microsecond; the second ticket's rename failed with "No such file or directory".
    let dir = tempfile::tempdir().unwrap();
    let pool = Pool::new(dir.path());
    for _ in 0..20 {
        let barrier = std::sync::Barrier::new(8);
        let results: Vec<Result<(), String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|i| {
                    let (pool, barrier) = (&pool, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        pool.enqueue(Resource::Sim, &holder(&format!("lane {i}")))
                            .map(drop)
                            .map_err(|e| format!("{e:#}"))
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for r in results {
            r.unwrap();
        }
    }
}
