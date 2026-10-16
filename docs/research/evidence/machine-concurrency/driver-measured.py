#!/usr/bin/env python3
"""#39 small-sample calibration. Run under the real native-build lock; see research."""
import concurrent.futures
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def execute():
    stamp = Path(sys.argv[2])
    command = [a.replace("@SIM@", os.environ.get("KR_SIM_UDID", "")) for a in sys.argv[3:]]
    start = time.monotonic()
    code = subprocess.call(command)
    write(stamp, {"duration_s": time.monotonic() - start, "exit_code": code})
    return code


def calibrate():
    root = Path(sys.argv[1]).resolve()
    sample = root / "sample"
    kr = str(root / "bin/kr")
    env = dict(os.environ, KR_CONFIG=str(root / "machine.toml"),
               KR_STATE_DIR=str(root / "state"))
    env.pop("KR_HELD", None)
    result_dir = root / "results"
    result_dir.mkdir(exist_ok=True)
    baseline = json.loads(subprocess.check_output([kr, "doctor", "--json"], cwd=sample, env=env))
    write(result_dir / "doctor.json", baseline)
    limit = baseline["capacity"]["sim"]["value"] + 1
    (sample / ".kr.toml").write_text(f'''[project]
name = "Krill"
xcodeproj = "KrSample.xcodeproj"
scheme = "KrSample"
[simulators]
runtime = "iOS 18.6"
[simulators.devices.default]
device_type = "iPhone 16 Pro"
count = {limit}
''')
    for command in [[kr, "lock", "build", "--", "xcodegen", "generate", "--quiet"],
                    [kr, "pool", "sync"]]:
        subprocess.run(command, cwd=sample, env=env, check=True)
    samples = []
    batches = []
    stop = threading.Event()
    active = set()
    mutex = threading.Lock()

    def monitor():
        while not stop.is_set():
            pressure = int(subprocess.check_output(["sysctl", "-n", "kern.memorystatus_vm_pressure_level"]))
            samples.append({"at": time.time(), "load": os.getloadavg(), "pressure": pressure})
            if pressure in (2, 4):
                stop.set()
                with mutex:
                    for child in active:
                        child.send_signal(signal.SIGTERM)
                break
            stop.wait(1)

    observer = threading.Thread(target=monitor)
    observer.start()

    def item(kind, capacity, name, index, products=None):
        if stop.is_set():
            return {"exit_code": None, "stopped": True}
        stamp = result_dir / f"{name}-{index}.json"
        result = result_dir / f"{name}-{index}.xcresult"
        if kind == "build":
            args = [kr, "lock", "build", "--", "python3", __file__, "--exec", str(stamp),
                    "xcodebuild", "build-for-testing", "-quiet", "-project", "KrSample.xcodeproj",
                    "-scheme", "KrSample", "-destination", "generic/platform=iOS Simulator",
                    "-derivedDataPath", str(root / f"dd-{name}-{index}"),
                    "CODE_SIGNING_ALLOWED=NO"]
        else:
            args = [kr, "pool", "lease", "--", "python3", __file__, "--exec", str(stamp),
                    "xcodebuild", "test-without-building", "-quiet", "-xctestrun", str(products),
                    "-destination", "id=@SIM@", "-parallel-testing-enabled", "NO",
                    "-only-testing:KrSampleUITests/LaunchUITests/testShowsGreeting",
                    "-resultBundlePath", str(result)]
        item_env = dict(env, **{f"KR_RESOURCES__{kind.upper()}": str(capacity)})
        start = time.monotonic()
        with (result_dir / f"{name}-{index}.log").open("w") as log:
            with mutex:
                if stop.is_set():
                    return {"exit_code": None, "stopped": True}
                child = subprocess.Popen(args, cwd=sample, env=item_env, stdout=log, stderr=log)
                active.add(child)
            try:
                code = child.wait(timeout=300)
            except subprocess.TimeoutExpired:
                child.terminate()
                code = child.wait(timeout=60)
            finally:
                with mutex:
                    active.remove(child)
        actual = json.loads(stamp.read_text()) if stamp.exists() else {}
        out = {"index": index, "exit_code": code, "wall_s": time.monotonic() - start, **actual}
        if kind == "sim" and result.exists():
            summary = subprocess.run([kr, "lock", "light", "--", "xcrun", "xcresulttool", "get",
                                      "test-results", "summary", "--path", str(result)],
                                     cwd=sample, env=env, capture_output=True, text=True)
            if summary.returncode == 0:
                value = json.loads(summary.stdout)
                write(result_dir / f"{name}-{index}-summary.json", value)
                out["passed_tests"] = value.get("passedTests", 0)
                out["failed_tests"] = value.get("failedTests", 0)
            if out.get("passed_tests") != 1 or out.get("failed_tests") != 0:
                out["exit_code"] = 1
        return out

    try:
        # Paired A/B/A/B, two fresh DerivedData builds in each batch.
        for repeat in range(2):
            for capacity in (1, 2):
                name = f"build-{capacity}-r{repeat}"
                start = time.monotonic()
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
                    items = list(executor.map(lambda i: item("build", capacity, name, i), range(2)))
                batches.append({"kind": "build", "capacity": capacity, "repeat": repeat,
                                "wall_s": time.monotonic() - start, "items": items})
                write(result_dir / "batches.json", batches)
                print(name, batches[-1]["wall_s"], items, flush=True)
                if stop.is_set() or any(i.get("exit_code") != 0 for i in items):
                    return 1
        products = next((root / "dd-build-1-r0-0/Build/Products").glob("*.xctestrun"))
        # Six identical launch tests at each setting; reverse the second sweep to
        # expose warm-up/time drift, rather than using only the best observation.
        for repeat in range(2):
            capacities = list(range(2, limit + 1))
            if repeat:
                capacities.reverse()
            for capacity in capacities:
                name = f"sim-{capacity}-r{repeat}"
                start = time.monotonic()
                with concurrent.futures.ThreadPoolExecutor(max_workers=capacity) as executor:
                    items = list(executor.map(lambda i: item("sim", capacity, name, i, products), range(6)))
                batches.append({"kind": "sim", "capacity": capacity, "repeat": repeat,
                                "wall_s": time.monotonic() - start, "items": items})
                write(result_dir / "batches.json", batches)
                print(name, batches[-1]["wall_s"], items, flush=True)
                if stop.is_set() or any(i.get("exit_code") != 0 for i in items):
                    return 1
        return 0
    finally:
        stop.set()
        observer.join()
        write(result_dir / "load.json", samples)


if __name__ == "__main__":
    sys.exit(execute() if sys.argv[1] == "--exec" else calibrate())
