//! The simulator phase of `kr run`: lanes, their work queues and the `sim` slots
//! (ADR 0005, 0010). A real run and `kr simulate` drive the same code; only the
//! [`Executor`] (what taking a slot, leasing a device and running an item do) and the
//! [`Clock`] differ.
//!
//! Each lane is one leased simulator of a device kind. Lanes queue for a `sim` slot one
//! after another in a fixed order, iPad lanes first; a lane with a slot boots its
//! device, takes items from its kind's queue (shared with the other lanes of the kind)
//! until the queue is empty, then gives the device and the slot back.

use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Index of a lane in [`Lanes::order`].
pub type LaneId = usize;

/// The lanes of a run: the device kind of each lane in the order they queue for a slot,
/// and the work queue of each kind.
#[derive(Debug, Clone)]
pub struct Lanes<I> {
    pub order: Vec<String>,
    pub queues: BTreeMap<String, VecDeque<I>>,
}

/// In which order the lanes of different kinds queue for their slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// iPad kinds first, then the others by name: what `kr run` does. Krill's iPad
    /// items are the longest; starting them first shortens the gate (ADR 0010).
    IpadFirst,
    /// By kind name only: what `kr run` did before (as far as thread timing allowed).
    Name,
}

/// One queue per device kind, keeping the order of `items` (longest first, see
/// [`crate::longest_first`]).
pub fn queues<I>(items: Vec<I>, kind: impl Fn(&I) -> String) -> BTreeMap<String, VecDeque<I>> {
    let mut queues: BTreeMap<String, VecDeque<I>> = BTreeMap::new();
    for item in items {
        queues.entry(kind(&item)).or_default().push_back(item);
    }
    queues
}

/// The lanes for `queues`: per kind, as many as there are slots, devices and items
/// (at least one). `devices` counts the usable devices of each kind; `ipad` tells
/// whether a kind is an iPad.
pub fn lanes<I>(
    queues: BTreeMap<String, VecDeque<I>>,
    devices: &BTreeMap<String, usize>,
    capacity: usize,
    order: Order,
    ipad: impl Fn(&str) -> bool,
) -> Lanes<I> {
    let mut kinds: Vec<&String> = queues.keys().collect();
    if order == Order::IpadFirst {
        kinds.sort_by_key(|kind| !ipad(kind));
    }
    let mut lanes = Vec::new();
    for kind in kinds {
        let count = capacity
            .min(devices.get(kind).copied().unwrap_or(0))
            .min(queues[kind].len())
            .max(1);
        lanes.extend(std::iter::repeat_n(kind.clone(), count));
    }
    Lanes {
        order: lanes,
        queues,
    }
}

/// What the scheduler asks the executor to do. Each action ends with one [`Done`].
#[derive(Debug)]
pub enum Action<I> {
    /// Queue for a `sim` slot.
    Take(LaneId),
    /// Lease and boot a device of the lane's kind (the lane holds a slot).
    Lease(LaneId),
    /// Run an item on the lane's device.
    Run(LaneId, I),
    /// Shut the device down and give it and the slot back; `ok` is false after an error.
    Release(LaneId, bool),
    /// Give the slot back without having leased a device: nothing was left to do.
    GiveBack(LaneId),
}

/// How an action ended.
#[derive(Debug)]
pub enum Done<O> {
    Taken(LaneId),
    Leased(LaneId),
    /// An item ran; `None` when a stop signal ended it midway.
    Ran(LaneId, Option<O>),
    Released(LaneId),
    /// The action failed. A lane that fails while taking a slot or leasing holds
    /// nothing afterwards; one that fails running an item still holds its device.
    Failed(LaneId, anyhow::Error),
}

pub trait Executor {
    type Item: Clone;
    type Output;
    /// Starts an action; it must not wait for it to end.
    fn start(&mut self, action: Action<Self::Item>);
    /// Waits for the next action to end.
    fn wait(&mut self) -> Done<Self::Output>;
    /// True after a stop signal: no new item starts.
    fn stopped(&self) -> bool;
    /// The scheduler is about to run `item` on `lane`; `queue` is its kind's queue before
    /// the pick, `item` first. For the record only (ADR 0010).
    fn chose(&mut self, _lane: LaneId, _queue: &VecDeque<Self::Item>) {}
}

pub trait Clock {
    /// Time since the start of the run.
    fn now(&self) -> Duration;
}

/// Real time.
pub struct WallClock(Instant);

impl WallClock {
    pub fn start() -> Self {
        Self(Instant::now())
    }
}

impl Clock for WallClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
}

/// Time that [`Virtual`] moves forward. Clones share the time.
#[derive(Debug, Clone, Default)]
pub struct VirtualClock(Rc<Cell<Duration>>);

impl Clock for VirtualClock {
    fn now(&self) -> Duration {
        self.0.get()
    }
}

/// When each lane queued, got its slot and ended, and when each item ran.
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    pub lanes: Vec<LaneSpan>,
    pub items: Vec<ItemSpan>,
}

#[derive(Debug, Clone)]
pub struct LaneSpan {
    pub kind: String,
    /// `None` for a lane that never queued (its kind's queue was empty by its turn).
    pub requested: Option<Duration>,
    pub granted: Option<Duration>,
    pub ended: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct ItemSpan {
    pub lane: LaneId,
    pub start: Duration,
    pub end: Duration,
}

impl Timeline {
    /// From the first item's start to the last item's end: the time simulators were in
    /// use, the same measure as in recorded runs.
    pub fn makespan(&self) -> Option<Duration> {
        let start = self.items.iter().map(|i| i.start).min()?;
        let end = self.items.iter().map(|i| i.end).max()?;
        Some(end - start)
    }

    /// Time lanes waited for a slot, as a share of the time from queueing to the end
    /// of each lane that got one.
    pub fn queue_share(&self) -> f64 {
        let (mut waited, mut total) = (Duration::ZERO, Duration::ZERO);
        for lane in &self.lanes {
            if let (Some(r), Some(g), Some(e)) = (lane.requested, lane.granted, lane.ended) {
                waited += g - r;
                total += e - r;
            }
        }
        if total.is_zero() {
            0.0
        } else {
            waited.as_secs_f64() / total.as_secs_f64()
        }
    }
}

/// How the simulator phase ended.
#[derive(Debug)]
pub struct Outcome<I, O> {
    /// Results of the items that ran, in the order they ended.
    pub done: Vec<O>,
    /// Items no lane took, per kind in lane order: after a stop signal, or when every
    /// lane of the kind failed.
    pub left: Vec<(String, Vec<I>)>,
    /// Lane errors with the lane's kind, in the order they happened.
    pub errors: Vec<(String, anyhow::Error)>,
    pub timeline: Timeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Leasing,
    Running(Duration),
    Releasing,
    Over,
}

/// Runs the lanes to the end. An item's error is the executor's to turn into a result;
/// a lane that fails ends, and the other lanes of its kind take over its queue.
pub fn drive<E: Executor>(
    lanes: Lanes<E::Item>,
    exec: &mut E,
    clock: &dyn Clock,
) -> Outcome<E::Item, E::Output> {
    let mut d = Driver {
        timeline: Timeline {
            lanes: lanes
                .order
                .iter()
                .map(|kind| LaneSpan {
                    kind: kind.clone(),
                    requested: None,
                    granted: None,
                    ended: None,
                })
                .collect(),
            items: Vec::new(),
        },
        state: vec![State::Over; lanes.order.len()],
        order: lanes.order,
        queues: lanes.queues,
        next: 0,
        in_flight: 0,
        exec,
        clock,
    };
    let mut done = Vec::new();
    let mut errors = Vec::new();
    d.queue_next();
    while d.in_flight > 0 {
        let event = d.exec.wait();
        let now = d.clock.now();
        match event {
            Done::Taken(lane) => {
                d.timeline.lanes[lane].granted = Some(now);
                // Other lanes may have finished the work while this one waited.
                if !d.has_work(lane) || d.exec.stopped() {
                    d.state[lane] = State::Releasing;
                    d.exec.start(Action::GiveBack(lane));
                } else {
                    d.state[lane] = State::Leasing;
                    d.exec.start(Action::Lease(lane));
                }
                d.queue_next();
            }
            Done::Leased(lane) => d.next_item(lane),
            Done::Ran(lane, output) => {
                if let State::Running(start) = d.state[lane] {
                    d.timeline.items.push(ItemSpan {
                        lane,
                        start,
                        end: now,
                    });
                }
                done.extend(output);
                d.next_item(lane);
            }
            Done::Released(lane) => d.end(lane, now),
            Done::Failed(lane, error) => {
                errors.push((d.order[lane].clone(), error));
                match d.state[lane] {
                    // It still holds its device.
                    State::Running(_) => {
                        d.state[lane] = State::Releasing;
                        d.exec.start(Action::Release(lane, false));
                    }
                    State::Waiting => {
                        d.state[lane] = State::Over;
                        d.in_flight -= 1;
                        d.queue_next();
                    }
                    State::Leasing | State::Releasing | State::Over => d.end(lane, now),
                }
            }
        }
    }
    let mut left = Vec::new();
    for kind in &d.order {
        if let Some(queue) = d.queues.remove(kind)
            && !queue.is_empty()
        {
            left.push((kind.clone(), queue.into()));
        }
    }
    Outcome {
        done,
        left,
        errors,
        timeline: d.timeline,
    }
}

struct Driver<'e, E: Executor> {
    order: Vec<String>,
    queues: BTreeMap<String, VecDeque<E::Item>>,
    state: Vec<State>,
    timeline: Timeline,
    /// The next lane to queue for a slot.
    next: LaneId,
    /// Lanes that have not ended.
    in_flight: usize,
    exec: &'e mut E,
    clock: &'e dyn Clock,
}

impl<E: Executor> Driver<'_, E> {
    /// Queues the next lane that has work for a slot. One lane queues at a time, so
    /// the slots go to the lanes in order.
    fn queue_next(&mut self) {
        while self.next < self.order.len() {
            let lane = self.next;
            self.next += 1;
            if !self.has_work(lane) || self.exec.stopped() {
                continue;
            }
            self.timeline.lanes[lane].requested = Some(self.clock.now());
            self.state[lane] = State::Waiting;
            self.in_flight += 1;
            self.exec.start(Action::Take(lane));
            return;
        }
    }

    fn has_work(&self, lane: LaneId) -> bool {
        self.queues
            .get(&self.order[lane])
            .is_some_and(|q| !q.is_empty())
    }

    /// Starts the lane's next item; releases the lane when there is none or after a
    /// stop signal.
    fn next_item(&mut self, lane: LaneId) {
        let item = if self.exec.stopped() {
            None
        } else {
            let queue = self.queues.get_mut(&self.order[lane]);
            if let Some(queue) = &queue
                && !queue.is_empty()
            {
                self.exec.chose(lane, queue);
            }
            queue.and_then(VecDeque::pop_front)
        };
        match item {
            Some(item) => {
                self.state[lane] = State::Running(self.clock.now());
                self.exec.start(Action::Run(lane, item));
            }
            None => {
                self.state[lane] = State::Releasing;
                self.exec.start(Action::Release(lane, true));
            }
        }
    }

    fn end(&mut self, lane: LaneId, now: Duration) {
        self.timeline.lanes[lane].ended = Some(now);
        self.state[lane] = State::Over;
        self.in_flight -= 1;
    }
}

/// An executor in virtual time for `kr simulate` and `kr plan`: `capacity` slots taken
/// first come first served, a boot per lease, and each item taking `duration`.
pub struct Virtual<I> {
    clock: VirtualClock,
    free: usize,
    waiting: VecDeque<LaneId>,
    boot: Duration,
    duration: Box<dyn Fn(&I) -> Duration>,
    /// (time, sequence, event): the earliest first, ties in the order they came.
    pending: Vec<(Duration, u64, Done<I>)>,
    seq: u64,
}

impl<I> Virtual<I> {
    pub fn new(
        clock: VirtualClock,
        capacity: usize,
        boot: Duration,
        duration: impl Fn(&I) -> Duration + 'static,
    ) -> Self {
        Self {
            clock,
            free: capacity,
            waiting: VecDeque::new(),
            boot,
            duration: Box::new(duration),
            pending: Vec::new(),
            seq: 0,
        }
    }

    fn at(&mut self, after: Duration, event: Done<I>) {
        self.seq += 1;
        self.pending
            .push((self.clock.now() + after, self.seq, event));
    }

    fn free_slot(&mut self) {
        match self.waiting.pop_front() {
            Some(lane) => self.at(Duration::ZERO, Done::Taken(lane)),
            None => self.free += 1,
        }
    }
}

impl<I: Clone> Executor for Virtual<I> {
    type Item = I;
    type Output = I;

    fn start(&mut self, action: Action<I>) {
        match action {
            Action::Take(lane) => {
                if self.free > 0 {
                    self.free -= 1;
                    self.at(Duration::ZERO, Done::Taken(lane));
                } else {
                    self.waiting.push_back(lane);
                }
            }
            Action::Lease(lane) => self.at(self.boot, Done::Leased(lane)),
            Action::Run(lane, item) => {
                let took = (self.duration)(&item);
                self.at(took, Done::Ran(lane, Some(item)));
            }
            Action::Release(lane, _) | Action::GiveBack(lane) => {
                self.free_slot();
                self.at(Duration::ZERO, Done::Released(lane));
            }
        }
    }

    fn wait(&mut self) -> Done<I> {
        let next = self
            .pending
            .iter()
            .enumerate()
            .min_by_key(|(_, (time, seq, _))| (*time, *seq))
            .map(|(i, _)| i)
            .expect("the scheduler waits only for started actions");
        let (time, _, event) = self.pending.swap_remove(next);
        self.clock.0.set(time);
        event
    }

    fn stopped(&self) -> bool {
        false
    }
}

/// Simulates the lanes in virtual time; returns the timeline and the items in the
/// order they ended.
pub fn simulate<I: Clone + 'static>(
    lanes: Lanes<I>,
    capacity: usize,
    boot: Duration,
    duration: impl Fn(&I) -> Duration + 'static,
) -> (Timeline, Vec<I>) {
    let clock = VirtualClock::default();
    let mut exec = Virtual::new(clock.clone(), capacity, boot, duration);
    let outcome = drive(lanes, &mut exec, &clock);
    (outcome.timeline, outcome.done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// (name, kind, seconds), longest first within each kind.
    fn krill_like() -> Vec<(&'static str, &'static str, u64)> {
        vec![
            ("ipad-layout", "iPad", 400),
            ("shelf-share-ipad", "iPad", 100),
            ("modules", "default", 300),
            ("ratings", "default", 200),
            ("search", "default", 100),
            ("trakt", "default", 100),
        ]
    }

    fn run(order: Order, capacity: usize) -> (Timeline, Vec<&'static str>) {
        let items = krill_like();
        let queues = queues(items, |i| i.1.to_owned());
        let devices = BTreeMap::from([("default".to_owned(), 2), ("iPad".to_owned(), 1)]);
        let lanes = lanes(queues, &devices, capacity, order, |k| k == "iPad");
        let (timeline, done) = simulate(lanes, capacity, secs(10), |i| secs(i.2));
        (timeline, done.into_iter().map(|i| i.0).collect())
    }

    #[test]
    fn ipad_lanes_queue_first_and_the_last_lane_waits_for_a_slot() {
        let (timeline, _) = run(Order::IpadFirst, 2);
        let kinds: Vec<&str> = timeline.lanes.iter().map(|l| l.kind.as_str()).collect();
        assert_eq!(kinds, ["iPad", "default", "default"]);
        // iPad: 10 boot + 500; default 1: 10 + 300 + 100 + 100 + ... shares with lane 3.
        // Lane 3 gets the iPad lane's slot at 510, boots until 520.
        assert_eq!(timeline.lanes[2].granted, Some(secs(510)));
        // Lane 2 runs modules (10–310), ratings (310–510), search (510–610); lane 3
        // runs trakt (520–620).
        assert_eq!(timeline.makespan(), Some(secs(610)));
    }

    #[test]
    fn by_name_the_ipad_lane_waits_until_an_iphone_lane_ends() {
        let (timeline, done) = run(Order::Name, 2);
        let kinds: Vec<&str> = timeline.lanes.iter().map(|l| l.kind.as_str()).collect();
        assert_eq!(kinds, ["default", "default", "iPad"]);
        // default lanes: modules 10–310, trakt 310–410 | ratings 10–210, search 210–310.
        // The second lane ends at 310 (queue empty); iPad boots 310–320, runs 500.
        assert_eq!(timeline.makespan(), Some(secs(810)));
        assert_eq!(done.last(), Some(&"shelf-share-ipad"));
    }

    #[test]
    fn a_lane_that_cannot_get_a_device_leaves_its_items_to_the_others() {
        struct FailingLease(Virtual<u64>);
        impl Executor for FailingLease {
            type Item = u64;
            type Output = u64;
            fn start(&mut self, action: Action<u64>) {
                match action {
                    Action::Lease(0) => {
                        self.0.free_slot();
                        self.0.at(
                            Duration::ZERO,
                            Done::Failed(0, anyhow::anyhow!("no device")),
                        );
                    }
                    a => self.0.start(a),
                }
            }
            fn wait(&mut self) -> Done<u64> {
                self.0.wait()
            }
            fn stopped(&self) -> bool {
                false
            }
        }
        let clock = VirtualClock::default();
        let mut exec = FailingLease(Virtual::new(clock.clone(), 2, secs(0), |s| secs(*s)));
        let lanes = Lanes {
            order: vec!["default".into(), "default".into()],
            queues: BTreeMap::from([("default".to_owned(), VecDeque::from([5, 5, 5]))]),
        };
        let outcome = drive(lanes, &mut exec, &clock);
        assert_eq!(outcome.done.len(), 3);
        assert!(outcome.left.is_empty());
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.timeline.makespan(), Some(secs(15)));
    }

    #[test]
    fn lanes_are_capped_by_slots_devices_and_items() {
        let queues = queues(
            vec![("a", "default"), ("b", "default"), ("c", "iPad")],
            |i| i.1.to_owned(),
        );
        let devices = BTreeMap::from([("default".to_owned(), 3), ("iPad".to_owned(), 1)]);
        let lanes = lanes(queues, &devices, 3, Order::IpadFirst, |k| k == "iPad");
        assert_eq!(lanes.order, ["iPad", "default", "default"]);
    }
}
