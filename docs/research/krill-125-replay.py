#!/usr/bin/env python3
"""Offline replay of the Krill #125 run (kr-cli #25, ADR 0010 layer 3).

Reads the saved events and run directory of run 01a13a1f-… (tests/fixtures/deliver/
krill-125, sanitized) or, if given, the full local run directory with the rerun logs. Prints Markdown:

1. lanes: which lane queued for `sim` and never got a slot (the lost-verdict cause);
2. verdicts: what the fixed `kr run` reports for each item from the saved results, and
   what re-judging today says, naming each result bundle that is missing;
3. reruns: the `-only-testing` filter old and new kr build for each failed test, with
   the framework read from the Krill sources.

Usage: python3 -I krill-125-replay.py <repo> [<run dir>] [<Krill checkout>]
"""

import json
import re
import sys
from pathlib import Path

RUN = "01a13a1f-8950-75e5-8020-4988e20bfa16"


def events(repo: Path):
    path = repo / "tests/fixtures/deliver/krill-125/state/events/2026-10-14.ndjson"
    out = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    return [e for e in out if e["run_id"] == RUN]


def lanes(evs):
    print("## 1 通道：谁排了 sim 队却没拿到槽位\n")
    queued, got = {}, {}
    for e in evs:
        f = e["fields"]
        if f.get("resource") != "sim":
            continue
        if e["type"] == "queue_start":
            queued[f["task"]] = queued.get(f["task"], 0) + 1
        if e["type"] == "slot_acquired":
            got[f["task"]] = got.get(f["task"], 0) + 1
    print("| 通道 | 排队次数 | 拿到槽位次数 |\n|---|---|---|")
    for task, n in queued.items():
        print(f"| {task} | {n} | {got.get(task, 0)} |")
    print()


def bundles(run_dir: Path, item: str):
    """Result bundles the item's logs say xcodebuild wrote, first run first."""
    found = []
    for log in sorted((run_dir / item).glob("*.log")) if run_dir else []:
        text = log.read_text(errors="replace")
        found += re.findall(r"Writing result bundle at path:\s*\n\s*(\S+\.xcresult)", text)
    return found


def verdicts(evs, run_dir):
    print("## 2 每项结论\n")
    print("修复前：kr 打印 `error: No such file or directory (os error 2)` 后退出，0 项结论。\n")
    print("修复后：每项照常给结论（来自运行时保存的结果）；今天重判时结果包已随工作区删除，")
    print("该项判失败并写明缺的文件。\n")
    print("| 项 | 设备 | 运行时结论（测试数 过/败/偶发/跳） | 今天重判 |\n|---|---|---|---|")
    passed = total = 0
    for e in evs:
        if e["type"] != "item_end":
            continue
        f = e["fields"]
        t = f.get("tests") or {}
        counts = "/".join(str(t.get(k, 0)) for k in ("passed", "failed", "flaky", "skipped"))
        verdict = "PASS" if f["passed"] else "FAIL"
        total += 1
        passed += f["passed"]
        found = bundles(run_dir, f["item"])
        if f["kind"] != "scenario" and not f.get("tests"):
            rejudge = "命令套件，没有结果包"
        elif found:
            first = next((b for b in found if b.endswith("/run.xcresult")), found[0])
            exists = Path(first).exists()
            rejudge = "可读" if exists else f"FAIL：no test results: `{redact(first)}` is missing or unreadable"
        else:
            rejudge = "（没有本机运行目录，未重判）"
        print(f"| {f['item']} | {f.get('device') or '本机'} | {verdict}（{counts}） | {rejudge} |")
    print(f"\n修复后的总结行：`run: {passed}/{total} PASS`\n")


def redact(path: str) -> str:
    return re.sub(r"^/Users/[^/]+/", "~/", path)


def framework(krill: Path, test: str):
    name = test.rsplit("/", 1)[-1].removesuffix("()")
    if not krill:
        return "?"
    for src in krill.rglob("*.swift"):
        text = src.read_text(errors="replace")
        if re.search(rf"@Test(\([^)]*\))?\s+func\s+{re.escape(name)}\s*\(", text):
            return "Swift Testing"
        if re.search(rf"func\s+{re.escape(name)}\s*\(", text):
            return "XCTest"
    return "?"


def reruns(evs, run_dir, krill):
    print("## 3 失败重跑的过滤\n")
    print("| 项 | 测试 | 框架 | 修复前过滤 | 修复后过滤 | 修复前这次重跑 |\n|---|---|---|---|---|---|")
    seen, round_dir = {}, {}
    for e in evs:
        f = e["fields"]
        if e["type"] != "rerun":
            continue
        item = f["item"]
        if f.get("whole_item"):
            # The whole item runs again in `<item>-again`, its reruns numbered from 1.
            round_dir[item], seen[item] = f"{item}-again", 0
            continue
        test = f["test"]
        n = seen[item] = seen.get(item, 0) + 1
        where = round_dir.get(item, item)
        fw = framework(krill, test)
        method = test.rsplit("/", 1)[-1]
        old = test.removesuffix("()") if method.startswith("test") else test
        new = test if fw == "Swift Testing" else test.removesuffix("()")
        took = ""
        log = run_dir / where / f"rerun-{n}.log" if run_dir else None
        if log and log.exists():
            m = re.search(r"([\d.]+) elapsed -- Testing started completed", log.read_text())
            took = f"{float(m.group(1)):.0f} s" if m else ""
        ran = "通过" if f["passed"] else "仍失败"
        mark = "（过滤不同）" if old != new else ""
        print(f"| {where} | `{method}` | {fw} | `{old.rsplit('/', 1)[-1]}` | `{new.rsplit('/', 1)[-1]}`{mark} | {ran} {took} |")
    print()


def main():
    repo = Path(sys.argv[1])
    # Default: the saved copy in the repo (first-run logs only, no rerun logs).
    run_dir = (
        Path(sys.argv[2]).expanduser()
        if len(sys.argv) > 2
        else repo / "tests/fixtures/deliver/krill-125/state/runs" / RUN
    )
    krill = Path(sys.argv[3]).expanduser() if len(sys.argv) > 3 else None
    evs = events(repo)
    lanes(evs)
    verdicts(evs, run_dir)
    reruns(evs, run_dir, krill)


main()
