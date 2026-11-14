#!/usr/bin/env python3
"""#69b: Conflux cold builds, capacity 1 (serial) vs 2 (parallel), measured only on a quiet machine.

  calibrate-conflux-build.py run --conflux REPO --out DIR --not-before HH:MM --deadline HH:MM
  calibrate-conflux-build.py run --dry-run --out DIR

`run` waits for --not-before, prepares a detached worktree of origin/main under /tmp, then polls
until the machine has been quiet for --quiet-seconds. Only then it takes
`kr lock build --task kr69b-calibration -- <this script> measure DIR`. The measure phase runs
A/B/A/B cold builds (serial, parallel, serial, parallel) inside a bounded window. The worktree
and DerivedData are removed on every exit path, then the analyzer writes DIR/summary.json and a
redacted DIR/public/.

Exit codes: 0 measured, 1 failed, 3 not quiet before the deadline.
"""
import argparse
import datetime as dt
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
TASK = 'kr69b-calibration'
WARN, CRITICAL = 2, 4
MIN_FREE_DISK_GIB = 8
RESOURCES = ('build', 'sim', 'light')
PHASES = [('A1', 'serial'), ('B1', 'parallel'), ('A2', 'serial'), ('B2', 'parallel')]
EXIT_MEASURED, EXIT_FAILED, EXIT_NOT_QUIET = 0, 1, 3


def resolve_window(now, not_before, deadline):
    """Next deadline after now, and the not-before that precedes it by less than a day."""
    dl = dt.datetime.combine(now.date(), deadline)
    if dl <= now:
        dl += dt.timedelta(days=1)
    nb = dt.datetime.combine(dl.date(), not_before)
    if nb >= dl:
        nb -= dt.timedelta(days=1)
    return nb, dl


def busy_rows(queue, own_task):
    """Holders and waiters of other tasks on build/sim/light. None (kr failed) counts as busy."""
    if queue is None:
        return ['kr-queue:unavailable']
    rows = []
    for r in queue.get('resources', []):
        if r.get('resource') not in RESOURCES:
            continue
        for kind in ('holders', 'waiters', 'load_waiters'):
            for entry in r.get(kind) or []:
                task = entry.get('task', '?')
                if task != own_task:
                    rows.append(f"{r['resource']}:{kind}:{task}")
    return rows


class QuietGate:
    """Passes once every sample for quiet_seconds has had no kr work, low outside CPU, normal pressure."""

    def __init__(self, quiet_seconds, threshold, pressure_limit=WARN):
        self.quiet_seconds = quiet_seconds
        self.threshold = threshold
        self.pressure_limit = pressure_limit
        self.quiet_since = None

    def reasons(self, s):
        out = [f'kr:{r}' for r in s['queue_busy']]
        outside = s.get('outside_pct')
        if outside is None:
            out.append('cpu:no-data')
        elif outside >= self.threshold:
            out.append(f'cpu:{outside:.1f}%')
        if s['pressure'] >= self.pressure_limit:
            out.append(f"pressure:{s['pressure']}")
        return out

    def feed(self, s):
        reasons = self.reasons(s)
        if reasons:
            self.quiet_since = None
        elif self.quiet_since is None:
            self.quiet_since = s['t']
        s['gate'] = {'quiet': not reasons, 'reasons': reasons}
        return self.quiet_since is not None and s['t'] - self.quiet_since >= self.quiet_seconds


def parse_cputime(text):
    days, _, rest = text.rpartition('-')
    seconds = 0.0
    for part in rest.split(':'):
        seconds = seconds * 60 + float(part)
    return seconds + (int(days) * 86400 if days else 0)


def ps_snapshot():
    out = subprocess.run(['ps', '-A', '-o', 'pid=,ppid=,time=,comm='], capture_output=True, text=True).stdout
    snap = {}
    for line in out.splitlines():
        parts = line.split(None, 3)
        if len(parts) == 4:
            snap[int(parts[0])] = (int(parts[1]), parse_cputime(parts[2]), parts[3])
    return snap


def descendants(snap, root):
    children = {}
    for pid, (ppid, _, _) in snap.items():
        children.setdefault(ppid, []).append(pid)
    tree, todo = set(), [root]
    while todo:
        pid = todo.pop()
        if pid in tree:
            continue
        tree.add(pid)
        todo.extend(children.get(pid, []))
    return tree


def attribute(prev, cur, seconds, ncpu, own_root, builds):
    """CPU per group as percent of the whole machine, from cputime deltas between two ps snapshots.

    A pid absent from prev started inside the interval, so all its cputime counts. Processes that
    exit between snapshots are invisible; the caller reports that gap as unattributed.
    """
    scale = 100 / (seconds * ncpu)
    own = descendants(cur, own_root)
    trees = {label: descendants(cur, pid) for label, pid in builds.items()}
    out = {'own_pct': 0.0, 'other_pct': 0.0, 'builds_pct': {label: 0.0 for label in builds}}
    others = []
    for pid, (_, cpu, name) in cur.items():
        before = prev.get(pid)
        delta = max(cpu - before[1], 0.0) if before and before[2] == name else cpu
        pct = delta * scale
        if pid in own:
            out['own_pct'] += pct
            for label, tree in trees.items():
                if pid in tree:
                    out['builds_pct'][label] += pct
        else:
            out['other_pct'] += pct
            others.append((pct, os.path.basename(name.strip('()'))))
    others.sort(reverse=True)
    out['top_other'] = [[name, round(pct, 2)] for pct, name in others[:5]]
    for key in ('own_pct', 'other_pct'):
        out[key] = round(out[key], 2)
    out['builds_pct'] = {k: round(v, 2) for k, v in out['builds_pct'].items()}
    return out


def sysctl(*names):
    # Keyed, not positional: sysctl skips names it does not know.
    out = subprocess.run(['sysctl', *names], capture_output=True, text=True).stdout.splitlines()
    return dict(line.split(': ', 1) for line in out if ': ' in line)


class Sampler:
    """One record per interval: whole-machine CPU from top, attribution from ps, memory, kr queue."""

    def __init__(self, own_root, own_task, interval):
        self.own_root, self.own_task, self.interval = own_root, own_task, interval
        self.ncpu = int(sysctl('hw.ncpu')['hw.ncpu'])
        self.page = int(sysctl('hw.pagesize')['hw.pagesize'])
        self.latest_cpu = None
        self.top = subprocess.Popen(['top', '-l', '0', '-s', '1', '-n', '0'], stdout=subprocess.PIPE, text=True)
        threading.Thread(target=self._read_top, daemon=True).start()
        self.prev, self.prev_at = ps_snapshot(), time.monotonic()
        self.t0 = time.monotonic()

    def _read_top(self):
        first = True
        for line in self.top.stdout:
            if line.startswith('CPU usage'):
                idle = re.search(r'([\d.]+)% idle', line)
                if idle and not first:
                    self.latest_cpu = round(100 - float(idle.group(1)), 2)
                first = False

    def close(self):
        self.top.terminate()

    def sample(self, phase, builds):
        time.sleep(max(self.prev_at + self.interval - time.monotonic(), 0))
        cur, at = ps_snapshot(), time.monotonic()
        rec = {'t': round(at - self.t0, 3), 'phase': phase, 'running': sorted(builds),
               'total_pct': self.latest_cpu}
        rec.update(attribute(self.prev, cur, at - self.prev_at, self.ncpu, self.own_root, builds))
        self.prev, self.prev_at = cur, at
        total = rec['total_pct']
        rec['outside_pct'] = None if total is None else round(max(total - rec['own_pct'], 0.0), 2)
        rec['unattributed_pct'] = None if total is None else round(total - rec['own_pct'] - rec['other_pct'], 2)
        vm = sysctl('kern.memorystatus_vm_pressure_level', 'vm.swapusage', 'vm.compressor_bytes_used',
                    'vm.page_free_count')
        swap = re.search(r'used = ([\d.]+)M', vm.get('vm.swapusage', ''))
        rec.update({
            'pressure': int(vm.get('kern.memorystatus_vm_pressure_level') or 0),
            'swap_used_mb': float(swap.group(1)) if swap else None,
            'compressor_gib': round(int(vm.get('vm.compressor_bytes_used') or 0) / 2**30, 2),
            'free_mem_mb': round(int(vm.get('vm.page_free_count') or 0) * self.page / 2**20),
            'load1': round(os.getloadavg()[0], 2),
        })
        q = subprocess.run(['kr', 'queue', '--json'], capture_output=True, text=True)
        try:
            queue = json.loads(q.stdout) if q.returncode == 0 else None
        except ValueError:
            queue = None
        rec['queue_busy'] = busy_rows(queue, self.own_task)
        return rec


def write_json(path, data):
    Path(path).write_text(json.dumps(data, indent=1, ensure_ascii=False) + '\n')


def sh(cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, text=True, **kw).stdout.strip()


def killpg(pid, sig=signal.SIGTERM):
    # macOS answers EPERM, not ESRCH, for a group whose members have all exited.
    try:
        os.killpg(pid, sig)
    except (ProcessLookupError, PermissionError):
        pass


def on_signal(signum, _frame):
    raise SystemExit(128 + signum)


def install_signal_handlers():
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        # Keep nohup's ignored SIGHUP, so closing the terminal does not end an overnight run.
        if signal.getsignal(sig) is not signal.SIG_IGN:
            signal.signal(sig, on_signal)



def run(args):
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    if args.dry_run:
        args = dry_run_setup(args, out)
    elif os.environ.get('KR_STATE_DIR'):
        sys.exit('KR_STATE_DIR is set; unset it so the gate and the lock see the real kr queue')
    elif not args.conflux:
        sys.exit('--conflux is required without --dry-run')
    install_signal_handlers()

    now = dt.datetime.now()
    nb, dl = resolve_window(now, args.not_before, args.deadline)
    scratch = Path(tempfile.mkdtemp(prefix='kr69b-', dir='/tmp'))
    src, work = scratch / 'src', scratch / 'dd'
    config = {'src': str(src), 'work': str(work), 'out': str(out), 'deadline_epoch': dl.timestamp(),
              'window_seconds': args.window_minutes * 60, 'root_pid': os.getpid(), 'own_task': TASK,
              'sample_interval': args.sample_interval, 'cpu_threshold_pct': args.cpu_threshold,
              'pressure_limit': args.pressure_limit, 'dry_run': args.dry_run}
    write_json(out / 'config.json', config)
    meta = {'not_before': nb.strftime('%Y-%m-%d %H:%M'), 'deadline': dl.strftime('%Y-%m-%d %H:%M'),
            'quiet_seconds': args.quiet_seconds, 'cpu_threshold_pct': args.cpu_threshold,
            'window_minutes': args.window_minutes, 'dry_run': args.dry_run,
            'machine': sysctl('hw.model', 'hw.ncpu', 'hw.perflevel0.physicalcpu', 'hw.perflevel1.physicalcpu',
                              'hw.memsize')}
    log(f'window not-before {meta["not_before"]} deadline {meta["deadline"]}; scratch {scratch}')
    child = None
    code = EXIT_FAILED
    result = {'result': 'failed', 'reason': 'interrupted'}
    try:
        while time.time() < nb.timestamp():
            time.sleep(min(60, nb.timestamp() - time.time()))
        sh(['git', '-C', args.conflux, 'fetch', '--quiet', 'origin', 'main'])
        sh(['git', '-C', args.conflux, 'worktree', 'add', '--detach', '--quiet', str(src), 'origin/main'])
        meta['conflux_commit'] = sh(['git', '-C', str(src), 'rev-parse', 'HEAD'])
        sh(['xcodegen', 'generate', '--spec', 'ConfluxMobile.yml'], cwd=src)
        write_json(out / 'run.json', meta)

        gate = wait_quiet(out, dl, args)
        meta['gate'] = gate
        write_json(out / 'run.json', meta)
        if not gate['passed']:
            result = {'result': 'not-quiet', 'reason': gate['reason']}
            code = EXIT_NOT_QUIET
            return code
        log('quiet; taking the build lock')
        child = subprocess.Popen(['kr', 'lock', 'build', '--task', TASK, '--', sys.executable, __file__,
                                  'measure', str(out)], start_new_session=True)
        code = child.wait()
        child = None
        result = json.loads((out / 'result.json').read_text()) if (out / 'result.json').exists() else \
            {'result': 'failed', 'reason': f'measure exited {code} without result.json'}
        return code
    except subprocess.CalledProcessError as e:
        result = {'result': 'failed', 'reason': f'{e.cmd[:3]} exited {e.returncode}: {e.stderr.strip()[-300:]}'}
        return EXIT_FAILED
    finally:
        if child and child.poll() is None:
            killpg(child.pid)
            try:
                child.wait(timeout=60)
            except subprocess.TimeoutExpired:
                killpg(child.pid, signal.SIGKILL)
        cleanup(args.conflux, scratch)
        if args.dry_run:
            shutil.rmtree(Path(args.conflux).parent, ignore_errors=True)
        write_json(out / 'result.json', result)
        log(f"RESULT {result['result']}: {result.get('reason', '')}")
        analyze = subprocess.run([sys.executable, str(HERE / 'analyze-conflux-build.py'), str(out)])
        if analyze.returncode:
            log('analyzer failed; raw evidence is still in the output directory')


def cleanup(conflux, scratch):
    src = scratch / 'src'
    if conflux and src.exists():
        subprocess.run(['git', '-C', conflux, 'worktree', 'remove', '--force', str(src)], capture_output=True)
    if conflux:
        subprocess.run(['git', '-C', conflux, 'worktree', 'prune'], capture_output=True)
    shutil.rmtree(scratch, ignore_errors=True)
    log(f'removed {scratch}')


def wait_quiet(out, deadline, args):
    sampler = Sampler(os.getpid(), own_task=None, interval=args.sample_interval)
    gate = QuietGate(args.quiet_seconds, args.cpu_threshold, args.pressure_limit)
    started = time.time()
    try:
        with open(out / 'gate.ndjson', 'w') as f:
            while True:
                s = sampler.sample('gate', {})
                passed = gate.feed(s)
                f.write(json.dumps(s) + '\n')
                f.flush()
                if passed:
                    return {'passed': True, 'waited_seconds': round(time.time() - started)}
                if time.time() >= deadline.timestamp():
                    last = s['gate']['reasons']
                    return {'passed': False, 'waited_seconds': round(time.time() - started),
                            'reason': f'not quiet for {args.quiet_seconds}s before the deadline; '
                                      f'last sample: {", ".join(last) or "quiet"}'}
    finally:
        sampler.close()


def log(msg):
    print(f'[{dt.datetime.now():%H:%M:%S}] {msg}', flush=True)



class Measure:
    def __init__(self, cfg):
        self.cfg = cfg
        self.src, self.work, self.out = Path(cfg['src']), Path(cfg['work']), Path(cfg['out'])
        self.t0 = time.monotonic()
        self.window_end = self.t0 + cfg['window_seconds']
        self.lock = threading.Lock()
        self.running = {}
        self.phase_name = 'setup'
        self.stop = None
        self.done = False
        self.phases = []
        self.pressure_max = 0

    def now(self):
        return round(time.monotonic() - self.t0, 3)

    def sample_loop(self):
        sampler = Sampler(self.cfg['root_pid'], self.cfg['own_task'], self.cfg['sample_interval'])
        try:
            with open(self.out / 'samples.ndjson', 'w') as f:
                while not self.done:
                    with self.lock:
                        builds = {label: p.pid for label, p in self.running.items()}
                    s = sampler.sample(self.phase_name, builds)
                    s['t'] = self.now()
                    f.write(json.dumps(s) + '\n')
                    f.flush()
                    self.pressure_max = max(self.pressure_max, s['pressure'])
                    if s['pressure'] >= CRITICAL and self.stop != 'critical':
                        self.stop = 'critical'
                        self.kill_builds()
                    elif s['pressure'] >= self.cfg['pressure_limit'] and self.stop is None:
                        self.stop = 'warn'
                    if shutil.disk_usage(self.work).free / 2**30 < MIN_FREE_DISK_GIB and self.stop is None:
                        self.stop = 'disk'
        finally:
            sampler.close()

    def kill_builds(self):
        with self.lock:
            procs = list(self.running.values())
        for p in procs:
            killpg(p.pid)

    def build(self, label):
        dd = self.work / label
        logs = self.out / 'logs'
        logs.mkdir(exist_ok=True)
        cmd = ['xcodebuild', 'build', '-project', 'ConfluxMobile.xcodeproj', '-scheme', 'ConfluxMobile',
               '-destination', 'generic/platform=iOS Simulator', '-derivedDataPath', str(dd)]
        start = self.now()
        with open(logs / f'{label}.log', 'w') as log_file:
            p = subprocess.Popen(cmd, cwd=self.src, stdout=log_file, stderr=subprocess.STDOUT,
                                 start_new_session=True)
            with self.lock:
                self.running[label] = p
            timer = threading.Timer(max(self.window_end - time.monotonic(), 1), self.window_expired, [p])
            timer.daemon = True
            timer.start()
            try:
                # wait4 returns the rusage of the build and every descendant it reaped, which includes
                # SWBBuildService and the compilers it spawns (they are children of xcodebuild).
                _, status, ru = os.wait4(p.pid, 0)
            finally:
                timer.cancel()
            p.returncode = os.waitstatus_to_exitcode(status)
            with self.lock:
                del self.running[label]
        seconds = round(self.now() - start, 2)
        tail = (logs / f'{label}.log').read_text(errors='replace')[-4000:]
        cpu = ru.ru_utime + ru.ru_stime
        return {'label': label, 'start': start, 'end': self.now(), 'seconds': seconds,
                'exit': p.returncode, 'ok': p.returncode == 0 and '** BUILD SUCCEEDED **' in tail,
                'user_s': round(ru.ru_utime, 2), 'sys_s': round(ru.ru_stime, 2), 'cpu_s': round(cpu, 2),
                'effective_cores': round(cpu / seconds, 2) if seconds else None,
                'max_rss_mb': round(ru.ru_maxrss / 2**20)}

    def window_expired(self, p):
        self.stop = self.stop or 'window'
        killpg(p.pid)

    def phase(self, name, mode):
        labels = [f'{name}-a', f'{name}-b']
        self.phase_name = name
        start = self.now()
        if mode == 'serial':
            builds = []
            for label in labels:
                if self.stop:
                    break
                builds.append(self.build(label))
        else:
            builds = [None, None]

            def go(i):
                builds[i] = self.build(labels[i])
            threads = [threading.Thread(target=go, args=(i,)) for i in range(2)]
            for t in threads:
                t.start()
            for t in threads:
                t.join()
        self.phase_name = 'between'
        for label in labels:
            shutil.rmtree(self.work / label, ignore_errors=True)
        rec = {'phase': name, 'mode': mode, 'capacity': 1 if mode == 'serial' else 2, 'start': start,
               'end': self.now(), 'total_seconds': round(self.now() - start, 2), 'builds': builds,
               'stop': self.stop}
        self.phases.append(rec)
        self.save()
        log(f"phase {name} ({mode}) {rec['total_seconds']}s stop={self.stop}")
        return rec

    def save(self, **extra):
        write_json(self.out / 'builds.json', {'phases': self.phases, 'pressure_max': self.pressure_max,
                                              'stop': self.stop, **extra})

    def main(self):
        self.work.mkdir(parents=True, exist_ok=True)
        sampler = threading.Thread(target=self.sample_loop, daemon=True)
        sampler.start()
        try:
            (self.out / 'xcode.txt').write_text(sh(['xcodebuild', '-version']) + '\n')
            # The first resolve fills the user-level SwiftPM caches; keep it out of the timed phases.
            self.phase_name = 'warm-resolve'
            warm = subprocess.run(['xcodebuild', '-resolvePackageDependencies', '-project',
                                   'ConfluxMobile.xcodeproj', '-scheme', 'ConfluxMobile', '-derivedDataPath',
                                   str(self.work / 'warm')], cwd=self.src, capture_output=True, text=True)
            shutil.rmtree(self.work / 'warm', ignore_errors=True)
            if warm.returncode:
                return {'result': 'failed', 'reason': f'resolvePackageDependencies exited {warm.returncode}'}
            last = None
            for name, mode in PHASES:
                estimate = last['total_seconds'] * 1.25 + 30 if last else 0
                if self.stop or time.monotonic() + estimate > self.window_end:
                    break
                last = self.phase(name, mode)
            failed = [b['label'] for p in self.phases for b in p['builds'] if b and not b['ok']]
            done = [p['phase'] for p in self.phases]
            if self.stop == 'critical' or failed or not done:
                return {'result': 'failed', 'reason': f'stop={self.stop} failed={failed} phases={done}'}
            return {'result': 'measured', 'reason': f'phases {",".join(done)} stop={self.stop}'}
        finally:
            self.kill_builds()
            self.done = True
            sampler.join(timeout=10)
            self.save(window_seconds=self.now())
            shutil.rmtree(self.work, ignore_errors=True)


def measure(args):
    out = Path(args.out)
    cfg = json.loads((out / 'config.json').read_text())
    install_signal_handlers()
    if time.time() >= cfg['deadline_epoch']:
        result = {'result': 'not-quiet', 'reason': 'got the build lock after the deadline'}
        write_json(out / 'result.json', result)
        return EXIT_NOT_QUIET
    result = {'result': 'failed', 'reason': 'interrupted'}
    try:
        result = Measure(cfg).main()
    finally:
        write_json(out / 'result.json', result)
    return EXIT_MEASURED if result['result'] == 'measured' else EXIT_FAILED



def dry_run_setup(args, out):
    """Fake kr/xcodebuild/xcodegen on PATH and a throwaway git repo as Conflux; seconds, not an hour."""
    root = Path(tempfile.mkdtemp(prefix='kr69b-dry-', dir='/tmp'))
    remote, clone = root / 'remote.git', root / 'conflux'
    sh(['git', 'init', '--quiet', '--bare', '--initial-branch=main', str(remote)])
    sh(['git', 'clone', '--quiet', str(remote), str(clone)])
    (clone / 'ConfluxMobile.yml').write_text('name: ConfluxMobile\n')
    sh(['git', '-C', str(clone), 'add', '.'])
    sh(['git', '-C', str(clone), '-c', 'user.name=dry', '-c', 'user.email=dry@example.invalid', 'commit',
        '--quiet', '-m', 'dry'])
    sh(['git', '-C', str(clone), 'push', '--quiet', 'origin', 'HEAD:main'])
    os.environ['PATH'] = f"{HERE / 'fakes' / 'conflux-calibration'}{os.pathsep}{os.environ['PATH']}"
    os.environ['KR69B_FAKE_STATE'] = str(root / 'fake-state')
    os.environ.setdefault('KR69B_FAKE_BUSY_POLLS', '3')
    now = dt.datetime.now()
    args.conflux = str(clone)
    args.not_before = now.time()
    args.deadline = (now + dt.timedelta(seconds=args.dry_deadline_seconds)).time()
    args.quiet_seconds = 2
    args.sample_interval = 0.25
    args.window_minutes = 1
    # The dry run samples this machine's real CPU and memory pressure; the unit tests cover both rules.
    args.cpu_threshold = 100.0
    args.pressure_limit = CRITICAL
    log(f'dry run: fakes on PATH, Conflux stand-in {clone}')
    return args


def hhmm(text):
    return dt.datetime.strptime(text, '%H:%M').time()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    r = sub.add_parser('run')
    r.add_argument('--conflux', help='Conflux checkout whose origin/main is measured')
    r.add_argument('--out', required=True, help='evidence directory')
    r.add_argument('--not-before', type=hhmm, default=hhmm('00:00'), help='local HH:MM')
    r.add_argument('--deadline', type=hhmm, default=hhmm('23:59'), help='local HH:MM; give up if not quiet by then')
    r.add_argument('--quiet-seconds', type=float, default=120)
    r.add_argument('--cpu-threshold', type=float, default=15.0, help='percent of total machine CPU')
    r.add_argument('--window-minutes', type=float, default=60)
    r.add_argument('--sample-interval', type=float, default=1.0)
    r.add_argument('--pressure-limit', type=int, default=WARN, help=argparse.SUPPRESS)
    r.add_argument('--dry-run', action='store_true')
    r.add_argument('--dry-deadline-seconds', type=float, default=60, help=argparse.SUPPRESS)
    m = sub.add_parser('measure')
    m.add_argument('out')
    args = ap.parse_args()
    return run(args) if args.cmd == 'run' else measure(args)


if __name__ == '__main__':
    sys.exit(main())
