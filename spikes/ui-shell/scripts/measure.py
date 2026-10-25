#!/usr/bin/env python3
"""Measurement harness for the M0-06 UI shell (macOS).

  scripts/measure.py cold   [--runs 10] [--bin PATH]   time to interactive + first paint
  scripts/measure.py rss    [--settle 10] [--bin PATH] idle memory: app + new WebKit helpers
  scripts/measure.py frames [--runs 3]  [--bin PATH]   rAF frame times (scroll, resize, theme swap)

Method notes are in docs/spikes/M0-06-ui-shell.md. The binary should be the release bundle:
  src-tauri/target/release/bundle/macos/Atrium UI Shell.app/Contents/MacOS/atrium-ui-shell
"""
import argparse, json, os, re, signal, statistics, subprocess, sys, tempfile, time

DEFAULT_BIN = "src-tauri/target/release/bundle/macos/Atrium UI Shell.app/Contents/MacOS/atrium-ui-shell"


def launch(binary, env_extra):
    env = dict(os.environ, **env_extra)
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


def stop(p):
    p.send_signal(signal.SIGTERM)
    try:
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        p.kill()


def med(xs):
    return statistics.median(xs)


def cold(a):
    rows = []
    for i in range(a.runs):
        out = tempfile.mktemp(suffix=".json")
        p, t0 = launch(a.bin, {"ATRIUM_BENCH": "startup", "ATRIUM_BENCH_OUT": out})
        r = wait_report(out)
        stop(p)
        os.unlink(out)
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


def ps_table():
    out = subprocess.check_output(["ps", "-axo", "pid=,ppid=,rss=,comm="], text=True)
    rows = []
    for line in out.splitlines():
        m = re.match(r"\s*(\d+)\s+(\d+)\s+(\d+)\s+(.*)$", line)
        if m:
            rows.append((int(m[1]), int(m[2]), int(m[3]), m[4]))
    return rows


def footprint_mb(pid):
    try:
        out = subprocess.check_output(["footprint", "-p", str(pid), "--noCategories"], text=True, stderr=subprocess.DEVNULL)
    except Exception:
        return None
    m = re.search(r"Footprint:\s+([\d.]+)\s*(KB|MB|GB)", out)
    if not m:
        return None
    v = float(m[1])
    return v / 1024 if m[2] == "KB" else v * 1024 if m[2] == "GB" else v


def rss(a):
    before = {r[0] for r in ps_table() if "WebKit" in r[3]}
    p, _ = launch(a.bin, {})
    time.sleep(a.settle)
    procs = [r for r in ps_table() if r[0] == p.pid or ("WebKit" in r[3] and r[0] not in before)]
    total_rss = sum(r[2] for r in procs) / 1024
    print(f"idle after {a.settle}s:")
    tot_fp = 0.0
    for pid, ppid, kb, comm in procs:
        fp = footprint_mb(pid)
        tot_fp += fp or 0
        print(f"  pid {pid:6d}  rss {kb/1024:7.1f} MB  footprint {fp if fp is None else round(fp,1)} MB  {os.path.basename(comm)}")
    print(f"  TOTAL rss {total_rss:.1f} MB   footprint {tot_fp:.1f} MB   ({len(procs)} processes)")
    time.sleep(20)
    procs = [r for r in ps_table() if r[0] == p.pid or ("WebKit" in r[3] and r[0] not in before)]
    print(f"  +20 s: TOTAL rss {sum(r[2] for r in procs)/1024:.1f} MB")
    stop(p)


def frames(a):
    for i in range(a.runs):
        out = tempfile.mktemp(suffix=".json")
        p, _ = launch(a.bin, {"ATRIUM_BENCH": "frames", "ATRIUM_BENCH_OUT": out})
        r = wait_report(out, timeout=90)
        stop(p)
        os.unlink(out)
        print(f"--- run {i+1}  (dpr {r['dpr']}, viewport {r['viewport']})")
        for k in ("idleFrames", "scrollReading", "scrollFling", "resize"):
            s = r[k]
            extra = f"  dist {s['distancePx']} px" if "distancePx" in s else ""
            w = s.get("handler") or s.get("step")
            if w:
                extra += f"  | main-thread/frame mean {w['meanMs']:.2f} p95 {w['p95Ms']:.2f} max {w['maxMs']:.2f} ms"
            print(f"{k:14s} {s['fps']:6.1f} fps  mean {s['meanMs']:5.2f}  p50 {s['p50Ms']:5.2f}  p95 {s['p95Ms']:5.2f}  p99 {s['p99Ms']:5.2f}  max {s['maxMs']:6.2f} ms  >16.7ms {s['over16_7ms']}/{s['frames']}  >25ms {s['over25ms']}{extra}")
        print("theme swap to next frame (ms):", [round(x, 1) for x in r["themeSwapToNextFrameMs"]])


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["cold", "rss", "frames"])
    ap.add_argument("--runs", type=int, default=None)
    ap.add_argument("--settle", type=int, default=10)
    ap.add_argument("--bin", default=DEFAULT_BIN)
    a = ap.parse_args()
    os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    if a.runs is None:
        a.runs = 10 if a.mode == "cold" else 3
    {"cold": cold, "rss": rss, "frames": frames}[a.mode](a)
