#!/usr/bin/env bash
# Captures the window of the built app for every direction (dark + light) plus palette, split
# panes and presets. Uses `screencapture -l <window id>`, so the window may be behind others, but
# the invoking process needs Screen Recording permission.
#   scripts/screenshots.sh [path/to/binary] [out-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="${1:-src-tauri/target/debug/bundle/macos/Atrium UI Shell.app/Contents/MacOS/atrium-ui-shell}"
OUT="${2:-../../docs/spikes/M0-06-ui-shell}"
mkdir -p "$OUT"
swiftc -O scripts/window-id.swift -o /tmp/atrium-window-id

shot() { # name, env assignments...
  local name="$1"; shift
  env "$@" "$BIN" >/dev/null 2>&1 &
  local pid=$!
  local id=""
  for _ in $(seq 1 40); do
    sleep 0.25
    id=$(/tmp/atrium-window-id "$pid" 2>/dev/null || true)
    [ -n "$id" ] && break
  done
  sleep 2.2
  screencapture -x -o -l"$id" "$OUT/$name.png"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  echo "captured $name"
}

for t in glass-dark glass-light flat-dark flat-light paper-dark paper-light imported-catppuccin-mocha; do
  shot "direction-$t" ATRIUM_THEME="$t"
done
shot palette-glass-dark ATRIUM_THEME=glass-dark ATRIUM_PALETTE=1
shot palette-flat-light ATRIUM_THEME=flat-light ATRIUM_PALETTE=1
shot palette-paper-dark ATRIUM_THEME=paper-dark ATRIUM_PALETTE=1
shot split-two-threads-glass-dark ATRIUM_THEME=glass-dark ATRIUM_TAB=split
shot split-two-threads-flat-dark ATRIUM_THEME=flat-dark ATRIUM_TAB=split
shot preset-review-glass-dark ATRIUM_THEME=glass-dark ATRIUM_PRESET=review
shot preset-focus-paper-light ATRIUM_THEME=paper-light ATRIUM_PRESET=focus
shot right-diff-flat-dark ATRIUM_THEME=flat-dark ATRIUM_RIGHT_TAB=diff
shot home-paper-dark ATRIUM_THEME=paper-dark ATRIUM_TAB=home
