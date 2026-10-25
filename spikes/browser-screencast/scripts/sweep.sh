#!/usr/bin/env bash
# Backend-only sweep over screencast settings (release build of the `screencast-bench` example).
#   scripts/sweep.sh <out-dir>
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="${1:?out dir}"; mkdir -p "$OUT"
cargo build -q --release --manifest-path src-tauri/Cargo.toml --example screencast-bench
BENCH=src-tauri/target/release/examples/screencast-bench
run() { local name="$1"; shift; "$BENCH" --secs 8 --idle-secs 3 --probes 60 "$@" > "$OUT/$name.json" 2>/dev/null; python3 - "$name" "$OUT/$name.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[2])); s = d['streaming']; l = d['inputToFrameMs']['dist']
r = lambda v: None if v is None else round(v, 1)
print(sys.argv[1].ljust(14), 'fps', r(s['fps']), 'px', s['frameImagePixels'], 'KiB', r(s['frameKiB']['mean']), 'Mbit/s', r(s['mbitPerSecond']),
      'interval p95/max', r(s['frameIntervalMs']['p95']), r(s['frameIntervalMs']['max']),
      'latency p50/p95/max', r(l['p50']), r(l['p95']), r(l['max']), 'misses', d['inputToFrameMs']['misses'],
      'cpu%', round(s['cpuRss']['cpuPercent']), 'footprintMB', round(s['cpuRss']['footprintMbMedian'] or 0))
PY
}
for q in 50 70 80 90; do run "jpeg-q$q" --quality "$q"; done
run png --format png
run "jpeg-q80-2x" --quality 80 --dsf 2 --extra "--force-device-scale-factor=2"
run "jpeg-q60-2x" --quality 60 --dsf 2 --extra "--force-device-scale-factor=2"
run "jpeg-q80-every2" --quality 80 --every-nth 2
