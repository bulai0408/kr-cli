#!/usr/bin/env python3
"""Run with python3 -m unittest discover -s scripts -p test_calibrate_conflux_build.py -v."""

import datetime as dt
import gzip
import importlib.util
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, HERE / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


cal = load('calibrate', 'calibrate-conflux-build.py')
ana = load('analyze', 'analyze-conflux-build.py')


def queue(**resources):
    return {'resources': [{'resource': r, 'holders': [], 'waiters': [], 'load_waiters': [], **resources.get(r, {})}
                          for r in ('build', 'sim', 'light')]}


class WindowTests(unittest.TestCase):
    def window(self, now):
        nb, dl = cal.resolve_window(dt.datetime.fromisoformat(now), dt.time(3, 30), dt.time(7, 30))
        return nb.isoformat(' ', 'minutes'), dl.isoformat(' ', 'minutes')

    def test_evening_launch_waits_for_tomorrow_morning(self):
        self.assertEqual(self.window('2026-11-14 23:00'), ('2026-11-15 03:30', '2026-11-15 07:30'))

    def test_launch_inside_the_window_starts_now(self):
        self.assertEqual(self.window('2026-11-15 03:45'), ('2026-11-15 03:30', '2026-11-15 07:30'))

    def test_launch_after_the_deadline_rolls_to_the_next_day(self):
        self.assertEqual(self.window('2026-11-15 08:00'), ('2026-11-16 03:30', '2026-11-16 07:30'))


class QueueTests(unittest.TestCase):
    def test_other_tasks_on_any_resource_are_busy(self):
        q = queue(sim={'holders': [{'task': 'ui'}]}, light={'load_waiters': [{'task': 'lint'}]},
                  build={'holders': [{'task': cal.TASK}], 'waiters': [{'task': 'gate'}]})
        self.assertEqual(sorted(cal.busy_rows(q, cal.TASK)),
                         ['build:waiters:gate', 'light:load_waiters:lint', 'sim:holders:ui'])

    def test_empty_queue_is_quiet_and_missing_queue_is_busy(self):
        self.assertEqual(cal.busy_rows(queue(), None), [])
        self.assertEqual(cal.busy_rows(None, None), ['kr-queue:unavailable'])


class GateTests(unittest.TestCase):
    def sample(self, t, outside=2.0, busy=(), pressure=1):
        return {'t': float(t), 'outside_pct': outside, 'queue_busy': list(busy), 'pressure': pressure}

    def first_pass(self, samples, quiet_seconds=120, threshold=15):
        gate = cal.QuietGate(quiet_seconds, threshold)
        return next((s['t'] for s in samples if gate.feed(s)), None)

    def test_passes_after_120_consecutive_quiet_seconds(self):
        self.assertEqual(self.first_pass([self.sample(t) for t in range(200)]), 120)

    def test_each_kind_of_noise_restarts_the_count(self):
        noise = {50: {'outside': 15.0}, 80: {'busy': ['build:waiters:x']}, 110: {'pressure': 2},
                 140: {'outside': None}}
        samples = [self.sample(t, **noise.get(t, {})) for t in range(400)]
        self.assertEqual(self.first_pass(samples), 141 + 120)

    def test_never_quiet_never_passes(self):
        self.assertIsNone(self.first_pass([self.sample(t, outside=40) for t in range(400)]))

    def test_records_reasons(self):
        s = self.sample(0, outside=30, busy=['sim:holders:ui'])
        cal.QuietGate(120, 15).feed(s)
        self.assertEqual(s['gate'], {'quiet': False, 'reasons': ['kr:sim:holders:ui', 'cpu:30.0%']})


class AttributionTests(unittest.TestCase):
    def test_groups_cpu_by_process_tree(self):
        # 1 driver -> 2 xcodebuild -> 3 build service -> 4 compiler (new); 9 is someone else's app.
        prev = {1: (0, 1.0, 'python3'), 2: (1, 0.0, 'xcodebuild'), 3: (2, 10.0, 'SWBBuildService'),
                9: (0, 100.0, '/Applications/Other.app/Contents/MacOS/Other')}
        cur = {1: (0, 1.1, 'python3'), 2: (1, 0.1, 'xcodebuild'), 3: (2, 13.0, 'SWBBuildService'),
               4: (3, 2.0, 'swift-frontend'), 9: (0, 100.8, '/Applications/Other.app/Contents/MacOS/Other')}
        out = cal.attribute(prev, cur, seconds=1.0, ncpu=8, own_root=1, builds={'A1-a': 2})
        self.assertAlmostEqual(out['builds_pct']['A1-a'], (0.1 + 3.0 + 2.0) / 8 * 100, places=1)
        self.assertAlmostEqual(out['own_pct'], (0.1 + 0.1 + 3.0 + 2.0) / 8 * 100, places=1)
        self.assertAlmostEqual(out['other_pct'], 0.8 / 8 * 100, places=1)
        self.assertEqual(out['top_other'][0][0], 'Other')

    def test_reused_pid_counts_from_zero(self):
        out = cal.attribute({5: (0, 50.0, 'old')}, {5: (0, 0.4, 'new')}, 1.0, 1, own_root=1, builds={})
        self.assertAlmostEqual(out['other_pct'], 40.0)

    def test_parses_ps_cputime(self):
        self.assertEqual(cal.parse_cputime('0:01.50'), 1.5)
        self.assertEqual(cal.parse_cputime('187:06.43'), 187 * 60 + 6.43)
        self.assertEqual(cal.parse_cputime('1-02:00:00.00'), 86400 + 7200)


def phase_samples(phase, start, n, other, builds):
    return [{'t': start + i, 'phase': phase, 'total_pct': 90.0, 'own_pct': 80.0, 'other_pct': o,
             'unattributed_pct': 0.0, 'builds_pct': {b: 40.0 for b in builds}, 'top_other': [['Mail', o]],
             'pressure': 1, 'swap_used_mb': 10.0, 'free_mem_mb': 500, 'compressor_gib': 1.0, 'queue_busy': []}
            for i, o in enumerate(other)]


def build(label, seconds, cpu):
    return {'label': label, 'seconds': seconds, 'cpu_s': cpu, 'user_s': cpu, 'sys_s': 0.0,
            'effective_cores': round(cpu / seconds, 2), 'max_rss_mb': 1, 'ok': True}


class AnalyzerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.out = Path(self.temp.name)

    def write_run(self, b1_other):
        (self.out / 'config.json').write_text(json.dumps({'cpu_threshold_pct': 15}))
        (self.out / 'result.json').write_text(json.dumps({'result': 'measured'}))
        (self.out / 'run.json').write_text(json.dumps({'machine': {'hw.ncpu': '8'},
                                                       'note': f'{Path.home()}/Workspace/Conflux'}))
        phases = [
            {'phase': 'A1', 'mode': 'serial', 'capacity': 1, 'total_seconds': 20,
             'builds': [build('A1-a', 10, 60), build('A1-b', 10, 60)]},
            {'phase': 'B1', 'mode': 'parallel', 'capacity': 2, 'total_seconds': 16,
             'builds': [build('B1-a', 16, 64), build('B1-b', 16, 64)]},
        ]
        (self.out / 'builds.json').write_text(json.dumps({'phases': phases, 'stop': None}))
        samples = phase_samples('A1', 0, 20, [16.0] * 2 + [3.0] * 18, ['A1-a']) + \
            phase_samples('B1', 20, 10, b1_other, ['B1-a', 'B1-b'])
        (self.out / 'samples.ndjson').write_text(''.join(json.dumps(s) + '\n' for s in samples))

    def test_flags_phase_with_outside_cpu_in_more_than_ten_percent_of_samples(self):
        self.write_run([20.0] * 2 + [3.0] * 8)
        s = ana.summarize(self.out)
        a1, b1 = s['phases']
        self.assertFalse(a1['contaminated'])
        self.assertTrue(b1['contaminated'])
        self.assertEqual(s['comparison']['clean_pairs'], 0)
        self.assertEqual([a1['phase_effective_cores'], b1['phase_effective_cores']], [6.0, 8.0])
        self.assertEqual(s['comparison']['pairs'][0]['per_build_slowdown'], 1.6)
        self.assertEqual(b1['other_cpu_pct_mean'], 6.4)

    def test_clean_pair_is_counted(self):
        self.write_run([3.0] * 10)
        s = ana.summarize(self.out)
        self.assertEqual(s['comparison']['total_ratio_clean'], [0.8])

    def test_public_export_redacts_home_and_refuses_foreign_paths(self):
        self.write_run([3.0] * 10)
        (self.out / 'logs').mkdir()
        (self.out / 'logs' / 'A1-a.log').write_text(f'DEVELOPMENT_TEAM = Z9Y8X7W6V5\n{Path.home()}/x\n')
        (self.out / 'summary.json').write_text('{}')
        self.assertEqual(ana.export_public(self.out), [])
        public = '\n'.join(p.read_text() for p in (self.out / 'public').glob('*') if p.suffix != '.gz')
        with gzip.open(self.out / 'public' / 'samples.ndjson.gz', 'rt') as f:
            public += f.read()
        self.assertNotIn(str(Path.home()), public)
        self.assertNotIn('Z9Y8X7W6V5', public)
        self.assertIn('~/Workspace/Conflux', public)
        (self.out / 'logs' / 'A1-a.log').write_text('/Users/somebody-else/project\n')
        self.assertTrue(ana.export_public(self.out))


class DryRunTests(unittest.TestCase):
    """The whole flow with fake kr, xcodebuild and xcodegen on PATH."""

    def run_dry(self, *extra, **env):
        out = Path(tempfile.mkdtemp(prefix='kr69b-test-'))
        self.addCleanup(subprocess.run, ['rm', '-rf', str(out)])
        p = subprocess.run([sys.executable, str(HERE / 'calibrate-conflux-build.py'), 'run', '--dry-run',
                            '--out', str(out), *extra], capture_output=True, text=True, timeout=120,
                           env={**os.environ, **env})
        scratch = re.search(r'scratch (\S+)', p.stdout).group(1)
        self.assertFalse(Path(scratch).exists(), 'scratch worktree and DerivedData removed')
        stand_in = re.search(r'Conflux stand-in (\S+)', p.stdout).group(1)
        self.assertFalse(Path(stand_in).exists(), 'dry-run stand-in repo removed')
        return p, out, json.loads((out / 'result.json').read_text())

    def test_quiet_machine_measures_four_phases(self):
        p, out, result = self.run_dry()
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertEqual(result['result'], 'measured')
        summary = json.loads((out / 'summary.json').read_text())
        self.assertEqual([r['phase'] for r in summary['phases']], ['A1', 'B1', 'A2', 'B2'])
        for row in summary['phases']:
            for b in row['builds']:
                self.assertGreater(b['cpu_s'], 0.3, 'rusage includes the grandchild that burned CPU')
        gate = [json.loads(line) for line in (out / 'gate.ndjson').read_text().splitlines()]
        self.assertIn('kr:build:waiters:someone-else', gate[0]['gate']['reasons'])
        self.assertTrue((out / 'public' / 'summary.json').exists())

    def test_busy_queue_until_deadline_exits_not_quiet_without_measuring(self):
        p, out, result = self.run_dry('--dry-deadline-seconds', '3', KR69B_FAKE_BUSY_POLLS='100000')
        self.assertEqual(p.returncode, 3, p.stdout + p.stderr)
        self.assertEqual(result['result'], 'not-quiet')
        self.assertIn('RESULT not-quiet', p.stdout)
        self.assertFalse((out / 'logs').exists())
        self.assertFalse((out / 'samples.ndjson').exists())

    def test_sigterm_during_a_build_still_cleans_up(self):
        out = Path(tempfile.mkdtemp(prefix='kr69b-test-'))
        self.addCleanup(subprocess.run, ['rm', '-rf', str(out)])
        p = subprocess.Popen([sys.executable, str(HERE / 'calibrate-conflux-build.py'), 'run', '--dry-run',
                              '--out', str(out)], stdout=subprocess.PIPE, text=True,
                             env={**os.environ, 'KR69B_FAKE_BUILD_SECONDS': '60'})
        lines = []
        for line in p.stdout:
            lines.append(line)
            if 'taking the build lock' in line:
                break
        deadline = time.time() + 30
        while not (out / 'logs').exists() and time.time() < deadline:
            time.sleep(0.2)
        p.terminate()
        lines += p.stdout.readlines()
        p.stdout.close()
        self.assertEqual(p.wait(timeout=30), 128 + signal.SIGTERM)
        text = ''.join(lines)
        self.assertFalse(Path(re.search(r'scratch (\S+)', text).group(1)).exists())
        self.assertEqual(json.loads((out / 'result.json').read_text())['result'], 'failed')
        burning = subprocess.run(['pgrep', '-f', 'end = time.time'], capture_output=True, text=True).stdout
        self.assertEqual(burning, '', 'fake build processes were killed')

    def test_failed_build_exits_1_and_still_cleans_up(self):
        p, _, result = self.run_dry(KR69B_FAKE_BUILD_FAIL='1')
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertEqual(result['result'], 'failed')


if __name__ == '__main__':
    unittest.main()
