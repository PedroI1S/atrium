#!/usr/bin/env python3
"""Measurement harness for the M0-06 UI shell (macOS).

  scripts/measure.py cold   [--runs 10] [--bin PATH]   time to interactive + first paint
  scripts/measure.py rss    [--settle 10] [--bin PATH] idle memory: app + new WebKit helpers
  scripts/measure.py frames [--runs 3]  [--bin PATH]   rAF frame times (scroll, resize, theme swap)

Method notes are in docs/spikes/M0-06-ui-shell.md. The binary should be the release bundle:
  src-tauri/target/release/bundle/macos/Atrium UI Shell.app/Contents/MacOS/atrium-ui-shell
"""
import argparse, ctypes, json, os, re, signal, statistics, subprocess, sys, tempfile, time

DEFAULT_BIN = "src-tauri/target/release/bundle/macos/Atrium UI Shell.app/Contents/MacOS/atrium-ui-shell"


# Every command pins the state the app starts in, so results never depend on what ran before
# (the app persists the last chosen theme). Printed with every result.
CONFIG = {"ATRIUM_THEME": "atrium-calm-dark", "ATRIUM_PRESET": "default", "ATRIUM_TAB": "auth", "ATRIUM_WINDOWS": "1", "ATRIUM_OPACITY": "off", "ATRIUM_PALETTE": "", "ATRIUM_RIGHT_TAB": "diff"}


def print_config(binary):
    print("config: " + " ".join(f"{k}={v}" for k, v in CONFIG.items()) + f"  bin={binary}")
    print(f"host: {subprocess.check_output(['sw_vers', '-productVersion'], text=True).strip()}, "
          f"{subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip()}")


def launch(binary, env_extra):
    env = dict(os.environ, **CONFIG, **env_extra)
    t0 = time.time() * 1000.0  # ms since epoch, taken right before exec
    p = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return p, t0


def wait_report(path, timeout=40):
    end = time.time() + timeout
    while time.time() < end:
        if os.path.exists(path) and os.path.getsize(path) > 0:
            time.sleep(0.05)
            with open(path) as f:
                return json.load(f)
        time.sleep(0.02)
    raise TimeoutError(path)


def validate_report(r):
    c = r["configuration"]
    expected = int(CONFIG["ATRIUM_WINDOWS"])
    assert c["readyWindows"] == expected and c["windows"] == expected, c
    assert c["opacity"] == CONFIG["ATRIUM_OPACITY"], c
    assert c["settings"]["theme"] == CONFIG["ATRIUM_THEME"], c
    assert c["settings"]["alpha"] == (1 if c["opacity"] == "off" else 0.6), c
    applied = c["applied"]
    assert not applied["error"] and not applied["accessibilityOpaque"], c
    assert applied["nativeAlpha"] == (0.6 if c["opacity"] == "whole" else 1), c
    assert applied["chromeAlpha"] == (0.6 if c["opacity"] == "background" else 1), c
    assert applied["contentAlpha"] == (0.74 if c["opacity"] == "background" else 1), c
    if c["opacity"] == "background": assert applied["vibrancy"] == "true", c
    print("verified app configuration:", json.dumps(c, sort_keys=True))
    print("raw report:", json.dumps(r, sort_keys=True))


def stop(p):
    p.send_signal(signal.SIGTERM)
    try:
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        p.kill()
        p.wait(timeout=5)


def med(xs):
    return statistics.median(xs)


def cold(a):
    print_config(a.bin)
    rows = []
    for i in range(a.runs):
        out = tempfile.mktemp(suffix=".json")
        p, t0 = launch(a.bin, {"ATRIUM_BENCH": "startup", "ATRIUM_BENCH_OUT": out})
        try:
            r = wait_report(out)
            validate_report(r)
        finally:
            stop(p)
            if os.path.exists(out): os.unlink(out)
        m = r["marks"]
        rows.append({
            "exec_to_main": r["mainEpochMs"] - t0,
            "main_to_js": m["jsStart"] - r["mainEpochMs"],
            "js_to_mounted": m["mounted"] - m["jsStart"],
            "first_contentful_paint": m.get("firstContentfulPaint", float("nan")) - t0,
            "first_paint_raf2": m["firstPaintRaf"] - t0,
            "tti_keystroke": m["firstInputHandled"] - t0,
            "tti_palette": m["paletteReady"] - t0,
        })
        print(f"run {i+1:2d}: " + "  ".join(f"{k}={v:7.1f}" for k, v in rows[-1].items()))
        time.sleep(0.5)
    print("\nms from process launch (run 1 = first launch after build, rest = warm file cache)")
    for k in rows[0]:
        xs = [r[k] for r in rows]
        rest = xs[1:] or xs
        print(f"{k:24s} first={xs[0]:7.1f}  median(all)={med(xs):7.1f}  median(2..n)={med(rest):7.1f}  min={min(xs):7.1f}  max={max(xs):7.1f}")
    tti = [r["tti_keystroke"] for r in rows]
    print(f"\nTTI vs the 400 ms budget: {sum(1 for x in tti if x > 400)}/{len(tti)} launches over 400 ms; "
          f"median {med(tti):.0f} ms, worst {max(tti):.0f} ms")


def ps_table():
    out = subprocess.check_output(["ps", "-axo", "pid=,ppid=,rss=,comm="], text=True)
    rows = []
    for line in out.splitlines():
        m = re.match(r"\s*(\d+)\s+(\d+)\s+(\d+)\s+(.*)$", line)
        if m:
            rows.append((int(m[1]), int(m[2]), int(m[3]), m[4]))
    return rows


def footprint_mb(pid):
    """Physical footprint in MB from `footprint`, or None if it can't be read."""
    try:
        out = subprocess.check_output(["footprint", "-p", str(pid), "--noCategories"], text=True, stderr=subprocess.DEVNULL)
    except Exception:
        return None
    m = re.search(r"Footprint:\s+([\d.]+)\s*(KB|MB|GB)", out)
    if not m:
        return None
    v = float(m[1])
    return v / 1024 if m[2] == "KB" else v * 1024 if m[2] == "GB" else v


def responsible_pid_fn():
    """macOS `responsibility_get_pid_responsible_for_pid` (private; what Activity Monitor uses to
    group WebKit's XPC helpers under their app). Returns None if it is unavailable."""
    try:
        f = ctypes.CDLL("/usr/lib/system/libquarantine.dylib").responsibility_get_pid_responsible_for_pid
    except (OSError, AttributeError):
        return None
    f.argtypes = [ctypes.c_int]
    f.restype = ctypes.c_int
    return f


def owned_processes(app_pid, resp):
    """The app plus every WebKit helper whose responsible process is the app. Helpers of other apps
    (Safari, other web views) are excluded by ownership, not by timing."""
    rows = []
    others = 0
    for pid, ppid, kb, comm in ps_table():
        if pid == app_pid:
            rows.append((pid, ppid, kb, comm, app_pid))
        elif "WebKit" in comm:
            r = resp(pid)
            if r == app_pid:
                rows.append((pid, ppid, kb, comm, r))
            else:
                others += 1
    return rows, others


def snapshot(app_pid, resp, label):
    rows, others = owned_processes(app_pid, resp)
    print(f"{label}:")
    fps, bad = [], []
    for pid, ppid, kb, comm, resp_pid in rows:
        fp = footprint_mb(pid)
        fps.append(fp)
        if fp is None:
            bad.append(pid)
        print(f"  pid {pid:6d}  responsible {resp_pid:6d}  rss {kb/1024:7.1f} MB  footprint {'UNREADABLE' if fp is None else f'{fp:6.1f} MB'}  {os.path.basename(comm)}")
    total_rss = sum(r[2] for r in rows) / 1024
    print(f"  ({others} WebKit processes belonging to other apps ignored)")
    kinds = {os.path.basename(r[3]) for r in rows}
    missing = {"com.apple.WebKit.WebContent", "com.apple.WebKit.Networking"} - kinds
    if bad or missing:
        print(f"  INCOMPLETE: unreadable footprints for pids {bad}; missing helpers {sorted(missing)}. No total reported.")
        return None
    print(f"  TOTAL rss {total_rss:.1f} MB   footprint {sum(fps):.1f} MB   ({len(rows)} processes)")
    return total_rss, sum(fps)


def launch_via_open(binary, extra):
    """Launches the .app with `open -n`, which makes the app its own 'responsible process'; WebKit's
    helpers are then attributable to it exactly (a direct exec would inherit the launching terminal as
    responsible, and every other web view under it would look the same). Returns the app's pid."""
    app = os.path.abspath(os.path.join(os.path.dirname(binary), "..", ".."))
    exe = os.path.abspath(binary)
    before = {r[0] for r in ps_table() if r[3] == exe}
    cmd = ["open", "-n"]
    for k, v in dict(CONFIG, **extra).items():
        cmd += ["--env", f"{k}={v}"]
    subprocess.check_call(cmd + [app])
    for _ in range(100):
        time.sleep(0.1)
        new = [r[0] for r in ps_table() if r[3] == exe and r[0] not in before]
        if len(new) == 1:
            return new[0]
    sys.exit("could not identify the launched app process")


def rss(a):
    print_config(a.bin)
    resp = responsible_pid_fn()
    if resp is None:
        sys.exit("cannot determine helper ownership on this macOS (responsibility API missing); refusing to guess")
    out = tempfile.mktemp(suffix=".json")
    pid = launch_via_open(a.bin, {"ATRIUM_BENCH": "startup", "ATRIUM_BENCH_OUT": out})
    if resp(pid) != pid:
        terminate_pid(pid)
        sys.exit(f"app pid {pid} is not its own responsible process (responsible={resp(pid)}); ownership would be ambiguous")
    ok = True
    try:
        validate_report(wait_report(out))
        time.sleep(a.settle)
        ok &= snapshot(pid, resp, f"idle {a.settle} s after launch") is not None
        time.sleep(20)
        ok &= snapshot(pid, resp, f"idle {a.settle + 20} s after launch") is not None
    finally:
        terminate_pid(pid)
        if os.path.exists(out): os.unlink(out)
    if not ok:
        sys.exit(1)


def frames(a):
    print_config(a.bin)
    for i in range(a.runs):
        out = tempfile.mktemp(suffix=".json")
        p, _ = launch(a.bin, {"ATRIUM_BENCH": "frames", "ATRIUM_BENCH_OUT": out})
        try:
            r = wait_report(out, timeout=90)
            validate_report(r)
        finally:
            stop(p)
            if os.path.exists(out): os.unlink(out)
        print(f"--- run {i+1}  (dpr {r['dpr']}, viewport {r['viewport']})")
        for k in ("idleFrames", "scrollReading", "scrollFling", "resize"):
            s = r[k]
            extra = f"  dist {s['distancePx']} px" if "distancePx" in s else ""
            w = s.get("handler") or s.get("step")
            if w:
                label = "scroll-handler+forced-layout" if "handler" in s else "resize-step+forced-layout"
                extra += f"\n{'':14s}   {label}: mean {w['meanMs']:.2f} p95 {w['p95Ms']:.2f} max {w['maxMs']:.2f} ms (synchronous part only)"
            rw = s.get("renderWork")
            if rw:
                extra += f"\n{'':14s}   rAF->next task (style/layout/paint-record/ResizeObserver): mean {rw['meanMs']:.2f} p95 {rw['p95Ms']:.2f} max {rw['maxMs']:.2f} ms"
            print(f"{k:14s} {s['fps']:6.1f} fps  mean {s['meanMs']:5.2f}  p50 {s['p50Ms']:5.2f}  p95 {s['p95Ms']:5.2f}  p99 {s['p99Ms']:5.2f}  max {s['maxMs']:6.2f} ms  >16.7ms {s['over16_7ms']}/{s['frames']}  >25ms {s['over25ms']}{extra}")
        print("theme swap to next frame (ms):", [round(x, 1) for x in r["themeSwapToNextFrameMs"]])


def terminate_pid(pid):
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    for _ in range(50):
        if not any(r[0] == pid for r in ps_table()): return
        time.sleep(0.1)
    os.kill(pid, signal.SIGKILL)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["cold", "rss", "frames"])
    ap.add_argument("--runs", type=int, default=None)
    ap.add_argument("--settle", type=int, default=10)
    ap.add_argument("--bin", default=DEFAULT_BIN)
    ap.add_argument("--theme", default=None, help="starting theme (default atrium-calm-dark)")
    ap.add_argument("--windows", type=int, choices=[1, 3], default=1)
    ap.add_argument("--opacity", choices=["off", "background", "whole"], default="off")
    a = ap.parse_args()
    CONFIG["ATRIUM_WINDOWS"] = str(a.windows)
    CONFIG["ATRIUM_OPACITY"] = a.opacity
    if a.theme:
        CONFIG["ATRIUM_THEME"] = a.theme
    os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    if a.runs is None:
        a.runs = 10 if a.mode == "cold" else 3
    if a.runs < 1 or a.settle < 0: ap.error("runs must be positive; settle must be nonnegative")
    {"cold": cold, "rss": rss, "frames": frames}[a.mode](a)
