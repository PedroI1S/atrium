#!/usr/bin/env python3
"""Time to interactive and idle memory of the app shell, with contract §14's statistics.

    python3 app/scripts/startup_bench.py [--runs 30] [--memory-launches 9]

Builds release binaries of the app and the daemon, then, in a throwaway ATRIUM_HOME:

  1. Cold start (warm launches): one warm-up launch, then `--runs` launches with
     `ATRIUM_BENCH=startup-exit`; each reports `tti` (from `main` to the later of first
     paint and first input), in ms. Gate (macOS): median < 400 ms and p90 < 450 ms.
  2. Idle memory, one window: `--memory-launches` launches with `ATRIUM_BENCH=startup`,
     each sampled at 1 Hz from 5 s to 65 s after TTI; per launch, the median of its
     samples. Gate: every launch < 150 MB. The memory is the app's and its webview
     processes': `phys_footprint` on macOS (`footprint`), private PSS on Linux (D-040).

The daemon is started by the app and left running between launches, as a user's would be.
Run it on the machine's own desktop session: it opens windows.
"""

import argparse
import json
import os
import pathlib
import re
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
APP = ROOT / "target" / "release" / "atrium"
TIMEOUT_S = 60
WEBKIT = re.compile(r"WebKit\.(WebContent|GPU|Networking)|WebKitWebProcess|WebKitNetworkProcess")


def build() -> None:
    subprocess.run(["pnpm", "build"], cwd=ROOT / "app", check=True, stdout=subprocess.DEVNULL)
    subprocess.run(
        ["cargo", "build", "--release", "-p", "atrium-app", "--features", "custom-protocol", "-p", "atrium-daemon"],
        cwd=ROOT, check=True,
    )


def report_of(log: pathlib.Path, kind: str) -> dict | None:
    for line in log.read_text(errors="replace").splitlines():
        if line.startswith("ATRIUM_REPORT "):
            report = json.loads(line[len("ATRIUM_REPORT "):])
            if report.get("kind") == kind:
                return report
            if report.get("kind") == "failure":
                raise SystemExit(f"the app failed; see {log}")
    return None


def launch(home: pathlib.Path, mode: str, log: pathlib.Path) -> subprocess.Popen:
    env = dict(os.environ, ATRIUM_HOME=str(home), ATRIUM_BENCH=mode)
    out = open(log, "wb")
    return subprocess.Popen([str(APP)], env=env, stdout=out, stderr=subprocess.STDOUT)


def wait_report(app: subprocess.Popen, log: pathlib.Path) -> dict:
    deadline = time.monotonic() + TIMEOUT_S
    while time.monotonic() < deadline:
        report = report_of(log, "startup")
        if report is not None:
            return report
        if app.poll() is not None:
            raise SystemExit(f"the app exited without a report; see {log}")
        time.sleep(0.05)
    raise SystemExit(f"timed out; see {log}")


def webkit_pids() -> set[int]:
    listing = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True, text=True).stdout
    return {int(line.split(None, 1)[0]) for line in listing.splitlines() if WEBKIT.search(line)}


def memory_mib(pid: int) -> float | None:
    if sys.platform == "darwin":
        out = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True).stdout
        found = re.search(r"Footprint:\s+([\d.]+)\s*(KB|MB|GB)", out)
        if not found:
            return None
        value, unit = float(found.group(1)), found.group(2)
        return value / 1024 if unit == "KB" else value * 1024 if unit == "GB" else value
    try:
        rollup = pathlib.Path(f"/proc/{pid}/smaps_rollup").read_text()
    except OSError:
        return None
    kib = sum(int(m.group(1)) for m in re.finditer(r"^Private_(?:Clean|Dirty):\s+(\d+) kB", rollup, re.M))
    return kib / 1024


def stop_daemon(home: pathlib.Path) -> None:
    try:
        os.kill(int(json.loads((home / "daemon.lock").read_text())["pid"]), signal.SIGINT)
    except (OSError, ValueError, KeyError):
        pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=30)
    parser.add_argument("--memory-launches", type=int, default=9)
    args = parser.parse_args()
    build()
    home = pathlib.Path(tempfile.mkdtemp(prefix="atrium-bench-", dir="/tmp"))
    home.chmod(0o700)
    logs = home / "logs"
    logs.mkdir()
    try:
        ttis = []
        for run in range(args.runs + 1):
            log = logs / f"tti-{run}.log"
            app = launch(home, "startup-exit", log)
            report = wait_report(app, log)
            try:
                app.wait(timeout=10)
            except subprocess.TimeoutExpired:
                app.kill()
                app.wait()
            if run > 0:  # run 0 is the warm-up
                ttis.append(report["tti"])
        ttis.sort()
        p90 = ttis[min(len(ttis) - 1, round(0.90 * (len(ttis) - 1)))]
        tti_pass = statistics.median(ttis) < 400 and p90 < 450
        print(f"TTI over {len(ttis)} warm launches: median {statistics.median(ttis):.0f} ms, "
              f"p90 {p90:.0f} ms, min {ttis[0]:.0f}, max {ttis[-1]:.0f} "
              f"({'within' if tti_pass else 'OVER'} the macOS gate)")

        launches = []
        for run in range(args.memory_launches):
            before = webkit_pids()
            log = logs / f"idle-{run}.log"
            app = launch(home, "startup", log)
            try:
                wait_report(app, log)
                time.sleep(5)
                ours = sorted(webkit_pids() - before)
                samples = []
                for _ in range(61):
                    tick = time.monotonic()
                    values = [memory_mib(pid) for pid in [app.pid, *ours]]
                    samples.append(sum(v for v in values if v is not None))
                    time.sleep(max(0.0, 1.0 - (time.monotonic() - tick)))
            finally:
                app.kill()
                app.wait()
            launches.append({"median": statistics.median(samples), "max": max(samples), "webkitProcesses": len(ours)})
            print(f"  launch {run + 1}: median {launches[-1]['median']:.1f} MiB, max {launches[-1]['max']:.1f} MiB, "
                  f"{len(ours)} webview processes")
        kind = "phys_footprint" if sys.platform == "darwin" else "private PSS"
        memory_pass = all(launch["median"] < 150 for launch in launches)
        print(f"Idle memory ({kind}), {len(launches)} launches: worst median "
              f"{max(l['median'] for l in launches):.1f} MiB, worst sample {max(l['max'] for l in launches):.1f} MiB "
              f"({'within' if memory_pass else 'OVER'} the gate)")
        print(json.dumps({"kind": "startup-bench", "ttiMs": ttis, "ttiPass": tti_pass,
                          "memory": launches, "memoryPass": memory_pass}))
        return 0
    finally:
        stop_daemon(home)
        time.sleep(0.5)
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
