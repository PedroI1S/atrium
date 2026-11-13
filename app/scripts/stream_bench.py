#!/usr/bin/env python3
"""The streaming run of contract §14, with the synthetic agent (M1-09b DoD).

    python3 app/scripts/stream_bench.py [--runs 3] [--debug]

Builds release binaries of the app and the daemon (`--debug` uses the debug ones, for trying
the script), then, `--runs` times, each in a throwaway ATRIUM_HOME: starts the app with
ATRIUM_BENCH=stream (app/src/stream-bench.ts), which starts 10 synthetic threads at
1,000 tokens/s each for 30 s, one token per event, shows one and holds the models of all ten.

Per run it prints the app's frame statistics (requestAnimationFrame deltas: WebKitGTK gives a
page no presentation timestamps) against the gate (at most 1 % of frames over 16.7 ms, p99 at
most 33 ms), and the peak memory while it ran, sampled at 1 Hz: the web content process, and
the app with all its webview processes (private PSS on Linux, `phys_footprint` on macOS).

The window must stay visible and uncovered: a hidden window gets no animation frames, and such
a run is reported as invalid. Run it on the machine's own desktop session.
"""

import argparse
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time

import startup_bench
from startup_bench import ROOT, memory_mib, report_of, stop_daemon, webkit_pids

TIMEOUT_S = 180


def command_of(pid: int) -> str:
    return subprocess.run(["ps", "-o", "command=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()


def one_run(app_path: pathlib.Path, run: int) -> dict:
    home = pathlib.Path(tempfile.mkdtemp(prefix="atrium-stream-", dir="/tmp"))
    home.chmod(0o700)
    log = home / "app.log"
    startup_bench.APP = app_path
    before = webkit_pids()
    app = startup_bench.launch(home, "stream", log)
    peak_web = peak_total = 0.0
    samples = 0
    try:
        deadline = time.monotonic() + TIMEOUT_S
        report = None
        while report is None:
            if time.monotonic() > deadline:
                raise SystemExit(f"timed out; see {log}")
            if app.poll() is not None:
                raise SystemExit(f"the app exited without a report; see {log}")
            tick = time.monotonic()
            ours = sorted(webkit_pids() - before)
            web = [memory_mib(pid) or 0.0 for pid in ours if "WebContent" in command_of(pid) or "WebKitWebProcess" in command_of(pid)]
            rest = [memory_mib(pid) or 0.0 for pid in [app.pid, *ours]]
            peak_web = max(peak_web, max(web, default=0.0))
            peak_total = max(peak_total, sum(rest))
            samples += 1
            time.sleep(max(0.0, 1.0 - (time.monotonic() - tick)))
            report = report_of(log, "stream")
    finally:
        app.kill()
        app.wait()
        stop_daemon(home)
        time.sleep(0.5)
        shutil.rmtree(home, ignore_errors=True)
    report["run"] = run
    report["peakWebContentMiB"] = round(peak_web, 1)
    report["peakAppAndWebviewsMiB"] = round(peak_total, 1)
    report["memorySamples"] = samples
    return report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--debug", action="store_true")
    args = parser.parse_args()
    profile = "debug" if args.debug else "release"
    subprocess.run(["pnpm", "build"], cwd=ROOT / "app", check=True, stdout=subprocess.DEVNULL)
    subprocess.run(
        ["cargo", "build", *([] if args.debug else ["--release"]), "-p", "atrium-app", "--features", "custom-protocol", "-p", "atrium-daemon"],
        cwd=ROOT, check=True,
    )
    app_path = ROOT / "target" / profile / "atrium"
    kind = "phys_footprint" if sys.platform == "darwin" else "private PSS"
    reports = []
    for run in range(1, args.runs + 1):
        report = one_run(app_path, run)
        reports.append(report)
        frame = report["frameMs"]
        valid = not report["hidden"] and report["hiddenDuring"] == 0 and report["frames"] > 0
        within = report["overGateFramePct"] <= 1.0 and frame["p99"] <= 33
        print(f"run {run}: {report['tokensPerSecond']} tokens/s over {report['seconds']} s, {report['fps']} fps, "
              f"frame p50 {frame['p50']} / p95 {frame['p95']} / p99 {frame['p99']} / max {frame['max']} ms, "
              f"{report['overGateFramePct']} % of {report['frames']} frames over 16.7 ms, "
              f"{report['longFrames']} long frames ({report['droppedFrames']} dropped), "
              f"web content peak {report['peakWebContentMiB']} MiB, app and webviews peak {report['peakAppAndWebviewsMiB']} MiB ({kind})"
              f"{'' if valid else '  INVALID: the window was hidden'}"
              f"  -> {'within' if within else 'OVER'} the gate")
        time.sleep(2)
    print(json.dumps({"kind": "stream-bench", "profile": profile, "memory": kind, "runs": reports}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
