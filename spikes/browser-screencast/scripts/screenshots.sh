#!/usr/bin/env bash
# Captures the app window for a few scripted scenes into docs/spikes/M0-08-browser-screencast/.
# Uses `screencapture -l <window id>`, so the invoking process needs Screen Recording permission.
#   scripts/screenshots.sh [path/to/binary] [out-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="${1:-src-tauri/target/release/atrium-browser-screencast}"
OUT="${2:-../../docs/spikes/M0-08-browser-screencast}"
DATA="$(mktemp -d)"
mkdir -p "$OUT"
swiftc -O scripts/window-id.swift -o "$DATA/window-id"

shot() { # name, scene, extra env...
  local name="$1" scene="$2"; shift 2
  env ATRIUM_BENCH="shot:$scene" ATRIUM_DATA_DIR="$DATA/$name" "$@" "$BIN" >/dev/null 2>&1 &
  local pid=$!
  local id=""
  for _ in $(seq 1 60); do
    sleep 0.25
    id=$("$DATA/window-id" "$pid" 2>/dev/null || true)
    [ -n "$id" ] && break
  done
  sleep 8
  screencapture -x -o -l"$id" "$OUT/$name.png"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  sleep 1
  echo "captured $name"
}

shot pane-agent-driving agent
shot pane-form-input-ime form

# Isolation: ONE launch (one browser process, one fixture port), three captures 10 s apart:
# A has written its marker, B on the same origin sees nothing, A still has its marker.
isolation() {
  env ATRIUM_BENCH="shot:isolation" ATRIUM_DATA_DIR="$DATA/isolation" "$BIN" >/dev/null 2>&1 &
  local pid=$!
  local id=""
  for _ in $(seq 1 60); do
    sleep 0.25
    id=$("$DATA/window-id" "$pid" 2>/dev/null || true)
    [ -n "$id" ] && break
  done
  sleep 8
  screencapture -x -o -l"$id" "$OUT/isolation-1-context-a-writes.png"
  sleep 10
  screencapture -x -o -l"$id" "$OUT/isolation-2-context-b-sees-nothing.png"
  sleep 10
  screencapture -x -o -l"$id" "$OUT/isolation-3-context-a-keeps-its-state.png"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  sleep 1
  echo "captured isolation (3 images)"
}
isolation
rm -rf "$DATA"
