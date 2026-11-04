#!/usr/bin/env python3
"""Measurement harness for the layout-B UI shell (macOS and Linux/WebKitGTK).

  scripts/measure.py cold   [--runs 10] [--bin PATH]  time to interactive + first paint
  scripts/measure.py coldmatrix [--runs 10]          cold, all six configurations interleaved
  scripts/measure.py rss    [--settle 10] [--bin PATH] idle memory: app + its WebKit helpers
  scripts/measure.py frames [--runs 3]  [--bin PATH]   rAF frame times (scroll 5k messages, resize)
  scripts/measure.py roundtrip [--bin PATH]            pop-out lifecycle checks in the real app

  --windows 1|3                main only, or main + editor and database pop-outs
  --opacity off|background|whole  alpha 1, or 0.6 in background or whole-window mode
  --warmup N                   cold modes: N unrecorded launches first (warm page cache)
  --trace S                    rss, Linux: also sample total PSS at 1 Hz for S seconds after ready
  --hz HZ                      frames: the display's refresh rate; each section is checked against it

Linux: the memory metric is PSS (smaps_rollup) summed over the app and all its descendants, GPU
memory (DRM fdinfo) is reported separately, and translucency is unsupported, so every window must
apply alpha 1 whatever opacity was requested (the opaque Linux mode). Method: D-037 and
docs/spikes/M0-linux-checks.md. Run under `flock /tmp/atrium-gui-measure.lock`.

Method: docs/spikes/M0-06b-ui-shell-layout-b.md ("Measurements"). Every failure is reported with
the app's stage and the tail of its stderr, and the run fails fast (review #2): a dead process,
an error report from the app, or a timeout each end the run with a reason. The last line of every
successful run is `SUMMARY {json}`, which scripts/summarize.py turns into the report's table.
"""
import argparse, ctypes, json, os, re, signal, statistics, subprocess, sys, tempfile, time

LINUX = sys.platform.startswith("linux")
if LINUX:
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
    import linuxproc

DEFAULT_BIN = ("src-tauri/target/release/atrium-ui-shell" if LINUX else
               "src-tauri/target/release/bundle/macos/Atrium UI Shell.app/Contents/MacOS/atrium-ui-shell")

# Every launch pins the state the app starts in (theme, opacity, windows); the app never reads
# or writes the user's settings file in a harness run. Printed with every result.
CONFIG = {"ATRIUM_THEME": "atrium-calm-dark", "ATRIUM_WINDOWS": "1", "ATRIUM_OPACITY": "off", "ATRIUM_SCREEN": "chat", "ATRIUM_RIGHT_TAB": "changes"}
ALPHA = 0.6
CONTENT_ALPHA = 0.74  # ALPHA + (1 - ALPHA) * 0.35 (READING_AREA_LIFT)


class HarnessError(Exception):
    pass


def print_config(binary):
    print("config: " + " ".join(f"{k}={v}" for k, v in CONFIG.items()) + f"  bin={binary}")
    if LINUX:
        print("host: " + linuxproc.host_line())
        return
    print(f"host: macOS {subprocess.check_output(['sw_vers', '-productVersion'], text=True).strip()}, "
          f"{subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip()}")


def tail(path, n=25):
    try:
        with open(path, errors="replace") as f:
            lines = f.readlines()
        return "".join(lines[-n:]).rstrip() or "(empty)"
    except OSError:
        return "(no log)"


def launch(binary, env_extra, log):
    """Direct exec: precise t0 and a process handle. The app activates itself in harness mode."""
    env = dict(os.environ, **CONFIG, **env_extra)
    out = open(log, "w")
    t0 = time.time() * 1000.0  # ms since epoch, right before exec
    p = subprocess.Popen([binary], env=env, stdout=out, stderr=subprocess.STDOUT)
    out.close()
    return p, t0


def wait_report(path, alive, log, timeout):
    """Polls for the report; fails fast on an error report or a dead process."""
    end = time.time() + timeout
    while time.time() < end:
        if os.path.exists(path):  # written atomically (temp file + rename)
            with open(path) as f:
                r = json.load(f)
            if r.get("kind") == "error":
                raise HarnessError(f"app failed at stage '{r.get('stage')}': {r.get('message')}\n"
                                   f"  windows: {json.dumps(r.get('windows'))}\n  app log tail:\n{tail(log)}")
            return r
        code = alive()
        if code is not None:
            raise HarnessError(f"app exited ({code}) before reporting\n  app log tail:\n{tail(log)}")
        time.sleep(0.02)
    raise HarnessError(f"no report within {timeout} s\n  app log tail:\n{tail(log)}")


def expected_windows():
    return ["main"] if CONFIG["ATRIUM_WINDOWS"] == "1" else ["main", "view-database", "view-editor"]


def validate_report(r, translucency=not LINUX):
    """Checks the real ready count and every window's applied appearance (review #3). Without
    translucency (Linux) the requested settings are kept but every window must apply alpha 1."""
    c = r["configuration"]
    per = c["perWindow"]
    want = expected_windows()
    if sorted(per) != want:
        raise HarnessError(f"ready windows {sorted(per)} != expected {want}")
    opacity = CONFIG["ATRIUM_OPACITY"]
    applied = opacity if translucency else "off"
    for label, w in per.items():
        s = w["settings"]
        checks = {
            "theme": (w["theme"], CONFIG["ATRIUM_THEME"]),
            "settings.alpha": (s["alpha"], 1 if opacity == "off" else ALPHA),
            "settings.mode": (s["mode"], "whole" if opacity == "whole" else "background"),
            "chromeAlpha": (w["chromeAlpha"], ALPHA if applied == "background" else 1),
            "contentAlpha": (w["contentAlpha"], CONTENT_ALPHA if applied == "background" else 1),
            "nativeAlpha": (w["nativeAlpha"], ALPHA if applied == "whole" else 1),
            "vibrancy": (w["vibrancy"], applied == "background"),
            "error": (w["error"], ""),
            "accessibilityOpaque": (w["accessibilityOpaque"], False),
        }
        bad = {k: v for k, v in checks.items() if v[0] != v[1]}
        if bad:
            raise HarnessError(f"window {label} applied the wrong appearance: " + ", ".join(f"{k}={a!r} (want {b!r})" for k, (a, b) in bad.items()))
    raf = {k: ("yes" if v.get("rafAt") else "NO") for k, v in per.items()}
    print("rAF fired after ready (main is frontmost; pop-outs may be occluded): " + json.dumps(raf, sort_keys=True))
    print("verified per-window appearance: " + json.dumps({k: {x: v[x] for x in ("chromeAlpha", "contentAlpha", "nativeAlpha", "vibrancy")} for k, v in per.items()}, sort_keys=True))


def stop(p):
    if p.poll() is None:
        p.send_signal(signal.SIGTERM)
        try:
            p.wait(timeout=5)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait(timeout=5)


def med(xs):
    return statistics.median(xs)


def summary(metric, **values):
    print("SUMMARY " + json.dumps({"metric": metric, "windows": int(CONFIG["ATRIUM_WINDOWS"]), "opacity": CONFIG["ATRIUM_OPACITY"], **values}))


def cold_once(binary):
    tmp = tempfile.mkdtemp(prefix="atrium-cold-")
    out, log = os.path.join(tmp, "report.json"), os.path.join(tmp, "app.log")
    p, t0 = launch(binary, {"ATRIUM_BENCH": "startup", "ATRIUM_BENCH_OUT": out}, log)
    try:
        r = wait_report(out, p.poll, log, timeout=40)
        validate_report(r)
    finally:
        stop(p)
    return timing(r, t0)


def timing(r, t0):
    """Contract §5: TTI is reached once the window has painted AND the first input is handled,
    so it is the later of the two. Input-handler readiness and first paint are reported apart."""
    m = r["marks"]
    if "firstPaint" not in m or "firstInputHandled" not in m:
        raise HarnessError(f"report lacks firstPaint or firstInputHandled: {sorted(m)}")
    row = {
        "exec_to_main": r["mainEpochMs"] - t0,
        "main_to_js": m["jsStart"] - r["mainEpochMs"],
        "js_to_mounted": m["mounted"] - m["jsStart"],
        "input_ready": m["firstInputHandled"] - t0,
        "first_paint": m["firstPaint"] - t0,
        "first_contentful_paint": m.get("firstContentfulPaint", float("nan")) - t0,
        "tti_palette": m["paletteReady"] - t0,
        "all_windows_ready": m["allWindowsReady"] - t0,
    }
    row["tti"] = max(row["input_ready"], row["first_paint"])
    if row["tti"] < row["first_paint"] or row["tti"] < row["input_ready"]:
        raise HarnessError(f"TTI {row['tti']:.1f} ms precedes paint or input: {row}")
    return row


def cold_summary(rows, load):
    print(f"\n[windows={CONFIG['ATRIUM_WINDOWS']} opacity={CONFIG['ATRIUM_OPACITY']}] ms from process launch")
    for k in rows[0]:
        xs = [r[k] for r in rows]
        print(f"{k:24s} first={xs[0]:7.1f}  median={med(xs):7.1f}  min={min(xs):7.1f}  max={max(xs):7.1f}")
    tti = [r["tti"] for r in rows]
    print(f"TTI vs the 400 ms budget: {sum(1 for x in tti if x > 400)}/{len(tti)} launches over 400 ms; median {med(tti):.0f} ms, worst {max(tti):.0f} ms")
    summary("cold", runs=len(rows), tti_median=med(tti), tti_worst=max(tti), over_budget=sum(1 for x in tti if x > 400),
            input_ready_median=med([r["input_ready"] for r in rows]), input_ready_worst=max(r["input_ready"] for r in rows),
            palette_median=med([r["tti_palette"] for r in rows]), first_paint_median=med([r["first_paint"] for r in rows]),
            main_to_js_median=med([r["main_to_js"] for r in rows]), js_to_mounted_median=med([r["js_to_mounted"] for r in rows]),
            all_windows_median=med([r["all_windows_ready"] for r in rows]), all_windows_worst=max(r["all_windows_ready"] for r in rows), load=load)


def warmup(a, run_once):
    """Unrecorded launches that bring the binary and WebKit into the page cache ("warm", D-037)."""
    for i in range(getattr(a, "warmup", 0)):
        r = run_once()
        print(f"warm-up {i+1} (not recorded): tti={r['tti']:6.1f}")
        time.sleep(0.5)


def cold(a):
    print_config(a.bin)
    warmup(a, lambda: cold_once(a.bin))
    load0 = os.getloadavg()[0]
    rows = []
    for i in range(a.runs):
        rows.append(cold_once(a.bin))
        print(f"run {i+1:2d}: " + "  ".join(f"{k}={v:7.1f}" for k, v in rows[-1].items()))
        time.sleep(0.5)
    cold_summary(rows, [round(load0, 1), round(os.getloadavg()[0], 1)])


COLD_CONFIGS = [(w, o) for w in ("1", "3") for o in ("off", "background", "whole")]


def coldmatrix(a, run_once=None, pause=0.3):
    """All six configurations interleaved launch by launch, so background load from other work on
    the machine spreads evenly over them instead of landing on whichever cell ran at the time.
    A failing configuration is recorded and skipped from then on; the others keep running and
    still get their summaries (review 2 #6). Raises at the end if any configuration failed."""
    run_once = run_once or (lambda: cold_once(a.bin))
    print_config(a.bin)
    warmup(a, run_once)
    rows = {c: [] for c in COLD_CONFIGS}
    failures = {}
    loads = []
    for i in range(a.runs):
        loads.append(round(os.getloadavg()[0], 1))
        for w, o in COLD_CONFIGS:
            if (w, o) in failures:
                continue
            CONFIG["ATRIUM_WINDOWS"], CONFIG["ATRIUM_OPACITY"] = w, o
            try:
                rows[(w, o)].append(run_once())
            except (HarnessError, KeyError, AssertionError, OSError) as e:
                failures[(w, o)] = f"run {i+1}: {type(e).__name__}: {e}"
                print(f"FAILED windows={w} opacity={o}: {failures[(w, o)]}")
                continue
            print(f"run {i+1:2d} windows={w} opacity={o:10s} tti={rows[(w, o)][-1]['tti']:6.1f}  all-ready={rows[(w, o)][-1]['all_windows_ready']:6.1f}")
            time.sleep(pause)
    print(f"1-minute load average at the start of each round: {loads}")
    for w, o in COLD_CONFIGS:
        CONFIG["ATRIUM_WINDOWS"], CONFIG["ATRIUM_OPACITY"] = w, o
        if (w, o) in failures:
            print(f"SUMMARY " + json.dumps({"metric": "cold", "windows": int(w), "opacity": o, "failed": failures[(w, o)]}))
        else:
            cold_summary(rows[(w, o)], [min(loads), max(loads)])
    if failures:
        raise HarnessError(f"{len(failures)} cold configuration(s) failed: " + "; ".join(f"{w}/{o}: {m}" for (w, o), m in failures.items()))


def default_runs(mode):
    """Both cold modes default to ten launches per configuration (review 2 #7); frames to three."""
    return 10 if mode in ("cold", "coldmatrix") else 3


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
    """The app plus every WebKit helper whose responsible process is the app."""
    rows, others = [], 0
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
    kinds = [os.path.basename(r[3]) for r in rows]
    missing = {"com.apple.WebKit.WebContent", "com.apple.WebKit.Networking"} - set(kinds)
    if bad or missing:
        raise HarnessError(f"incomplete snapshot: unreadable footprints for pids {bad}; missing helpers {sorted(missing)}")
    print(f"  TOTAL rss {total_rss:.1f} MB   footprint {sum(fps):.1f} MB   ({len(rows)} processes, {kinds.count('com.apple.WebKit.WebContent')} WebContent)")
    return total_rss, sum(fps)


def launch_via_open(binary, extra, log):
    """`open -n` makes the app its own responsible process, so WebKit's helpers are attributable to
    it exactly (a direct exec inherits the terminal as responsible). Returns the app's pid."""
    app = os.path.abspath(os.path.join(os.path.dirname(binary), "..", ".."))
    exe = os.path.abspath(binary)
    before = {r[0] for r in ps_table() if r[3] == exe}
    cmd = ["open", "-n", "--stdout", log, "--stderr", log]
    for k, v in dict(CONFIG, **extra).items():
        cmd += ["--env", f"{k}={v}"]
    subprocess.check_call(cmd + [app])
    for _ in range(100):
        time.sleep(0.1)
        new = [r[0] for r in ps_table() if r[3] == exe and r[0] not in before]
        if len(new) == 1:
            return new[0]
    raise HarnessError("could not identify the launched app process")


def pid_alive(pid):
    def check():
        try:
            os.kill(pid, 0)
            return None
        except ProcessLookupError:
            return "gone"
    return check


LINUX_HELPERS = {"WebKitWebProcess", "WebKitNetworkProcess"}


def linux_snapshot(app_pid, label, quiet=False):
    """PSS of the app and every descendant (WebKit's processes and any sandbox helpers), plus DRM GPU
    memory reported apart. Refuses an incomplete picture: an unreadable process or a missing helper."""
    rows = linuxproc.ps_rows()
    pids = [app_pid] + linuxproc.descendants(rows, app_pid)
    mems, bad = {}, []
    for pid in pids:
        m = linuxproc.mem_kb(pid)
        if m is None:
            bad.append(pid)
        mems[pid] = m
    names = {pid: linuxproc.proc_name(pid) for pid in pids}
    missing = LINUX_HELPERS - set(names.values())
    if bad or missing:
        raise HarnessError(f"incomplete snapshot: unreadable smaps_rollup for pids {bad}; missing helpers {sorted(missing)}")
    gpu = linuxproc.gpu_kb(pids)
    total = {k: sum(m[k] for m in mems.values()) / 1024 for k in ("pss", "rss", "uss")}
    if not quiet:
        print(f"{label}:")
        for pid in pids:
            m = mems[pid]
            print(f"  pid {pid:7d}  ppid {rows.get(pid, (0,))[0]:7d}  pss {m['pss']/1024:7.1f} MB  uss {m['uss']/1024:7.1f} MB  rss {m['rss']/1024:7.1f} MB  {names[pid]}")
        gtxt = "none (no DRM client)" if gpu is None else f"total {gpu['total']/1024:.1f} MB, resident {gpu['resident']/1024:.1f} MB"
        print(f"  TOTAL pss {total['pss']:.1f} MB   uss {total['uss']:.1f} MB   rss {total['rss']:.1f} MB   "
              f"({len(pids)} processes, {list(names.values()).count('WebKitWebProcess')} WebKitWebProcess)   GPU (DRM fdinfo, not in PSS): {gtxt}")
    return total, (None if gpu is None else gpu["total"] / 1024)


def rss_linux(a):
    print_config(a.bin)
    tmp = tempfile.mkdtemp(prefix="atrium-rss-")
    out, log = os.path.join(tmp, "report.json"), os.path.join(tmp, "app.log")
    p, _ = launch(a.bin, {"ATRIUM_BENCH": "startup", "ATRIUM_BENCH_OUT": out}, log)
    trace = []
    try:
        validate_report(wait_report(out, p.poll, log, timeout=40))
        t_ready = time.time()
        marks = {a.settle: None, a.settle + 20: None}
        end = max(a.settle + 20, a.trace)
        tick = 0
        while tick <= end:  # 1 Hz from ready; the two report snapshots land on their seconds
            time.sleep(max(0.0, t_ready + tick - time.time()))
            if tick in marks:
                marks[tick] = linux_snapshot(p.pid, f"idle {tick} s after ready")
                m, g = marks[tick]
            elif a.trace and tick <= a.trace:
                m, g = linux_snapshot(p.pid, "", quiet=True)
            else:
                tick += 1
                continue
            if a.trace and tick <= a.trace:
                trace.append({"t": round(time.time() - t_ready, 1), "pss": round(m["pss"], 1), "gpu": None if g is None else round(g, 1)})
            tick += 1
        (m1, g1), (m2, g2) = marks[a.settle], marks[a.settle + 20]
    finally:
        stop(p)
    extra = {}
    if trace:
        xs = [s["pss"] for s in trace]
        path = os.path.join(tmp, "pss-trace.json")
        with open(path, "w") as f:
            json.dump(trace, f)
        print(f"PSS trace, {len(trace)} samples over {trace[-1]['t']:.0f} s: min {min(xs):.1f}  median {med(xs):.1f}  max {max(xs):.1f} MB  ({path})")
        extra = {"trace_s": a.trace, "trace_pss_min": min(xs), "trace_pss_median": med(xs), "trace_pss_max": max(xs), "trace_file": path}
    summary("rss", metric_memory="pss", load=round(os.getloadavg()[0], 1), pss=[round(m1["pss"], 1), round(m2["pss"], 1)],
            uss=[round(m1["uss"], 1), round(m2["uss"], 1)], rss=[round(m1["rss"], 1), round(m2["rss"], 1)],
            gpu_drm=[g1 if g1 is None else round(g1, 1), g2 if g2 is None else round(g2, 1)], pss_max=max(m1["pss"], m2["pss"]), **extra)


def rss(a):
    if LINUX:
        return rss_linux(a)
    print_config(a.bin)
    resp = responsible_pid_fn()
    if resp is None:
        raise HarnessError("cannot determine helper ownership on this macOS (responsibility API missing); refusing to guess")
    tmp = tempfile.mkdtemp(prefix="atrium-rss-")
    out, log = os.path.join(tmp, "report.json"), os.path.join(tmp, "app.log")
    pid = launch_via_open(a.bin, {"ATRIUM_BENCH": "startup", "ATRIUM_BENCH_OUT": out}, log)
    try:
        if resp(pid) != pid:
            raise HarnessError(f"app pid {pid} is not its own responsible process (responsible={resp(pid)})")
        validate_report(wait_report(out, pid_alive(pid), log, timeout=40))
        time.sleep(a.settle)
        r1, f1 = snapshot(pid, resp, f"idle {a.settle} s after launch")
        time.sleep(20)
        r2, f2 = snapshot(pid, resp, f"idle {a.settle + 20} s after launch")
    finally:
        terminate_pid(pid)
    summary("rss", load=round(os.getloadavg()[0], 1), footprint=[round(f1, 1), round(f2, 1)], rss=[round(r1, 1), round(r2, 1)], footprint_max=max(f1, f2))


def frames(a):
    print_config(a.bin)
    runs = []
    for i in range(a.runs):
        tmp = tempfile.mkdtemp(prefix="atrium-frames-")
        out, log = os.path.join(tmp, "report.json"), os.path.join(tmp, "app.log")
        p, _ = launch(a.bin, {"ATRIUM_BENCH": "frames", "ATRIUM_BENCH_OUT": out}, log)
        try:
            r = wait_report(out, p.poll, log, timeout=90)
            validate_report(r)
        finally:
            stop(p)
        runs.append(r)
        print(f"--- run {i+1}  (dpr {r['dpr']}, viewport {r['viewport']})")
        if LINUX:  # which monitor each window landed on, and WebKitGTK's 60 fps pacing (src/linux.rs)
            with open(log, errors="replace") as f:
                for line in f:
                    if line.startswith("linux-display "):
                        print("   " + line.rstrip())
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
        time.sleep(0.5)
    def agg(k):
        res = {"fps_median": med([r[k]["fps"] for r in runs]), "fps_worst": min(r[k]["fps"] for r in runs),
               "p95_median": med([r[k]["p95Ms"] for r in runs]), "p99_worst": max(r[k]["p99Ms"] for r in runs),
               "max_worst": max(r[k]["maxMs"] for r in runs), "over25_total": sum(r[k]["over25ms"] for r in runs)}
        if a.hz:
            res.update(display_check(runs, k, a.hz))
        return res
    sections = ("idleFrames", "scrollReading", "scrollFling", "resize")
    aggs = {k: agg(k) for k in sections}
    if a.hz:
        for k in sections:
            c = aggs[k]
            print(f"{k:14s} vs {a.hz} Hz: {'PASS' if c['pass'] else 'FAIL'}  worst run {c['fps_worst']:.1f} fps (need >= {c['fps_needed']:.1f}); "
                  f"worst p99 {c['p99_worst']:.2f} ms ({'over' if c['p99_over_2_intervals'] else 'within'} 2 frame intervals = {2000 / a.hz:.1f} ms)")
    summary("frames", load=round(os.getloadavg()[0], 1), runs=len(runs), hz=a.hz, **aggs)


PASS_FRACTION = 0.97  # D-037: every run's fps at least 97% of the display's refresh rate


def display_check(runs, k, hz):
    """D-037's smoothness rule for one section: every run reaches 97% of the refresh rate; a p99 above
    two frame intervals is flagged but doesn't fail the check."""
    need = PASS_FRACTION * hz
    return {"fps_needed": need, "pass": all(r[k]["fps"] >= need for r in runs),
            "p99_over_2_intervals": max(r[k]["p99Ms"] for r in runs) > 2000 / hz}


def roundtrip(a):
    """Pop-out lifecycle in the real app: close-with-pending-edit, polite and forced bring-back."""
    print_config(a.bin)
    tmp = tempfile.mkdtemp(prefix="atrium-roundtrip-")
    out, log = os.path.join(tmp, "report.json"), os.path.join(tmp, "app.log")
    p, _ = launch(a.bin, {"ATRIUM_BENCH": "roundtrip", "ATRIUM_BENCH_OUT": out}, log)
    try:
        r = wait_report(out, p.poll, log, timeout=60)
    finally:
        stop(p)
    for c in r["checks"]:
        print(f"{'PASS' if c['ok'] else 'FAIL'}  {c['name']}  {c['detail']}")
    failed = [c["name"] for c in r["checks"] if not c["ok"]]
    summary("roundtrip", checks=len(r["checks"]), failed=failed)
    if failed:
        raise HarnessError(f"{len(failed)} pop-out check(s) failed: {failed}")


def terminate_pid(pid):
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    for _ in range(50):
        if not any(r[0] == pid for r in ps_table()):
            return
        time.sleep(0.1)
    os.kill(pid, signal.SIGKILL)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["cold", "coldmatrix", "rss", "frames", "roundtrip"])
    ap.add_argument("--runs", type=int, default=None)
    ap.add_argument("--settle", type=int, default=10)
    ap.add_argument("--bin", default=DEFAULT_BIN)
    ap.add_argument("--theme", default=None, help="starting theme (default atrium-calm-dark)")
    ap.add_argument("--windows", type=int, choices=[1, 3], default=1)
    ap.add_argument("--opacity", choices=["off", "background", "whole"], default="off")
    ap.add_argument("--warmup", type=int, default=0, help="cold modes: unrecorded launches first")
    ap.add_argument("--trace", type=int, default=0, help="rss (Linux): 1 Hz PSS trace for this many seconds after ready")
    ap.add_argument("--hz", type=float, default=None, help="frames: display refresh rate to check against")
    a = ap.parse_args()
    CONFIG["ATRIUM_WINDOWS"] = str(a.windows)
    CONFIG["ATRIUM_OPACITY"] = a.opacity
    if a.theme:
        CONFIG["ATRIUM_THEME"] = a.theme
    os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    if a.runs is None:
        a.runs = default_runs(a.mode)
    if a.runs < 1 or a.settle < 0:
        ap.error("runs must be positive; settle must be nonnegative")
    if not os.access(a.bin, os.X_OK):
        sys.exit(f"FAILED: no executable at {a.bin} (build it with `pnpm tauri build`)")
    try:
        {"cold": cold, "coldmatrix": coldmatrix, "rss": rss, "frames": frames, "roundtrip": roundtrip}[a.mode](a)
    except (HarnessError, KeyError, AssertionError) as e:
        print(f"FAILED ({a.mode}, windows={a.windows}, opacity={a.opacity}): {type(e).__name__}: {e}", file=sys.stderr)
        print(f"FAILED {type(e).__name__}: {e}")
        sys.exit(1)
