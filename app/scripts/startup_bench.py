#!/usr/bin/env python3
"""Time to interactive, the startup profile, idle memory and the cost of a pop-out, with
contract §14's statistics.

    python3 app/scripts/startup_bench.py [--runs 30] [--memory-launches 9] [--popout-launches 5]
        [--only tti,memory,popout,breakdown] [--app PATH] [--env NAME=VALUE]… [--out FILE]

Builds the app and the daemon with the workspace's `perf` profile (optimized like `release`,
with debug assertions on), then works in a throwaway ATRIUM_HOME. Not `--release`: a release
build ignores ATRIUM_HOME (`lifecycle::default_paths`), so it would use the user's real Atrium
folders and leave its daemon running there, and a release daemon refuses every app until
packaging gives it the app's identity ("no app identity is known"). Then:

  1. tti: cold start (warm launches): one warm-up launch, then `--runs` launches with
     `ATRIUM_BENCH=startup-exit`. Each reports `tti` (from `main` to the later of first paint and
     first input) and the startup profile: the script's spawn time, Rust's marks, the webview's
     navigation timing, boot.js, the bundle's start and each boot stage. Gate (macOS): median
     < 400 ms and p90 < 450 ms; Linux is a goal (D-041). Median and p90 per phase are printed.
  2. memory: idle, one window: `--memory-launches` launches with `ATRIUM_BENCH=startup`, each
     sampled at 1 Hz from 5 s to 65 s after TTI; per launch, the median of its samples. Gate:
     every launch < 150 MB. macOS: `phys_footprint` of the app and its WebKit processes. Linux
     (D-040): **private PSS** (PSS without shared-library pages, `linuxmem.py`) summed over the
     app and its WebKit processes, with full PSS, RSS, USS and GPU memory (DRM fdinfo) beside it,
     and private PSS per process (UI, web, network, helpers).
  3. popout (Linux): `--popout-launches` launches with `ATRIUM_BENCH=startup-popout`
     (`app/src/popout-bench.ts`): the main window shows an idle thread, sampled 5–35 s; the
     thread moves to its own window, sampled 5–35 s after it is ready. The cost of one pop-out
     is the difference of the two medians, per launch (D-028: measured, not capped).
  4. breakdown (Linux, diagnostic): one idle launch, and at 30 s after TTI the private PSS of
     each process split by kind and by its largest mappings.

`--app` measures another binary (an A/B against a saved build; it is not rebuilt). `--env`
sets a variable for every launch (an experiment, e.g. a WebKit setting). The daemon is
started by the app and left running between launches, as a user's would be. Run it on the
machine's own desktop session: it opens windows. Hold the GUI lock while it runs
(`flock /tmp/atrium-gui-measure.lock python3 app/scripts/startup_bench.py`).
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

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import linuxmem  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
APP = ROOT / "target" / "perf" / "atrium"
TIMEOUT_S = 60
WEBKIT = re.compile(r"WebKit\.(WebContent|GPU|Networking)|WebKitWebProcess|WebKitNetworkProcess")
LINUX = sys.platform.startswith("linux")
GATE_MB = 150
MIB_PER_MB = 1_000_000 / 1_048_576  # the budgets are in MB; the numbers are in MiB


def build() -> None:
    subprocess.run(["pnpm", "build"], cwd=ROOT / "app", check=True, stdout=subprocess.DEVNULL)
    subprocess.run(
        ["cargo", "build", "--profile", "perf", "-p", "atrium-app", "--features", "custom-protocol", "-p", "atrium-daemon"],
        cwd=ROOT, check=True,
    )


def reports(log: pathlib.Path) -> list[dict]:
    out = []
    for line in log.read_text(errors="replace").splitlines():
        if line.startswith("ATRIUM_REPORT "):
            report = json.loads(line[len("ATRIUM_REPORT "):])
            if report.get("kind") == "failure":
                raise SystemExit(f"the app failed; see {log}")
            out.append(report)
    return out


def launch(app: pathlib.Path, home: pathlib.Path, mode: str, log: pathlib.Path, extra_env: dict) -> tuple[subprocess.Popen, float]:
    env = dict(os.environ, ATRIUM_HOME=str(home), ATRIUM_BENCH=mode, **extra_env)
    out = open(log, "wb")
    spawned = time.time() * 1000
    return subprocess.Popen([str(app)], env=env, stdout=out, stderr=subprocess.STDOUT), spawned


def wait_report(app: subprocess.Popen, log: pathlib.Path, kind: str, test=lambda r: True, timeout=TIMEOUT_S) -> dict:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for report in reports(log):
            if report.get("kind") == kind and test(report):
                return report
        if app.poll() is not None:
            raise SystemExit(f"the app exited without a {kind} report; see {log}")
        time.sleep(0.05)
    raise SystemExit(f"timed out waiting for {kind}; see {log}")


def stop_app(app: subprocess.Popen) -> None:
    if app.poll() is None:
        app.kill()
    app.wait()


# -- statistics -------------------------------------------------------------------------------

def p90(values: list[float]) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, round(0.90 * (len(ordered) - 1)))]


def median(values: list[float]) -> float:
    return statistics.median(values)


# -- the startup profile ----------------------------------------------------------------------

def stage_at(report: dict, name: str) -> float | None:
    for stage, at in report.get("profile", {}).get("stages", []):
        if stage == name:
            return at
    return None


def phases(report: dict, spawned: float) -> dict[str, float]:
    """Durations in ms, in boot order, from one startup report (None where a mark is missing)."""
    p = report.get("profile", {})
    native = p.get("native", {})
    nav = p.get("navigation") or {}
    main = report["mainEpochMs"]
    points = [
        ("spawn", spawned),
        ("main", main),
        ("setupStart", native.get("setupStart")),
        ("bridgeStarted", native.get("bridgeStarted")),
        ("windowBuilt", native.get("windowBuilt")),
        ("navigationStart", p.get("timeOrigin")),
        ("htmlLoaded", nav.get("responseEnd")),
        ("bootJs", report.get("jsStart")),
        ("moduleStart", p.get("moduleStart")),
        ("bundleEvaluated", stage_at(report, "subscribe")),
        ("subscribed", stage_at(report, "startup-info")),
        ("startupInfo", stage_at(report, "render")),
        ("mounted", report.get("mounted")),
        ("firstPaint", report.get("firstPaint")),
    ]
    out: dict[str, float] = {}
    for (a, ta), (b, tb) in zip(points, points[1:]):
        out[f"{a} → {b}"] = tb - ta if ta is not None and tb is not None else None
    # After mount, the boot stages before the harness's probe (attach … ready).
    stages = p.get("stages", [])
    for (a, ta), (b, tb) in zip(stages, stages[1:]):
        if a in ("attach", "restore-windows", "after-mount", "native-paint", "ready"):
            out[f"stage {a}"] = tb - ta
    first_input = report.get("firstInput")
    out["firstPaint → firstInput"] = first_input - report["firstPaint"] if first_input is not None else None
    # The summary rows (cumulative from main, as M0's §2.4 table).
    out["= spawn → main"] = main - spawned
    out["= main → first JS (boot.js)"] = report["jsStart"] - main if report.get("jsStart") else None
    out["= first JS → mounted"] = report["mounted"] - report["jsStart"] if report.get("jsStart") and report.get("mounted") else None
    out["= main → first paint"] = report["firstPaint"] - main
    out["= main → input handled"] = first_input - main if first_input is not None else None
    out["= TTI (main → later of the two)"] = report["tti"]
    out["= spawn → TTI"] = report["tti"] + main - spawned
    return out


def print_phase_table(rows: list[dict[str, float]]) -> dict:
    names = list(rows[0].keys())
    summary = {}
    print(f"  {'phase':44s} {'median':>8s} {'p90':>8s} {'min':>8s} {'max':>8s}")
    for name in names:
        values = [r[name] for r in rows if r.get(name) is not None]
        if not values:
            continue
        summary[name] = {"median": median(values), "p90": p90(values), "min": min(values), "max": max(values)}
        s = summary[name]
        print(f"  {name:44s} {s['median']:8.1f} {s['p90']:8.1f} {s['min']:8.1f} {s['max']:8.1f}")
    return summary


# -- memory -----------------------------------------------------------------------------------

def webkit_pids() -> set[int]:
    listing = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True, text=True).stdout
    return {int(line.split(None, 1)[0]) for line in listing.splitlines() if WEBKIT.search(line)}


def footprint_mib(pid: int) -> float | None:
    out = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True).stdout
    found = re.search(r"Footprint:\s+([\d.]+)\s*(KB|MB|GB)", out)
    if not found:
        return None
    value, unit = float(found.group(1)), found.group(2)
    return value / 1024 if unit == "KB" else value * 1024 if unit == "GB" else value


def mem_sample(app_pid: int, mac_pids: list[int]) -> dict:
    """One sample in MiB. Linux: D-040's private PSS and the numbers beside it. macOS: footprint."""
    if not LINUX:
        values = [footprint_mib(pid) for pid in [app_pid, *mac_pids]]
        return {"budgeted": sum(v for v in values if v is not None)}
    s = linuxmem.sample(app_pid)
    t = s["total"]
    row = {
        "budgeted": linuxmem.mib(t["private_pss"]),
        "pss": linuxmem.mib(t["pss"]), "rss": linuxmem.mib(t["rss"]), "uss": linuxmem.mib(t["uss"]),
        "anon": linuxmem.mib(t["anon"]), "libDirty": linuxmem.mib(t["lib_dirty"]), "file": linuxmem.mib(t["file"]),
        "shmem": linuxmem.mib(t["shmem"]), "libClean": linuxmem.mib(t["lib_clean"]),
        "gpuResident": linuxmem.mib(s["gpu"]["resident"]) if s["gpu"] else None,
        "gpuTotal": linuxmem.mib(s["gpu"]["total"]) if s["gpu"] else None,
        "processes": s["processes"],
    }
    for role, values in s["roles"].items():
        row[f"role:{role}"] = linuxmem.mib(values["private_pss"])
    return row


def sample_window(app_pid: int, mac_pids: list[int], seconds: int) -> list[dict]:
    samples = []
    for _ in range(seconds):
        tick = time.monotonic()
        samples.append(mem_sample(app_pid, mac_pids))
        time.sleep(max(0.0, 1.0 - (time.monotonic() - tick)))
    return samples


def summarize(samples: list[dict]) -> dict:
    """Median of every number over the samples, the budgeted number's max, and the longest run
    of consecutive samples over the gate."""
    keys = {k for s in samples for k, v in s.items() if isinstance(v, (int, float))}
    out = {k: median([s[k] for s in samples if s.get(k) is not None]) for k in sorted(keys)}
    out["max"] = max(s["budgeted"] for s in samples)
    run = longest = 0
    for s in samples:
        run = run + 1 if s["budgeted"] * MIB_PER_MB > GATE_MB else 0
        longest = max(longest, run)
    out["longestOverGateS"] = longest
    return out


def mem_line(m: dict) -> str:
    if not LINUX:
        return f"footprint {m['budgeted']:.1f} MiB"
    gpu = f"GPU resident {m['gpuResident']:.1f} / total {m['gpuTotal']:.1f}" if m.get("gpuResident") is not None else "GPU none"
    roles = ", ".join(f"{k[5:]} {v:.1f}" for k, v in m.items() if k.startswith("role:"))
    return (f"private PSS {m['budgeted']:.1f} MiB ({m['budgeted'] / MIB_PER_MB:.1f} MB) [{roles}]; "
            f"PSS {m['pss']:.1f}, RSS {m['rss']:.1f}, USS {m['uss']:.1f}, {gpu}")


def idle_launches(app_path, home, logs, n, extra_env) -> list[dict]:
    launches = []
    for run in range(n):
        before = webkit_pids() if not LINUX else set()
        log = logs / f"idle-{run}.log"
        app, _ = launch(app_path, home, "startup", log, extra_env)
        try:
            wait_report(app, log, "startup")
            time.sleep(5)
            mac = sorted(webkit_pids() - before) if not LINUX else []
            samples = sample_window(app.pid, mac, 61)
        finally:
            stop_app(app)
        summary = summarize(samples)
        launches.append(summary)
        print(f"  launch {run + 1}: median {mem_line(summary)}; max sample {summary['max']:.1f} MiB")
    return launches


def popout_launches(app_path, home, logs, n, extra_env) -> list[dict]:
    out = []
    for run in range(n):
        log = logs / f"popout-{run}.log"
        app, _ = launch(app_path, home, "startup-popout", log, extra_env)
        try:
            wait_report(app, log, "startup")
            wait_report(app, log, "popout-stage", lambda r: r["stage"] == "baseline")
            time.sleep(5)
            before = summarize(sample_window(app.pid, [], 31))
            opened = wait_report(app, log, "popout-stage", lambda r: r["stage"] == "open", timeout=90)
            time.sleep(5)
            after = summarize(sample_window(app.pid, [], 31))
        finally:
            stop_app(app)
        delta = {k: after[k] - before[k] for k in after if k in before and isinstance(after[k], (int, float)) and k != "longestOverGateS"}
        out.append({"before": before, "after": after, "delta": delta, "readyMs": opened.get("readyMs")})
        roles = ", ".join(f"{k[5:]} {v:+.1f}" for k, v in delta.items() if k.startswith("role:"))
        gpu = f", GPU resident {delta['gpuResident']:+.1f}" if "gpuResident" in delta else ""
        print(f"  launch {run + 1}: one window {before['budgeted']:.1f} → with a pop-out {after['budgeted']:.1f} MiB: "
              f"private PSS {delta['budgeted']:+.1f} MiB [{roles}], PSS {delta.get('pss', 0):+.1f}, RSS {delta.get('rss', 0):+.1f}{gpu}; "
              f"pop-out ready {opened.get('readyMs', 0):.0f} ms after it was asked for")
    return out


def breakdown(app_path, home, logs, extra_env, top=12) -> dict:
    log = logs / "breakdown.log"
    app, _ = launch(app_path, home, "startup", log, extra_env)
    result = {}
    try:
        wait_report(app, log, "startup")
        time.sleep(30)
        for pid in linuxmem.app_tree(app.pid):
            names: dict = {}
            mem = linuxmem.smaps_kb(pid, names)
            if mem is None:
                continue
            role = linuxmem.role_of(linuxmem.exe_name(pid))
            biggest = sorted(names.items(), key=lambda kv: -kv[1])[:top]
            result[f"{role} {pid}"] = {"kinds": {k: linuxmem.mib(v) for k, v in mem.items()},
                                       "top": [(n, linuxmem.mib(v)) for n, v in biggest]}
            kinds = ", ".join(f"{k} {linuxmem.mib(v):.1f}" for k, v in mem.items())
            print(f"  {role} ({linuxmem.exe_name(pid)}, pid {pid}): {kinds}")
            for name, kb in biggest:
                print(f"      {linuxmem.mib(kb):7.1f} MiB  {name}")
        gpu = linuxmem.gpu_kb(linuxmem.app_tree(app.pid))
        print(f"  GPU (DRM fdinfo): {({k: round(linuxmem.mib(v), 1) for k, v in gpu.items()}) if gpu else 'none'}")
        result["gpu"] = gpu
    finally:
        stop_app(app)
    return result


# -- the daemon -------------------------------------------------------------------------------

def daemons_of(home: pathlib.Path) -> set[int]:
    """Every `atriumd` of this run: the one in the lock file and, on Linux, any whose
    ATRIUM_HOME is this run's folder (one the app was still starting has no lock file yet)."""
    found: set[int] = set()
    try:
        found.add(int(json.loads((home / "daemon.lock").read_text())["pid"]))
    except (OSError, ValueError, KeyError):
        pass
    wanted = f"ATRIUM_HOME={home}".encode()
    for proc in pathlib.Path("/proc").glob("[0-9]*"):
        try:
            if b"atriumd" in (proc / "cmdline").read_bytes() and wanted in (proc / "environ").read_bytes().split(b"\0"):
                found.add(int(proc.name))
        except OSError:
            continue
    return found


def stop_daemon(home: pathlib.Path) -> None:
    """Stops this run's daemon and doesn't return while one is alive: SIGINT (it stops its
    sessions and exits), then SIGKILL for one that is still there after a few seconds. A run
    that failed early can leave a daemon the app was still starting, so it looks twice."""
    for _ in range(2):
        pids = daemons_of(home)
        for pid in pids:
            try:
                os.kill(pid, signal.SIGINT)
            except OSError:
                pass
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and any(pathlib.Path(f"/proc/{pid}").exists() for pid in pids):
            time.sleep(0.1)
        for pid in pids:
            if pathlib.Path(f"/proc/{pid}").exists():
                try:
                    os.kill(pid, signal.SIGKILL)
                except OSError:
                    pass
        time.sleep(1)


# -- conditions -------------------------------------------------------------------------------

def conditions() -> str:
    load = " ".join(f"{v:.2f}" for v in os.getloadavg())
    busiest = subprocess.run(["ps", "-eo", "pcpu=,comm=", "--sort=-pcpu"], capture_output=True, text=True).stdout.splitlines()[:5]
    return f"load average {load}; busiest: " + "; ".join(" ".join(line.split()) for line in busiest)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=int, default=30)
    parser.add_argument("--memory-launches", type=int, default=9)
    parser.add_argument("--popout-launches", type=int, default=5)
    parser.add_argument("--only", default="tti,memory,popout" if LINUX else "tti,memory")
    parser.add_argument("--app", type=pathlib.Path, help="measure this binary instead of building")
    parser.add_argument("--env", action="append", default=[], help="NAME=VALUE for every launch")
    parser.add_argument("--out", type=pathlib.Path, help="write every number as JSON here")
    args = parser.parse_args()
    parts = set(args.only.split(","))
    extra_env = dict(item.split("=", 1) for item in args.env)
    app_path = args.app or APP
    if args.app is None:
        build()
    print(f"binary {app_path}; extra env {extra_env or 'none'}")
    print(f"at start: {conditions()}")
    home = pathlib.Path(tempfile.mkdtemp(prefix="atrium-bench-", dir="/tmp"))
    home.chmod(0o700)
    logs = home / "logs"
    logs.mkdir()
    result: dict = {"kind": "startup-bench", "binary": str(app_path), "env": extra_env, "startConditions": conditions()}
    try:
        if "tti" in parts:
            ttis, rows, raw = [], [], []
            for run in range(args.runs + 1):
                log = logs / f"tti-{run}.log"
                app, spawned = launch(app_path, home, "startup-exit", log, extra_env)
                report = wait_report(app, log, "startup")
                try:
                    app.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    stop_app(app)
                if run > 0:  # run 0 is the warm-up
                    ttis.append(report["tti"])
                    rows.append(phases(report, spawned))
                    raw.append(report | {"spawnEpochMs": spawned})
            tti_pass = median(ttis) < 400 and p90(ttis) < 450
            print(f"TTI over {len(ttis)} warm launches: median {median(ttis):.0f} ms, p90 {p90(ttis):.0f} ms, "
                  f"min {min(ttis):.0f}, max {max(ttis):.0f} ({'within' if tti_pass else 'OVER'} the macOS gate"
                  f"{'; a goal on Linux, D-041' if LINUX else ''})")
            print("Startup profile (ms):")
            result.update(ttiMs=ttis, ttiPass=tti_pass, phases=print_phase_table(rows), phaseRows=rows, reports=raw)

        if "memory" in parts:
            kind = "phys_footprint" if not LINUX else "private PSS (D-040)"
            print(f"Idle memory, one window, {args.memory_launches} launches (median of 1 Hz samples 5–65 s after TTI):")
            launches = idle_launches(app_path, home, logs, args.memory_launches, extra_env)
            memory_pass = all(m["budgeted"] * MIB_PER_MB < GATE_MB for m in launches)
            worst = max(m["budgeted"] for m in launches)
            print(f"Idle memory ({kind}): per-launch medians {min(m['budgeted'] for m in launches):.1f}–{worst:.1f} MiB "
                  f"({worst / MIB_PER_MB:.1f} MB at worst), worst sample {max(m['max'] for m in launches):.1f} MiB, "
                  f"longest run over {GATE_MB} MB {max(m['longestOverGateS'] for m in launches)} s "
                  f"({'within' if memory_pass else 'OVER'} the gate of {GATE_MB} MB)")
            result.update(memory=launches, memoryPass=memory_pass)

        if "popout" in parts and LINUX:
            print(f"One pop-out, {args.popout_launches} launches (medians of 1 Hz samples, 5–35 s, before and after):")
            pops = popout_launches(app_path, home, logs, args.popout_launches, extra_env)
            costs = [p["delta"]["budgeted"] for p in pops]
            print(f"Cost of one pop-out (private PSS): median {median(costs):.1f} MiB (min {min(costs):.1f}, max {max(costs):.1f}); "
                  f"PSS {median([p['delta']['pss'] for p in pops]):.1f}, RSS {median([p['delta']['rss'] for p in pops]):.1f}"
                  + (f", GPU resident {median([p['delta']['gpuResident'] for p in pops]):.1f}" if all('gpuResident' in p['delta'] for p in pops) else "")
                  + " MiB (medians)")
            result.update(popout=pops)

        if "breakdown" in parts and LINUX:
            print("Breakdown of one idle launch at 30 s after TTI (MiB; `top` is each mapping's counted PSS):")
            result.update(breakdown=breakdown(app_path, home, logs, extra_env))

        result["endConditions"] = conditions()
        print(f"at end: {result['endConditions']}")
        if args.out:
            args.out.write_text(json.dumps(result, indent=1))
        print(json.dumps({k: result[k] for k in ("kind", "ttiPass", "memoryPass") if k in result}))
        return 0
    finally:
        stop_daemon(home)
        time.sleep(0.5)
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
