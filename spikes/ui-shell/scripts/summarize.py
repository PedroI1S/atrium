#!/usr/bin/env python3
"""Prints the measurement table (markdown) from a measure-layout-b.sh output directory.

  python3 scripts/summarize.py /tmp/atrium-layout-b-measurements

Reads the `SUMMARY {json}` line of each cell's log; a cell without one is shown as FAILED with
its first FAILED line. Budgets: TTI < 400 ms, footprint < 150 MB, 60 fps (PLAN §4).
"""
import glob, json, os, sys

out = sys.argv[1] if len(sys.argv) > 1 else "/tmp/atrium-layout-b-measurements"
cells = {}
for path in sorted(glob.glob(os.path.join(out, "*.log"))):
    parts = os.path.basename(path)[:-4].split("-")
    text = open(path, errors="replace").read()
    lines = [json.loads(l[len("SUMMARY "):]) for l in text.splitlines() if l.startswith("SUMMARY ")]
    for c in lines:  # a coldmatrix log holds one SUMMARY per configuration
        cells[(c["metric"], str(c["windows"]), c["opacity"])] = c
    if not lines and len(parts) == 3:
        metric, windows, opacity = parts
        failed = next((l for l in text.splitlines() if l.startswith("FAILED")), "FAILED (no summary)")
        cells[(metric, windows, opacity)] = {"failed": failed}


def cold(c):
    if not c:
        return "not run"
    if "failed" in c:
        return c["failed"][:80]
    mark = "✅" if c["tti_worst"] < 400 else ("⚠️" if c["tti_median"] < 400 else "❌")
    return f"{c['tti_median']:.0f} / {c['tti_worst']:.0f} ms {mark} ({c['over_budget']}/{c['runs']} over)"


def rss(c):
    if not c:
        return "not run"
    if "failed" in c:
        return c["failed"][:80]
    mark = "✅" if c["footprint_max"] < 150 else "❌"
    return f"{min(c['footprint']):.0f}–{max(c['footprint']):.0f} MB {mark} (RSS {min(c['rss']):.0f}–{max(c['rss']):.0f})"


def frames(c):
    if not c:
        return "not run"
    if "failed" in c:
        return c["failed"][:80]
    parts = []
    for k, label in (("scrollReading", "scroll"), ("scrollFling", "fling"), ("resize", "resize")):
        s = c[k]
        parts.append(f"{label} {s['fps_median']:.1f} (worst {s['fps_worst']:.1f}) fps, p95 {s['p95_median']:.1f}, max {s['max_worst']:.0f} ms")
    return "; ".join(parts)


print("| Config | TTI median / worst | Footprint (2 samples) | Frames (median fps, worst run fps, p95, worst max) |")
print("|---|---|---|---|")
for windows in ("1", "3"):
    for opacity in ("off", "background", "whole"):
        print(f"| {opacity}, {windows} window{'s' if windows == '3' else ''} | {cold(cells.get(('cold', windows, opacity)))} | "
              f"{rss(cells.get(('rss', windows, opacity)))} | {frames(cells.get(('frames', windows, opacity)))} |")
