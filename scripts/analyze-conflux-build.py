#!/usr/bin/env python3
"""#69: summarize calibrate-conflux-build.py output. Usage: analyze-conflux-build.py OUT > summary.json"""
import json
import os
import re
import statistics
import sys

OUT = sys.argv[1]
data = json.load(open(os.path.join(OUT, 'builds.json')))
samples = [json.loads(line) for line in open(os.path.join(OUT, 'samples.ndjson'))]


def swap_mb(s):
    return float(re.search(r'used = ([\d.]+)M', s['swap']).group(1))


def other_kr_work(w):
    rows = set()
    for s in w:
        for r in (s['queue'] or {}).get('resources', []):
            rows.update(f"{r['resource']}:hold:{h['task']}" for h in r['holders'] if h['task'] != 'kr69-calibration')
            rows.update(f"{r['resource']}:wait:{x['task']}" for x in r['waiters'])
    return sorted(rows)


def describe(p):
    w = [s for s in samples if p['start'] <= s['t'] <= p['end']]
    idle = [s['cpu']['idle'] for s in w if s['cpu']]
    levels = {}
    for s in w:
        levels[s['pressure']] = levels.get(s['pressure'], 0) + 1
    return {
        'phase': p['phase'], 'kind': p['phase'].split('-')[0], 'capacity': p['capacity'],
        'total_seconds': p['total_seconds'], 'build_seconds': [b['seconds'] for b in p['builds']],
        'ok': len(p['builds']) == 2 and all(b['ok'] for b in p['builds']),
        'samples': len(w), 'pressure_level_counts': levels,
        'cpu_idle_percent_mean': round(statistics.mean(idle), 1),
        'cpu_idle_samples_below_10_percent': sum(i < 10 for i in idle),
        'load1_max': round(max(s['load'][0] for s in w), 1),
        'swap_used_mb_min_max': [min(map(swap_mb, w)), max(map(swap_mb, w))],
        'compressor_gib_max': round(max(s['compressor_bytes'] for s in w) / 2**30, 2),
        'free_disk_gib_min': min(s['free_gib'] for s in w),
        'other_kr_work': other_kr_work(w),
    }


rows = [describe(p) for p in data['phases'] if p['mode'] != 'untimed']
warn = [s for s in samples if s['pressure'] >= 2]
result = {
    'phases': rows, 'stop': data['stop'], 'window_seconds': data.get('window_seconds'), 'samples': len(samples),
    'pressure_max': max(s['pressure'] for s in samples),
    'warn_samples': {'count': len(warn), 'first_t': warn[0]['t'], 'last_t': warn[-1]['t']} if warn else None,
}
for kind in ('cold', 'incr'):
    ones = [r for r in rows if r['kind'] == kind and r['capacity'] == 1 and r['ok']]
    twos = [r for r in rows if r['kind'] == kind and r['capacity'] == 2 and r['ok']]
    pairs = list(zip(ones, twos))
    if not pairs:
        continue
    serial = [x for a, _ in pairs for x in a['build_seconds']]
    parallel = [x for _, b in pairs for x in b['build_seconds']]
    result[kind] = {
        'pairs': [[a['phase'], b['phase']] for a, b in pairs],
        'total_ratio_each_pair': [round(b['total_seconds'] / a['total_seconds'], 3) for a, b in pairs],
        'serial_build_seconds_mean': round(statistics.mean(serial), 1),
        'parallel_build_seconds_mean': round(statistics.mean(parallel), 1),
        'per_build_slowdown': round(statistics.mean(parallel) / statistics.mean(serial), 3),
    }
json.dump(result, sys.stdout, indent=1, ensure_ascii=False)
print()
