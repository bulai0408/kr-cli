"""Read a copied kr state directory; summarize the evidence, without running tests."""

import argparse
from datetime import datetime
import json
from pathlib import Path


def summarize(state):
    events = [
        json.loads(line)
        for file in sorted((state / "events").glob("*.ndjson"))
        for line in file.read_text().splitlines()
        if line
    ]
    rows = []
    for index, event in enumerate(events):
        fields = event["fields"]
        if event["type"] != "rerun" or not fields.get("whole_item"):
            continue
        if fields.get("skipped"):
            continue
        run_id, item = event["run_id"], fields["item"]
        solos = [
            e["fields"]
            for e in events[:index]
            if e["run_id"] == run_id
            and e["type"] == "rerun"
            and e["fields"].get("item") == item
            and "test" in e["fields"]
        ]
        failed_in_log = False
        normal_load_evidence = False
        for number, solo in enumerate(solos, 1):
            file = state / "runs" / run_id / item / f"rerun-{number}.log"
            parts = solo["test"].split("/")
            if len(parts) < 3 or not file.exists():
                continue
            text = file.read_text(errors="replace")
            if "Failing tests:" not in text:
                continue
            failing = text.split("Failing tests:", 1)[1].split("** TEST EXECUTE", 1)[0]
            cls, name = parts[-2:]
            # Exact identifiers in xcodebuild's explicit failure list; no keyword guess.
            if not any(
                line.strip() in (f"{cls}.{name}", f"-[{cls} {name}]")
                for line in failing.splitlines()
            ):
                continue
            failed_in_log = True
            load = solo.get("load") or {}
            normal_load_evidence |= (
                solo.get("load_state") == "normal"
                and load.get("start") is not None
                and load.get("end") is not None
            )
        end = next(
            (
                e
                for e in events[index + 1 :]
                if e["run_id"] == run_id
                and e["type"] == "item_end"
                and e["fields"].get("item") == item
            ),
            None,
        )
        seconds = (
            (datetime.fromisoformat(end["ts"]) - datetime.fromisoformat(event["ts"])).total_seconds()
            if end
            else None
        )
        rows.append(
            {
                "run_id": run_id,
                "item": item,
                "repeated_failure_in_log": failed_in_log,
                "skip_with_normal_load_evidence": normal_load_evidence,
                "rerun_window_s": seconds,
                "passed_after_whole": end["fields"].get("passed") if end else None,
            }
        )
    return {
        "schema_version": 1,
        "whole_reruns_before": len(rows),
        "solo_reruns_with_load_evidence": sum(
            e["type"] == "rerun"
            and "test" in e["fields"]
            and e["fields"].get("load", {}).get("start") is not None
            and e["fields"].get("load", {}).get("end") is not None
            for e in events
        ),
        "repeated_failures_in_logs": sum(r["repeated_failure_in_log"] for r in rows),
        "skipped_after": sum(r["skip_with_normal_load_evidence"] for r in rows),
        "whole_reruns_after": sum(not r["skip_with_normal_load_evidence"] for r in rows),
        "measurable_windows": sum(r["rerun_window_s"] is not None for r in rows),
        "recorded_rerun_window_s": sum(r["rerun_window_s"] or 0 for r in rows),
        "saved_window_s": sum(
            r["rerun_window_s"] or 0 for r in rows if r["skip_with_normal_load_evidence"]
        ),
        "rows": rows,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("copied_state", type=Path)
    args = parser.parse_args()
    print(json.dumps(summarize(args.copied_state), indent=2))
