#!/usr/bin/env python3
"""Prints the headline numbers of every run-*.json in a directory, plus median and worst case."""
import glob, json, statistics, sys

d = sys.argv[1]
rows = []
for p in sorted(glob.glob(f"{d}/run-*.json")):
    r = json.load(open(p))
    if not r.get("ok"):
        print(p, "FAILED", r.get("error"))
        continue
    s, l, i = r["streaming"], r["inputToFrameMs"]["dist"], r["idleNoScreencast"]["chromium"]
    sc = s["cpuRss"]["chromium"]
    rows.append({
        "fps": s["fps"], "int_p95": s["drawIntervalMs"]["p95"], "int_max": s["drawIntervalMs"]["max"],
        "over33": s["intervalsOver33ms"], "kib": s["frameKiB"]["mean"], "mbit": s["mbitPerSecond"],
        "lat_p50": l["p50"], "lat_p95": l["p95"], "lat_max": l["max"], "misses": r["inputToFrameMs"]["misses"],
        "idle_cpu": i["cpuPercent"], "idle_fp": i["footprintMbMedian"], "idle_rss": i["rssMbMedian"],
        "str_cpu": sc["cpuPercent"], "str_fp": sc["footprintMbMedian"], "str_rss": sc["rssMbMedian"],
        "app_cpu": s["cpuRss"]["tauriAppProcessOnly"]["cpuPercent"],
        "switch": r["contextSwitch"]["toFirstFrameMs"],
    })
    print(p.split("/")[-1], {k: round(v, 1) if isinstance(v, float) else v for k, v in rows[-1].items()})
if rows:
    print("median:", {k: round(statistics.median(r[k] for r in rows), 1) for k in rows[0]})
    print("worst: ", {k: round(max(r[k] for r in rows), 1) for k in rows[0]})
