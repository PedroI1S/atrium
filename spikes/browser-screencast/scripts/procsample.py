#!/usr/bin/env python3
"""Linux: run the app and sample its process tree at 1 Hz (D-037; `procstat.rs` reads `ps`, whose
`time` column has 1 s resolution on Linux and gives no PSS).

  scripts/procsample.py OUT.json -- COMMAND...

Groups: the app itself; the browser (the descendant started with `--remote-debugging-pipe` and its
own descendants); WebKit (every other descendant: WebKitGTK's processes and sandbox helpers).
Per group and sample: CPU % from /proc/<pid>/stat deltas, PSS from smaps_rollup, GPU memory from DRM
fdinfo (apart from PSS). The summary covers the samples where the browser was running. Exits with
the command's exit code. `scripts/bench.sh` wraps each run with it on Linux.
"""
import json, os, statistics, subprocess, sys, time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
import linuxproc  # noqa: E402

GROUPS = ("app", "browser", "webkit")


def group_tree(rows, app):
    desc = linuxproc.descendants(rows, app)
    roots = [p for p in desc if "--remote-debugging-pipe" in rows[p][3]]
    browser = set(roots)
    for r in roots:
        browser |= set(linuxproc.descendants(rows, r))
    return {"app": [app], "browser": sorted(browser), "webkit": [p for p in desc if p not in browser]}


def sample(rows, groups, prev, dt):
    """prev: cumulative CPU seconds per pid from the previous sample; updated in place. Deltas are
    per process, so a helper exiting or starting doesn't skew the group's CPU."""
    s = {}
    for g, pids in groups.items():
        if dt:
            used = sum(rows[p][2] - prev[p] for p in pids if p in rows and p in prev)
            s[g + "_cpu_pct"] = max(0.0, used / dt * 100)
        mems = [linuxproc.mem_kb(p) for p in pids]
        s[g + "_pss_mb"] = sum(m["pss"] for m in mems if m) / 1024
        s[g + "_unreadable"] = sum(1 for m in mems if m is None)
        gpu = linuxproc.gpu_kb(pids)
        s[g + "_gpu_drm_mb"] = None if gpu is None else gpu["total"] / 1024
        s[g + "_procs"] = len(pids)
    prev.clear()
    prev.update({p: r[2] for p, r in rows.items()})
    return s


def stats(xs):
    xs = [x for x in xs if x is not None]
    return {"median": statistics.median(xs), "mean": statistics.mean(xs), "max": max(xs)} if xs else None


def main():
    if len(sys.argv) < 4 or sys.argv[2] != "--":
        sys.exit(__doc__)
    out, cmd = sys.argv[1], sys.argv[3:]
    host = linuxproc.host_info()
    p = subprocess.Popen(cmd)
    samples, prev, t0, last = [], {}, time.time(), None
    try:
        while p.poll() is None:
            now = time.time()
            rows = linuxproc.ps_rows()
            groups = group_tree(rows, p.pid)
            samples.append({"t": round(now - t0, 2), **sample(rows, groups, prev, (now - last) if last else None)})
            last = now
            time.sleep(max(0.0, 1.0 - (time.time() - now)))
    finally:
        if p.poll() is None:
            p.terminate()
            p.wait(timeout=10)
    live = [s for s in samples if s["browser_procs"] > 0]
    # PSS stats skip samples where a process of the group couldn't be read (a partial sum, e.g. while
    # the sandboxed browser is still starting); `unreadable_max` still reports that it happened.
    summary = {g: {"cpu_pct": stats([s.get(g + "_cpu_pct") for s in live]),
                   "pss_mb": stats([s[g + "_pss_mb"] for s in live if s[g + "_unreadable"] == 0]),
                   "gpu_drm_mb": stats([s[g + "_gpu_drm_mb"] for s in live]), "unreadable_max": max((s[g + "_unreadable"] for s in live), default=0)}
               for g in GROUPS}
    with open(out, "w") as f:
        json.dump({"host": host, "command": cmd, "exit": p.returncode, "browser_samples": len(live), "summary": summary, "samples": samples}, f, indent=1)
    sys.exit(p.returncode)


if __name__ == "__main__":
    main()
