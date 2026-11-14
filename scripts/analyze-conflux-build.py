#!/usr/bin/env python3
"""#69b: summarize calibrate-conflux-build.py output.

  analyze-conflux-build.py OUT

Writes OUT/summary.json and OUT/public/, a redacted copy safe to commit (home path, user name,
host name and team IDs replaced). Exits 2 if anything private survives redaction.
"""
import getpass
import gzip
import json
import os
import re
import socket
import statistics
import sys
from pathlib import Path

CONTAMINATED_SHARE = 0.10
PUBLIC_FILES = ['run.json', 'result.json', 'builds.json', 'xcode.txt', 'summary.json']
LOG_TAIL_LINES = 40


def load_ndjson(path):
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def mean(values):
    values = [v for v in values if v is not None]
    return round(statistics.mean(values), 2) if values else None


def describe_phase(p, samples, threshold, ncpu):
    w = [s for s in samples if s['phase'] == p['phase'] and s.get('total_pct') is not None]
    other = [s['other_pct'] for s in w]
    over = sum(o > threshold for o in other)
    builds = [b for b in p['builds'] if b]
    cpu = sum(b['cpu_s'] for b in builds)
    ps_cpu = {}
    for label in (b['label'] for b in builds):
        ps_cpu[label] = 0.0
        for prev, s in zip(w, w[1:]):
            ps_cpu[label] += s['builds_pct'].get(label, 0) / 100 * (s['t'] - prev['t'])
    levels = {}
    for s in w:
        levels[str(s['pressure'])] = levels.get(str(s['pressure']), 0) + 1
    kr = sorted({row for s in w for row in s['queue_busy']})
    swaps = [s['swap_used_mb'] for s in w if s['swap_used_mb'] is not None]
    return {
        'phase': p['phase'], 'mode': p['mode'], 'capacity': p['capacity'],
        'ok': len(builds) == 2 and all(b['ok'] for b in builds),
        'total_seconds': p['total_seconds'],
        'builds': [{k: b[k] for k in ('label', 'seconds', 'cpu_s', 'user_s', 'sys_s', 'effective_cores',
                                      'max_rss_mb', 'ok')} for b in builds],
        'phase_effective_cores': round(cpu / p['total_seconds'], 2) if p['total_seconds'] else None,
        'samples': len(w),
        'total_cpu_pct_mean': mean(s['total_pct'] for s in w),
        'build_cpu_pct_mean': mean(sum(s['builds_pct'].values()) for s in w),
        'other_cpu_pct_mean': mean(other),
        'unattributed_cpu_pct_mean': mean(s['unattributed_pct'] for s in w),
        'samples_other_over_threshold': over,
        'contaminated': bool(w) and over / len(w) > CONTAMINATED_SHARE,
        'top_other': top_other(w),
        # ps sees only processes alive at a sample; short compiler jobs that start and exit
        # between two samples are missing. This ratio says how much of rusage ps captured.
        'ps_capture_ratio': {b['label']: round(ps_cpu[b['label']] * ncpu / b['cpu_s'], 2)
                             for b in builds if b['cpu_s']},
        'pressure_level_counts': levels,
        'swap_used_mb_min_max': [min(swaps), max(swaps)] if swaps else None,
        'free_mem_mb_min': min((s['free_mem_mb'] for s in w), default=None),
        'compressor_gib_max': max((s['compressor_gib'] for s in w), default=None),
        'other_kr_work': kr,
    }


def top_other(w):
    totals = {}
    for s in w:
        for name, pct in s.get('top_other', []):
            totals[name] = totals.get(name, 0) + pct
    ranked = sorted(totals.items(), key=lambda kv: -kv[1])[:5]
    return [[name, round(total / len(w), 2)] for name, total in ranked] if w else []


def compare(rows):
    pairs = []
    for a, b in zip([r for r in rows if r['capacity'] == 1], [r for r in rows if r['capacity'] == 2]):
        serial = [x['seconds'] for x in a['builds']]
        parallel = [x['seconds'] for x in b['builds']]
        pairs.append({
            'serial': a['phase'], 'parallel': b['phase'],
            'clean': a['ok'] and b['ok'] and not a['contaminated'] and not b['contaminated'],
            'total_ratio': round(b['total_seconds'] / a['total_seconds'], 3),
            'per_build_slowdown': round(statistics.mean(parallel) / statistics.mean(serial), 3),
            'effective_cores': [a['phase_effective_cores'], b['phase_effective_cores']],
        })
    clean = [p for p in pairs if p['clean']]
    return {'pairs': pairs, 'clean_pairs': len(clean),
            'total_ratio_clean': [p['total_ratio'] for p in clean],
            'per_build_slowdown_clean': [p['per_build_slowdown'] for p in clean]}


def summarize(out):
    config = json.loads((out / 'config.json').read_text())
    threshold = config['cpu_threshold_pct']
    result = json.loads((out / 'result.json').read_text())
    gate = load_ndjson(out / 'gate.ndjson')
    samples = load_ndjson(out / 'samples.ndjson')
    builds = json.loads((out / 'builds.json').read_text()) if (out / 'builds.json').exists() else {'phases': []}
    run = json.loads((out / 'run.json').read_text()) if (out / 'run.json').exists() else {}
    ncpu = int(run.get('machine', {}).get('hw.ncpu') or os.cpu_count())
    rows = [describe_phase(p, samples, threshold, ncpu) for p in builds['phases']]
    quiet = [s for s in gate if s.get('gate', {}).get('quiet')]
    return {
        'result': result, 'cpu_threshold_pct': threshold, 'contaminated_if_share_over': CONTAMINATED_SHARE,
        'gate': {'samples': len(gate), 'quiet_samples': len(quiet),
                 'outside_cpu_pct_mean_last_quiet_run': mean(s['outside_pct'] for s in quiet[-120:])},
        'stop': builds.get('stop'), 'pressure_max': builds.get('pressure_max'),
        'window_seconds': builds.get('window_seconds'),
        'phases': rows, 'comparison': compare(rows) if rows else None,
    }


def redactor():
    home = str(Path.home())
    user = getpass.getuser()
    host = socket.gethostname().split('.')[0]
    rules = [(re.compile(re.escape(home)), '~'),
             (re.compile(r'(?i)(DEVELOPMENT_TEAM|team ?identifier|team ?id)([\s=:"]+)[A-Z0-9]{10}\b'),
              r'\1\2<team>')]
    for word, token in ((user, '<user>'), (host, '<host>')):
        if len(word) >= 3:
            rules.append((re.compile(rf'(?<![A-Za-z0-9]){re.escape(word)}(?![A-Za-z0-9])'), token))

    def redact(text):
        for pattern, repl in rules:
            text = pattern.sub(repl, text)
        return text
    leaks = [re.compile(r'/Users/(?!Shared/)[^/\s"<]+')] + [p for p, _ in rules]
    return redact, leaks


def export_public(out):
    redact, leaks = redactor()
    public = out / 'public'
    public.mkdir(exist_ok=True)
    written = {}
    for name in PUBLIC_FILES:
        if (out / name).exists():
            written[name] = redact((out / name).read_text(errors='replace'))
    for name in ('gate.ndjson', 'samples.ndjson'):
        if (out / name).exists():
            written[name + '.gz'] = redact((out / name).read_text())
    for log in sorted((out / 'logs').glob('*.log')) if (out / 'logs').exists() else []:
        lines = log.read_text(errors='replace').splitlines()[-LOG_TAIL_LINES:]
        written[f'{log.stem}.tail.log'] = redact('\n'.join(lines) + '\n')
    found = sorted({m.group(0) for text in written.values() for p in leaks for m in p.finditer(text)})
    if found:
        return [f'{len(found)} private string(s) survived redaction, e.g. {found[0][:3]}...']
    for name, text in written.items():
        if name.endswith('.gz'):
            with gzip.open(public / name, 'wt') as f:
                f.write(text)
        else:
            (public / name).write_text(text)
    return []


def main():
    out = Path(sys.argv[1])
    summary = summarize(out)
    (out / 'summary.json').write_text(json.dumps(summary, indent=1, ensure_ascii=False) + '\n')
    errors = export_public(out)
    for e in errors:
        print(f'public export refused: {e}', file=sys.stderr)
    print(json.dumps({'result': summary['result'], 'comparison': summary['comparison']}, ensure_ascii=False))
    return 2 if errors else 0


if __name__ == '__main__':
    sys.exit(main())
