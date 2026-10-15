#!/usr/bin/env python3
"""Replay frozen Krill merge parents read-only; all merge writes stay in a temp dir."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def run(argv, cwd=None, env=None):
    return subprocess.run(argv, cwd=cwd, env=env, capture_output=True, text=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--kr", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    fixtures = Path(__file__).resolve().parents[2] / "tests/fixtures/merge"
    sample = json.loads((fixtures / "history-sample.json").read_text())
    rows = []
    for case in sample:
        with tempfile.TemporaryDirectory(prefix="kr-merge-eval-") as directory:
            work = Path(directory)
            for name, commit in zip(["base", "ours", "theirs"], [case["base"], *case["parents"]]):
                result = run(["git", "-C", args.repo, "show", f"{commit}:Krill/Localizable.xcstrings"])
                if result.returncode:
                    raise RuntimeError(f"missing sample commit {commit}")
                (work / f"{name}.xcstrings").write_text(result.stdout)
            before = run(["git", "merge-file", "-p", "ours.xcstrings", "base.xcstrings", "theirs.xcstrings"], cwd=work)
            env = dict(os.environ, HOME=str(work), KR_STATE_DIR=str(work / "state"))
            env.pop("KR_REPLAY", None)
            env.pop("KR_RECORD", None)
            after = run([str(Path(args.kr).resolve()), "merge", "xcstrings", "base.xcstrings", "ours.xcstrings", "theirs.xcstrings", "--json"], cwd=work, env=env)
            data = json.loads(after.stdout)
            rows.append({"merge": case["merge"], "text_conflict": before.returncode != 0,
                         "driver_exit": after.returncode, "validated": data.get("validated", False),
                         "same_key_conflicts": case["same_key_conflicts"]})
    result = {"schema_version": 1, "layer": 3, "sample_count": len(rows),
              "before_manual": sum(r["text_conflict"] for r in rows),
              "after_manual": sum(r["driver_exit"] != 0 for r in rows),
              "validated_catalogs": sum(r["validated"] for r in rows),
              "calibration": "deterministic historical replay; no runtime model or prospective error estimate",
              "cases": rows}
    Path(args.output).write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "cases"}))


if __name__ == "__main__":
    main()
