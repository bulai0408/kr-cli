#!/usr/bin/env python3
"""#69: real Conflux build, capacity 1 (serial) vs 2 (parallel), inside one held build slot.

Usage (from anywhere):
  kr lock build --task kr69-calibration -- python3 calibrate-conflux-build.py SRC WORK OUT [DEADLINE_MIN]

SRC is a Conflux checkout with ConfluxMobile.xcodeproj generated, WORK holds the
DerivedData folders, OUT receives samples.ndjson, builds.json and the logs.
"""
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time

SRC, WORK, OUT = (os.path.abspath(p) for p in sys.argv[1:4])
DEADLINE = time.monotonic() + 60 * float(sys.argv[4] if len(sys.argv) > 4 else 72)
EDIT_FILE = os.path.join(SRC, 'ConfluxCore/Sources/ConfluxCore/Models/Platform.swift')
WARN = 2
CRITICAL = 4
T0 = time.monotonic()

state = {'stop': None, 'pressure_max': 0, 'running': {}}
lock = threading.Lock()
os.makedirs(OUT, exist_ok=True)
os.makedirs(WORK, exist_ok=True)


def now():
    return round(time.monotonic() - T0, 3)


def sysctl(name):
    return subprocess.run(['sysctl', '-n', name], capture_output=True, text=True).stdout.strip()


def free_gib():
    st = os.statvfs(WORK)
    return st.f_bavail * st.f_frsize / 2**30


def top_reader(latest):
    proc = subprocess.Popen(['top', '-l', '0', '-s', '1', '-n', '0'], stdout=subprocess.PIPE, text=True)
    for line in proc.stdout:
        if line.startswith('CPU usage'):
            m = re.findall(r'([\d.]+)% (user|sys|idle)', line)
            latest['cpu'] = {k: float(v) for v, k in m}
        elif line.startswith('PhysMem'):
            latest['physmem'] = line.strip()
        if state['stop'] == 'done':
            proc.terminate()
            return


def sampler():
    latest = {}
    threading.Thread(target=top_reader, args=(latest,), daemon=True).start()
    with open(os.path.join(OUT, 'samples.ndjson'), 'w') as f:
        while state['stop'] != 'done':
            level = int(sysctl('kern.memorystatus_vm_pressure_level') or 0)
            queue = subprocess.run(['kr', 'queue', '--json'], capture_output=True, text=True).stdout
            try:
                queue = json.loads(queue)
            except ValueError:
                queue = None
            with lock:
                running = sorted(state['running'])
            rec = {
                't': now(), 'load': os.getloadavg(), 'pressure': level,
                'swap': sysctl('vm.swapusage'), 'compressor_bytes': int(sysctl('vm.compressor_bytes_used') or 0),
                'free_gib': round(free_gib(), 2), 'cpu': latest.get('cpu'), 'physmem': latest.get('physmem'),
                'running': running, 'queue': queue,
            }
            f.write(json.dumps(rec) + '\n')
            f.flush()
            state['pressure_max'] = max(state['pressure_max'], level)
            if level >= CRITICAL and state['stop'] is None:
                state['stop'] = 'critical'
                terminate_all()
            elif level >= WARN and state['stop'] is None:
                state['stop'] = 'warn'
            if rec['free_gib'] < 8 and state['stop'] is None:
                state['stop'] = 'disk'
            time.sleep(1)


def terminate_all():
    with lock:
        procs = list(state['running'].values())
    for p in procs:
        try:
            os.killpg(p.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass


def build(label, dd):
    log = open(os.path.join(OUT, f'{label}.log'), 'w')
    cmd = ['xcodebuild', 'build', '-project', 'ConfluxMobile.xcodeproj', '-scheme', 'ConfluxMobile',
           '-destination', 'generic/platform=iOS Simulator', '-derivedDataPath', dd]
    start = now()
    p = subprocess.Popen(cmd, cwd=SRC, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    with lock:
        state['running'][label] = p
    remaining = DEADLINE - time.monotonic()
    try:
        code = p.wait(timeout=max(remaining, 1))
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGTERM)
        code = p.wait()
        state['stop'] = state['stop'] or 'deadline'
    with lock:
        del state['running'][label]
    log.close()
    tail = open(os.path.join(OUT, f'{label}.log')).read()[-4000:]
    ok = code == 0 and '** BUILD SUCCEEDED **' in tail
    return {'label': label, 'dd': os.path.basename(dd), 'start': start, 'end': now(),
            'seconds': round(now() - start, 2), 'exit': code, 'ok': ok}


def phase(name, mode, items):
    """items: list of (label, dd). mode 'serial' (capacity 1) or 'parallel' (capacity 2)."""
    start = now()
    if mode == 'serial':
        results = []
        for label, dd in items:
            if state['stop']:
                break
            results.append(build(label, dd))
    else:
        results = [None] * len(items)

        def run(i, label, dd):
            results[i] = build(label, dd)
        threads = [threading.Thread(target=run, args=(i, *it)) for i, it in enumerate(items)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
    rec = {'phase': name, 'mode': mode, 'capacity': 1 if mode == 'serial' else 2,
           'start': start, 'end': now(), 'total_seconds': round(now() - start, 2), 'builds': results,
           'stop': state['stop']}
    log_phase(rec)
    return rec


phases = []


def log_phase(rec):
    phases.append(rec)
    with open(os.path.join(OUT, 'builds.json'), 'w') as f:
        json.dump({'phases': phases, 'pressure_max': state['pressure_max'], 'stop': state['stop']}, f, indent=1)
    print(json.dumps({k: rec[k] for k in ('phase', 'total_seconds', 'stop')}), flush=True)


def edit(n):
    with open(EDIT_FILE, 'a') as f:
        f.write(f'\npublic enum KR69Marker{n} {{}}\n')


def rm(*names):
    for n in names:
        shutil.rmtree(os.path.join(WORK, n), ignore_errors=True)


def fits(estimate):
    return not state['stop'] and time.monotonic() + estimate < DEADLINE


def main():
    threading.Thread(target=sampler, daemon=True).start()
    dd = lambda n: os.path.join(WORK, n)
    # The first resolve of a fresh DerivedData fills the user-level SwiftPM caches; keep it out of the samples.
    warm = subprocess.run(['xcodebuild', '-resolvePackageDependencies', '-project', 'ConfluxMobile.xcodeproj',
                           '-scheme', 'ConfluxMobile', '-derivedDataPath', dd('warm')], cwd=SRC,
                          capture_output=True, text=True)
    log_phase({'phase': 'warm-resolve', 'mode': 'untimed', 'capacity': 0, 'start': 0, 'end': now(),
               'total_seconds': now(), 'builds': [], 'exit': warm.returncode, 'stop': state['stop']})
    rm('warm')

    a = phase('cold-1', 'serial', [('cold-1-a', dd('c1a')), ('cold-1-b', dd('c1b'))])
    rm('c1a', 'c1b')
    cold_one = max(b['seconds'] for b in a['builds']) if a['builds'] else 900
    b = phase('cold-2', 'parallel', [('cold-2-a', dd('c2a')), ('cold-2-b', dd('c2b'))])
    pair = [dd('c2a'), dd('c2b')]
    # Second cold pair in reverse order (B then A) balances drift across the window.
    if fits(b['total_seconds'] + cold_one * 2 + 60):
        rm('c2a', 'c2b')
        phase('cold-3', 'parallel', [('cold-3-a', dd('c3a')), ('cold-3-b', dd('c3b'))])
        rm('c3a', 'c3b')
        if fits(cold_one * 2 + 60):
            phase('cold-4', 'serial', [('cold-4-a', dd('c4a')), ('cold-4-b', dd('c4b'))])
            pair = [dd('c4a'), dd('c4b')]

    marker = 0
    for i, mode in enumerate(['serial', 'parallel', 'serial', 'parallel']):
        last = [p for p in phases if p['phase'].startswith('incr')]
        estimate = (max(last[-1]['total_seconds'], 60) * 1.5) if last else 420
        if not fits(estimate) or not all(os.path.isdir(x) for x in pair):
            break
        marker += 1
        edit(marker)
        phase(f'incr-{i + 1}', mode, [(f'incr-{i + 1}-a', pair[0]), (f'incr-{i + 1}-b', pair[1])])
    rm('c2a', 'c2b', 'c3a', 'c3b', 'c4a', 'c4b')

    subprocess.run(['git', '-C', SRC, 'checkout', '--', EDIT_FILE])
    stop = state['stop']
    state['stop'] = 'done'
    time.sleep(2)
    with open(os.path.join(OUT, 'builds.json'), 'w') as f:
        json.dump({'phases': phases, 'pressure_max': state['pressure_max'], 'stop': stop,
                   'window_seconds': now()}, f, indent=1)
    failed = [b for p in phases for b in p['builds'] if b and not b['ok']]
    return 1 if failed or stop in ('critical',) else 0


if __name__ == '__main__':
    sys.exit(main())
