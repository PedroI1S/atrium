#!/usr/bin/env python3
"""Benchmark driver for the M0-07 stream-perf spike (macOS).

  scripts/bench.py build                          build the daemon, the frontend and the release .app
  scripts/bench.py micro                          in-webview decode and markdown microbenchmarks
  scripts/bench.py single [--secs 60]             1 thread x 10k tokens/s
  scripts/bench.py multi  [--secs 60]             10 threads x 1k tokens/s, one visible, typing in the composer
  scripts/bench.py memory [--secs 300]            5 minutes at 10k tokens/s, memory sampled throughout
  scripts/bench.py sweep  [--secs 20]             encodings x batch intervals, 1 thread x 10k tokens/s
  scripts/bench.py all                            everything above except memory

Common: --encoding E --batch-ms N --chunk-chars N --label NAME --typing synthetic|none
Every run writes its full JSON report to measurements/<label>.json. Method: docs/spikes/M0-07-stream-perf.md.
"""
import argparse, ctypes, json, os, re, signal, statistics, subprocess, sys, tempfile, threading, time

HERE = os.path.dirname(os.path.abspath(__file__))
SPIKE = os.path.dirname(HERE)
ROOT = os.path.abspath(os.path.join(SPIKE, "..", ".."))
APP = os.path.join(SPIKE, "src-tauri/target/release/bundle/macos/Atrium Stream Perf.app")
BIN = os.path.join(APP, "Contents/MacOS/atrium-stream-perf")
DAEMON_BUILT = os.path.join(ROOT, "target/release/spike-daemon")
# The app spawns the daemon, and macOS blocks (TCC) a process started by an app from running a binary
# under ~/Documents until the user answers a prompt, so the daemon and its fake agent run from a copy.
DAEMON = "/tmp/atrium-m007-bin/spike-daemon"


def stage_daemon():
    os.makedirs(os.path.dirname(DAEMON), exist_ok=True)
    for name in ("spike-daemon", "spike-fake-agent"):
        subprocess.check_call(["cp", os.path.join(ROOT, "target/release", name), os.path.join(os.path.dirname(DAEMON), name)])
OUT = os.path.join(SPIKE, "measurements")


def sh(cmd, cwd=None, env=None):
    print("+", " ".join(cmd), flush=True)
    subprocess.check_call(cmd, cwd=cwd, env=env)


def build(_a=None):
    sh(["cargo", "build", "--release", "-p", "spike-daemon-socket", "--bins"], cwd=ROOT)
    sh(["pnpm", "install", "--frozen-lockfile"], cwd=SPIKE)
    sh(["pnpm", "tauri", "build", "--bundles", "app"], cwd=SPIKE)
    if not os.path.exists(BIN):
        sys.exit(f"expected {BIN}")


# ---------------------------------------------------------------- processes


def ps_table():
    out = subprocess.check_output(["ps", "-axo", "pid=,ppid=,rss=,time=,command="], text=True)
    rows = {}
    for line in out.splitlines():
        m = re.match(r"\s*(\d+)\s+(\d+)\s+(\d+)\s+([\d:.]+)\s+(.*)$", line)
        if m:
            rows[int(m[1])] = (int(m[2]), int(m[3]), cpu_seconds(m[4]), m[5])
    return rows


def cpu_seconds(t):
    parts = [float(x) for x in t.split(":")]
    s = 0.0
    for p in parts:
        s = s * 60 + p
    return s


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


def responsible_fn():
    try:
        f = ctypes.CDLL("/usr/lib/system/libquarantine.dylib").responsibility_get_pid_responsible_for_pid
    except (OSError, AttributeError):
        return None
    f.argtypes = [ctypes.c_int]
    f.restype = ctypes.c_int
    return f


def descendants(rows, root):
    out, todo = [], [root]
    while todo:
        p = todo.pop()
        for pid, (ppid, *_r) in rows.items():
            if ppid == p:
                out.append(pid)
                todo.append(pid)
    return out


class Procs:
    """The app, the WebKit helpers it is responsible for, and the daemon with its fake agents."""

    def __init__(self, app_pid, dirpath, resp):
        self.app, self.dir, self.resp = app_pid, dirpath, resp

    def find(self):
        rows = ps_table()
        groups = {"app": [self.app] if self.app in rows else [], "webcontent": [], "webkit_other": [], "daemon": [], "agents": []}
        for pid, (ppid, rss, cpu, cmd) in rows.items():
            if "com.apple.WebKit" in cmd and self.resp and self.resp(pid) == self.app:
                (groups["webcontent"] if "WebContent" in cmd else groups["webkit_other"]).append(pid)
            elif f"--dir {self.dir}" in cmd and "spike-daemon" in cmd:
                groups["daemon"].append(pid)
        for d in groups["daemon"]:
            groups["agents"] += descendants(rows, d)
        return rows, groups


class Sampler(threading.Thread):
    """1 Hz RSS and CPU (from cumulative CPU time deltas) per process group; footprint every `fp_every` s."""

    def __init__(self, procs, fp_every=0):
        super().__init__(daemon=True)
        self.procs, self.fp_every = procs, fp_every
        self.samples, self.fps, self.stop_ev = [], [], threading.Event()
        self.t0 = time.time()

    def run(self):
        prev, prev_t, n = {}, None, 0
        while not self.stop_ev.is_set():
            rows, groups = self.procs.find()
            now = time.time()
            s = {"t": now - self.t0}
            for g, pids in groups.items():
                s[g + "_rss_mb"] = sum(rows[p][1] for p in pids if p in rows) / 1024
                cpu = sum(rows[p][2] for p in pids if p in rows)
                if g in prev and prev_t:
                    s[g + "_cpu_pct"] = max(0.0, (cpu - prev[g]) / (now - prev_t) * 100)
                prev[g] = cpu
            prev_t = now
            if self.fp_every and n % self.fp_every == 0:
                self.fps.append({"t": now - self.t0, **footprint_groups(groups)})
            self.samples.append(s)
            n += 1
            self.stop_ev.wait(1.0)

    def stop(self):
        self.stop_ev.set()
        self.join(timeout=5)


def footprint_groups(groups):
    res = {}
    for g in ("app", "webcontent", "webkit_other", "daemon", "agents"):
        vals = [footprint_mb(p) for p in groups[g]]
        res[g + "_footprint_mb"] = None if any(v is None for v in vals) else sum(vals)
    parts = [v for v in res.values() if v is not None]
    res["total_footprint_mb"] = sum(parts) if len(parts) == 5 else None
    return res


def summarize_cpu(samples, key, lo, hi):
    xs = [s[key] for s in samples if lo <= s["t"] <= hi and key in s]
    return {"mean": statistics.mean(xs), "max": max(xs)} if xs else None


# ---------------------------------------------------------------- running a scenario


def kill_tree(pid):
    rows = ps_table()
    for p in descendants(rows, pid)[::-1] + [pid]:
        try:
            os.kill(p, signal.SIGKILL)
        except ProcessLookupError:
            pass


def cleanup_dir(dirpath):
    rows = ps_table()
    for pid, (_pp, _r, _c, cmd) in rows.items():
        if f"--dir {dirpath}" in cmd and "spike-daemon" in cmd:
            kill_tree(pid)
    subprocess.call(["rm", "-rf", dirpath])


def launch(scenario, out_path, dirpath):
    cmd = ["open", "-n", "--env", f"ATRIUM_BENCH={json.dumps(scenario)}", "--env", f"ATRIUM_BENCH_OUT={out_path}",
           "--env", f"ATRIUM_BENCH_DIR={dirpath}", "--env", f"ATRIUM_DAEMON_EXE={DAEMON}", APP]
    before = {p for p, (_a, _b, _c, cmd_) in ps_table().items() if cmd_.startswith(BIN)}
    subprocess.check_call(cmd)
    for _ in range(100):
        time.sleep(0.1)
        new = [p for p, (_a, _b, _c, cmd_) in ps_table().items() if cmd_.startswith(BIN) and p not in before]
        if len(new) == 1:
            return new[0]
    sys.exit("could not identify the launched app")


def host_info():
    def q(*a):
        return subprocess.check_output(a, text=True).strip()
    return {
        "macos": q("sw_vers", "-productVersion"),
        "cpu": q("sysctl", "-n", "machdep.cpu.brand_string"),
        "cores": int(q("sysctl", "-n", "hw.ncpu")),
        "loadavg_at_start": os.getloadavg(),
    }


def invalid_reason(r):
    """Why a run cannot be used, or None. The window must stay visible for the whole stream (rAF stops
    otherwise), frames must keep coming, and nothing may time out."""
    if "error" in r or "frames" not in r:
        return f"error: {r.get('error')}"
    if r.get("timedOut"):
        return "timed out"
    v = r.get("visibility", {})
    lo, hi = v.get("streamStartRelMs", 0), v.get("streamEndRelMs", 1e12)
    hidden = [c for c in v.get("changes", []) if c["state"] == "hidden" and lo - 100 <= c["t"] <= hi]
    if hidden:
        return f"window hidden during the stream at {hidden[0]['t'] - lo:.0f} ms (occluded by another window?)"
    expected = r["pipeline"]["streamSeconds"] * 1000 / r["refreshMs"]
    if r["frames"]["frames"] < 0.9 * expected:
        return f"only {r['frames']['frames']} frames, expected ~{expected:.0f}"
    if r["frames"]["max"] > 1000:
        return f"rAF stall of {r['frames']['max']} ms"
    return None


def run_scenario(sc, label, attempts=4, **kw):
    """Runs a scenario; a run in which rAF stalled for over a second (window hidden or occluded) or that
    timed out is an environment artifact: it is kept as <label>-invalid<N>.json and repeated."""
    existing = os.path.join(OUT, label + ".json")
    if os.path.exists(existing):  # resume: reuse a valid report of the same scenario
        try:
            r = json.load(open(existing))
            if not invalid_reason(r) and all(r["scenario"].get(k) == v for k, v in sc.items() if k != "hold"):
                print(f"== {label}: reusing {existing}", flush=True)
                return r
        except (ValueError, KeyError):
            pass
    for n in range(1, attempts + 1):
        try:
            r = _run_once(sc, label, **kw)
        except (TimeoutError, RuntimeError) as e:
            r = {"error": str(e)}
            open(os.path.join(OUT, label + ".json"), "w").write(json.dumps(r))
        stalled = invalid_reason(r)
        if not stalled:
            r["attempt"] = n
            json.dump(r, open(os.path.join(OUT, label + ".json"), "w"), indent=1)
            return r
        print(f"   INVALID run ({stalled}); kept as {label}-invalid{n}.json", flush=True)
        os.replace(os.path.join(OUT, label + ".json"), os.path.join(OUT, f"{label}-invalid{n}.json"))
    sys.exit(f"{label}: {attempts} invalid runs in a row")


def _run_once(sc, label, hold_s=0, fp_every=0, timeout=None, quiet=None):
    os.makedirs(OUT, exist_ok=True)
    out_path = os.path.join(OUT, label + ".json")
    if os.path.exists(out_path):
        os.unlink(out_path)
    if quiet:
        wait_quiet(quiet)
    dirpath = tempfile.mkdtemp(prefix="asp-")
    os.rmdir(dirpath)  # the client creates it with mode 0700
    resp = responsible_fn()
    sc = dict(sc, hold=hold_s > 0)
    host = host_info()
    print(f"== {label}: {json.dumps(sc)}\n   host: {host}", flush=True)
    pid = launch(sc, out_path, dirpath)
    # Another window on top hides this one and WebKit stops running rAF; ask for the foreground.
    subprocess.call(["osascript", "-e", 'tell application id "dev.atrium.stream-perf" to activate'], stderr=subprocess.DEVNULL)
    if resp and resp(pid) != pid:
        kill_tree(pid)
        sys.exit(f"app pid {pid} is not its own responsible process; memory attribution would be ambiguous")
    procs = Procs(pid, dirpath, resp)
    sampler = Sampler(procs, fp_every)
    sampler.start()
    t_start = time.time()
    timeout = timeout or (sc.get("durationMs", 0) / 1000 + 150)
    try:
        while not (os.path.exists(out_path) and os.path.getsize(out_path) > 0):
            if time.time() - t_start > timeout:
                raise TimeoutError(f"no report after {timeout:.0f}s")
            if pid not in ps_table():
                raise RuntimeError("app exited without a report")
            time.sleep(0.25)
        time.sleep(0.2)
        report = json.load(open(out_path))
        t_report = time.time() - sampler.t0
        post = []
        if hold_s > 0:
            for wait in (0, 15, 60):
                time.sleep(max(0, wait - (post[-1]["after_s"] if post else 0)))
                rows, groups = procs.find()
                post.append({"after_s": wait, **footprint_groups(groups),
                             "rss_mb": {g: sum(rows[p][1] for p in groups[g] if p in rows) / 1024 for g in groups}})
                if wait >= hold_s:
                    break
    finally:
        sampler.stop()
        kill_tree(pid)
        cleanup_dir(dirpath)
    if "error" in report and "scenario" not in report:
        return report
    if "error" in report:
        print("REPORT ERROR:", report["error"])
    tl = report.get("timeline", {})
    run_lo = (tl.get("streamStartEpochMs", 0) / 1000) - sampler.t0
    run_hi = (tl.get("streamEndEpochMs", 0) / 1000) - sampler.t0
    report["host"] = {**report.get("host", {}), **host, "loadavg_at_end": os.getloadavg()}
    report["resources"] = {
        "cpu_pct_during_stream": {g: summarize_cpu(sampler.samples, g + "_cpu_pct", run_lo, run_hi) for g in ("app", "webcontent", "webkit_other", "daemon", "agents")},
        "peak_rss_mb": {g: max((s.get(g + "_rss_mb", 0) for s in sampler.samples), default=0) for g in ("app", "webcontent", "webkit_other", "daemon", "agents")},
        "footprint_series": sampler.fps,
        "after_report": post,
        "rss_series_1hz": [{k: round(v, 1) for k, v in s.items() if k.endswith("rss_mb") or k == "t"} for s in sampler.samples[:: max(1, len(sampler.samples) // 400)]],
        "report_at_s": t_report,
    }
    json.dump(report, open(out_path, "w"), indent=1)
    return report


def wait_quiet(threshold, max_wait=900):
    t0 = time.time()
    while os.getloadavg()[0] > threshold and time.time() - t0 < max_wait:
        print(f"   waiting for load < {threshold} (now {os.getloadavg()[0]:.1f})", flush=True)
        time.sleep(15)


# ---------------------------------------------------------------- scenarios


def base(a, **kw):
    sc = {"encoding": a.encoding, "batchMs": a.batch_ms, "chunkChars": a.chunk_chars, "ackIntervalMs": a.ack_ms,
          "toolCallEveryMs": 4000, "typing": "none", "typingIntervalMs": 120, "typingStartMs": 3000, "idleTypingMs": 0,
          "baselineMs": 1500}
    sc.update(kw)
    return sc


def single_sc(a):
    return base(a, name="single", threads=1, tokensPerSecond=10000, durationMs=a.secs * 1000)


def multi_sc(a):
    return base(a, name="multi", threads=10, tokensPerSecond=1000, durationMs=a.secs * 1000,
                typing=a.typing, idleTypingMs=4000 if a.typing != "none" else 0)


def fmt(r):
    f, l, t = r["frames"], r["latency"], r["typing"]
    lines = [
        f"frames: n={f['frames']} fps={f['fps']:.1f} p50={f['p50']} p95={f['p95']} p99={f['p99']} max={f['max']} ms; slow(>1.5x refresh {r['refreshMs']})={f['slowFrames']} dropped={f['droppedFrames']} >25ms={f['over25ms']}",
        f"e2e latency (agent ts -> committed frame, visible deltas): n={l['n']} p50={l['p50']} p95={l['p95']} p99={l['p99']} max={l['max']} ms",
        f"  segments (per event): agent->bridge p50={r['bridge']['agentToBridgeMs']['p50']} p99={r['bridge']['agentToBridgeMs']['p99']}  batching p50={r['bridge']['batchingMs']['p50']} p99={r['bridge']['batchingMs']['p99']}  ipc p50={r['ipc']['p50']} p99={r['ipc']['p99']}  js-queue p50={r['queueWait']['p50']} p99={r['queueWait']['p99']} ms",
        f"  per-frame: apply(JS+Solid) p50={r['applyMs']['p50']} p99={r['applyMs']['p99']} max={r['applyMs']['max']}  rAF->next task p50={r['renderWork']['p50']} p99={r['renderWork']['p99']} max={r['renderWork']['max']} ms",
        f"events: bridge={r['bridge']['events']} received={r['pipeline']['eventsReceived']} applied={r['pipeline']['eventsApplied']}  tokens~{r['bridge']['tokensApprox']} ({r['bridge']['tokensPerSecondApprox']:.0f}/s)  batches={r['pipeline']['batchesReceived']} bytes={r['bridge']['bytes']} backpressure waits={r['bridge']['backpressureWaits']}",
    ]
    if t["streaming"]["inputs"]:
        s, i = t["streaming"], t["idle"]
        lines.append(f"typing (streaming): inputs={s['inputs']} typed={t['typed']} textarea={t['textareaLength']} handler p50={s['handlerMs']['p50']} p99={s['handlerMs']['p99']} frame p50={s['frameMs']['p50']} p99={s['frameMs']['p99']} paint p50={s['paintMs']['p50']} p95={s['paintMs']['p95']} p99={s['paintMs']['p99']} max={s['paintMs']['max']} ms")
        if i:
            lines.append(f"typing (idle ref):  inputs={i['inputs']} paint p50={i['paintMs']['p50']} p95={i['paintMs']['p95']} p99={i['paintMs']['p99']} max={i['paintMs']['max']} ms")
    cpu = r["resources"]["cpu_pct_during_stream"]
    lines.append("cpu% during stream (mean): " + " ".join(f"{g}={v['mean']:.0f}" for g, v in cpu.items() if v))
    return "\n".join(lines)


def cmd_micro(a):
    r = run_scenario({"mode": "micro"}, a.label or "micro", timeout=180)
    print(json.dumps(r["decode"][:3], indent=1), "...")
    return r


def cmd_single(a):
    r = run_scenario(single_sc(a), a.label or f"single-{a.encoding}-b{a.batch_ms}", quiet=a.quiet)
    print(fmt(r))
    return r


def cmd_multi(a):
    r = run_scenario(multi_sc(a), a.label or f"multi-{a.encoding}-b{a.batch_ms}", quiet=a.quiet)
    print(fmt(r))
    return r


def cmd_memory(a):
    sc = base(a, name="memory", threads=1, tokensPerSecond=a.tps, durationMs=a.secs * 1000)
    r = run_scenario(sc, a.label or "memory", hold_s=60, fp_every=30, timeout=a.secs + 400, quiet=a.quiet)
    print(fmt(r))
    print("transcript:", r["transcript"])
    for p in r["resources"]["after_report"]:
        print("after", p["after_s"], "s:", {k: (round(v, 1) if isinstance(v, float) else v) for k, v in p.items() if k.endswith("footprint_mb")})
    for p in r["resources"]["footprint_series"]:
        print("during t=%.0f:" % p["t"], {k: (round(v, 1) if isinstance(v, float) else v) for k, v in p.items() if k.endswith("footprint_mb")})
    return r


def cpu_mean(r, g):
    v = r["resources"]["cpu_pct_during_stream"][g]
    return v["mean"] if v else 0.0


def cmd_sweep(a):
    """encodings x batch intervals, `--reps` repetitions each; the table gives the median over repetitions
    and, for the tail columns, the worst repetition."""
    encs = a.encodings.split(",")
    batches = [int(x) for x in a.batches.split(",")]
    results = {}
    for rep in range(1, a.reps + 1):
        for enc in encs:
            for b in batches:
                a.encoding, a.batch_ms = enc, b
                r = run_scenario(single_sc(a), f"sweep-{enc}-b{b}-r{rep}", quiet=a.quiet)
                results.setdefault((enc, b), []).append(r)
    med = statistics.median
    print(f"\nmedian over {a.reps} valid runs of {a.secs} s; (worst) = worst run")
    print("| encoding | batch ms | fps | frame p99 (worst) | frame max (worst) | dropped (worst) | e2e p50 | e2e p95 | e2e p99 (worst) | e2e max (worst) | MB over IPC | Rust encode ms | JS decode ms | app cpu% | webcontent cpu% |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    for (enc, b), rs in results.items():
        col = lambda f: [f(r) for r in rs]
        print(f"| {enc} | {b} | {med(col(lambda r: r['frames']['fps'])):.1f} | {med(col(lambda r: r['frames']['p99']))} ({max(col(lambda r: r['frames']['p99']))}) | {med(col(lambda r: r['frames']['max']))} ({max(col(lambda r: r['frames']['max']))}) | {med(col(lambda r: r['frames']['droppedFrames']))} ({max(col(lambda r: r['frames']['droppedFrames']))}) | {med(col(lambda r: r['latency']['p50']))} | {med(col(lambda r: r['latency']['p95']))} | {med(col(lambda r: r['latency']['p99']))} ({max(col(lambda r: r['latency']['p99']))}) | {med(col(lambda r: r['latency']['max']))} ({max(col(lambda r: r['latency']['max']))}) | {med(col(lambda r: r['bridge']['bytes']))/1e6:.1f} | {med(col(lambda r: r['bridge']['encodeUs']['totalMs'])):.0f} | {med(col(lambda r: r['decode']['totalMs'])):.0f} | {med(col(lambda r: cpu_mean(r, 'app'))):.0f} | {med(col(lambda r: cpu_mean(r, 'webcontent'))):.0f} |")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("mode", choices=["build", "micro", "single", "multi", "memory", "sweep", "all"])
    ap.add_argument("--secs", type=int, default=None)
    ap.add_argument("--encoding", default="bin")
    ap.add_argument("--batch-ms", type=int, default=8)
    ap.add_argument("--chunk-chars", type=int, default=4)
    ap.add_argument("--ack-ms", type=int, default=30)
    ap.add_argument("--tps", type=int, default=10000, help="memory: tokens/s")
    ap.add_argument("--typing", default="synthetic", choices=["synthetic", "none"])
    ap.add_argument("--label", default=None)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--quiet", type=float, default=None, help="wait until the 1-minute load average is below this")
    ap.add_argument("--encodings", default="json,json-slim,msgpack,msgpack-slim,bin")
    ap.add_argument("--batches", default="0,4,8,16,33")
    a = ap.parse_args()
    if a.mode == "build":
        return build()
    if a.secs is None:
        a.secs = {"sweep": 20, "memory": 300}.get(a.mode, 60)
    if not os.path.exists(BIN) or not os.path.exists(DAEMON_BUILT):
        sys.exit("run `pnpm bench:build` first")
    stage_daemon()
    {"micro": cmd_micro, "single": cmd_single, "multi": cmd_multi, "memory": cmd_memory, "sweep": cmd_sweep}.get(a.mode, lambda x: None)(a)
    if a.mode == "all":
        a.secs = 60
        for f in (cmd_micro, cmd_sweep, cmd_single, cmd_multi):
            if f is cmd_sweep:
                a.secs = 20
            else:
                a.secs = 60
            f(a)


if __name__ == "__main__":
    main()
