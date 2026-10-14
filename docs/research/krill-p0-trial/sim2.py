"""Discrete-event model of kr's simulator phase, faithful to run.rs:
lanes are started per device kind in name order (default, iPad); each lane takes a sim
slot FIFO, boots its device (BOOT s), pops items from its kind's queue (longest first)
and gives the slot back when that queue is empty. Durations come from real runs."""
import json, re, os, glob, sys, heapq
BOOT = 30.0

def model(dur, kinds, capacity, kind_order, boot=BOOT):
    """dur: item -> seconds; kinds: item -> kind. Returns makespan."""
    queues = {}
    for item in sorted(dur, key=lambda n: -dur[n]):
        queues.setdefault(kinds[item], []).append(item)
    devices = {"default": 2, "iPad": 1}
    lane_requests = []                                # FIFO order of slot requests
    for k in kind_order:
        if k in queues:
            lane_requests += [k] * min(capacity, devices[k], len(queues[k]))
    # Simulate: slots free at times; a lane gets the earliest free slot in FIFO order,
    # works until its kind's queue is empty (shared with sibling lanes), then frees it.
    t_free = [0.0] * capacity
    heapq.heapify(t_free)
    # Lanes of one kind run concurrently once they have slots; model per-kind lane timelines.
    lane_end = []
    active = {}                                        # kind -> heap of lane free times
    for k in lane_requests:
        start = heapq.heappop(t_free)
        active.setdefault(k, []).append(start + boot)
        lane_end.append((k, len(active[k]) - 1))
        # Lane slot is released when its kind's queue is done: computed after assignment.
        heapq.heappush(t_free, float("inf"))         # placeholder, fixed below
    # Assign items to lanes of each kind (earliest free lane first).
    finish = {}
    for k, lanes in active.items():
        h = [(t, i) for i, t in enumerate(lanes)]
        heapq.heapify(h)
        for item in queues[k]:
            t, i = heapq.heappop(h); heapq.heappush(h, (t + dur[item], i))
        finish[k] = max(t for t, _ in h)
    return max(finish.values()), finish

def simulate_fifo(dur, kinds, capacity, kind_order, boot=BOOT):
    """Proper slot accounting: a lane holds its slot until its kind's queue is empty."""
    queues = {}
    for item in sorted(dur, key=lambda n: -dur[n]):
        queues.setdefault(kinds[item], []).append(item)
    devices = {"default": 2, "iPad": 1}
    requests = [k for k in kind_order if k in queues for _ in range(min(capacity, devices[k], len(queues[k])))]
    slots = [0.0] * capacity                           # time each slot becomes free
    pending = {k: list(q) for k, q in queues.items()}
    lane_free = {}                                     # kind -> list of [time]
    # Event loop over lanes in FIFO order; a lane starts when a slot is free.
    import itertools
    t_now = 0.0
    running = []                                       # (end_time, kind, lane_id)
    lane_id = itertools.count()
    req = list(requests)
    lanes = {}                                         # id -> [kind, free_at]
    events = []
    def start_lane(t, k):
        lid = next(lane_id); lanes[lid] = [k, t + boot]
    # Greedy: process time forward.
    free_slots = capacity
    time = 0.0
    while req and free_slots:
        start_lane(0.0, req.pop(0)); free_slots -= 1
    done_at = 0.0
    while lanes:
        # The lane that is free earliest takes the next item of its kind, or quits.
        lid = min(lanes, key=lambda i: lanes[i][1])
        k, t = lanes[lid]
        if pending[k]:
            item = pending[k].pop(0); lanes[lid][1] = t + dur[item]
        else:
            del lanes[lid]; done_at = max(done_at, t)
            if req:
                start_lane(t, req.pop(0))
    return done_at

def kr_run(run_id):
    dur, kinds = {}, {}
    for f in sorted(glob.glob(os.path.expanduser("~/.local/state/kr/events/*.ndjson"))):
        for line in open(f):
            e = json.loads(line)
            if e["run_id"] == run_id and e["type"] == "item_end" and e["fields"].get("device"):
                d = e["fields"]["device"]; dur[e["fields"]["item"]] = e["fields"]["duration_ms"] / 1000
                kinds[e["fields"]["item"]] = "iPad" if "iPad" in d else "default"
    return dur, kinds

def old_batch6():
    EV = "/Users/cyh/orca/workspaces/Krill/v0-evidence/batch6-gate"
    log = open(f"{EV}/e2e.log").read(); dur = {}
    for name, path in re.findall(r"^(?:PASS|FAIL): ([\w-]+)\nxcresult: (\S+)/results\.xcresult", log, re.M):
        p = f"{EV}/{path.split('/build/')[1]}/summary.json"
        if os.path.exists(p):
            s = json.load(open(p)); dur[name] = s["finishTime"] - s["startTime"]
    dur.setdefault("module-watchlist", 120.0)
    kinds = {n: ("iPad" if n in ("ipad-layout", "shelf-share-ipad") else "default") for n in dur}
    return dur, kinds

if __name__ == "__main__":
    dur, kinds = old_batch6()
    print(f"输入：batch6-gate 实测 {len(dur)} 个场景，合计 {sum(dur.values())/60:.1f} 分钟")
    print("| 方案 | 预计总耗时（分钟） |\n|---|---|")
    for label, cap, order in [("kr 现状：sim=2，先 default 后 iPad", 2, ["default", "iPad"]),
                              ("改：sim=2，先 iPad 后 default", 2, ["iPad", "default"]),
                              ("sim=1", 1, ["default", "iPad"]),
                              ("sim=3（机器允许时）", 3, ["default", "iPad"])]:
        print(f"| {label} | {simulate_fifo(dur, kinds, cap, order)/60:.1f} |")
