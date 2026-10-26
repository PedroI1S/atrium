#!/usr/bin/env bash
# Screenshots of layout B for docs/spikes/M0-06b-ui-shell-layout-b/. Each shot launches the
# built app with pinned settings (never the user's settings file) and writes a small JPEG.
# Opaque shots capture only the app's window (`screencapture -l`) and composite it over a
# synthetic wallpaper. Opacity shots ("screen" mode) first open a full-screen synthetic
# wallpaper window (scripts/backdrop.swift) and capture with ScreenCaptureKit through a filter
# that includes only the app's and the backdrop's windows (scripts/capture.swift), so real
# vibrancy and real NSWindow alpha show over a known backdrop, and nothing else on the screen
# can appear in the image.
#
#   scripts/screenshots.sh [path/to/binary] [out-dir] [shot-name...]
#
# Needs Screen Recording permission for the calling terminal. Run it under the shared GUI lock
# so it never overlaps another agent's measurement windows:
#   /usr/bin/lockf -k /tmp/atrium-gui-measure.lock scripts/screenshots.sh
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
BIN="${1:-src-tauri/target/release/bundle/macos/Atrium UI Shell.app/Contents/MacOS/atrium-ui-shell}"
OUT="${2:-../../docs/spikes/M0-06b-ui-shell-layout-b}"
shift $(( $# > 2 ? 2 : $# ))
ONLY=("$@")
mkdir -p -- "$OUT"
TMP="$(mktemp -d)"
trap 'rm -rf -- "$TMP"' EXIT
swiftc -O scripts/window-id.swift -o "$TMP/window-id"
swiftc -O scripts/composite.swift -o "$TMP/composite"
swiftc -O scripts/backdrop.swift -o "$TMP/backdrop"
swiftc -O scripts/capture.swift -o "$TMP/capture"

PIDS=()
cleanup_apps() { for p in ${PIDS[@]+"${PIDS[@]}"}; do kill "$p" 2>/dev/null || true; wait "$p" 2>/dev/null || true; done; PIDS=(); }
trap 'cleanup_apps; rm -rf -- "$TMP"' EXIT

MARGIN=32
# shot <name> <light|dark> <largest|smallest> <window|screen> -- ENV=value...
shot() {
  local name="$1" look="$2" which="$3" mode="$4"; shift 5
  if [[ ${#ONLY[@]} -gt 0 && ! " ${ONLY[*]} " =~ " $name " ]]; then return; fi
  local backdrop=""
  if [[ "$mode" == screen ]]; then
    "$TMP/backdrop" "$look" &
    backdrop=$!
    PIDS+=("$backdrop")
    sleep 0.8
  fi
  env ATRIUM_WINDOWS=1 "$@" "$BIN" >"$TMP/$name.log" 2>&1 &
  local pid=$!
  PIDS+=("$pid")
  local info=""
  for _ in $(seq 1 60); do
    sleep 0.25
    info=$("$TMP/window-id" "$pid" "$which" 2>/dev/null || true)
    [[ -n "$info" ]] && break
  done
  if [[ -z "$info" ]]; then echo "no window for $name (see $TMP/$name.log)" >&2; cleanup_apps; return 1; fi
  sleep 2.5 # fonts, first layout, native vibrancy
  info=$("$TMP/window-id" "$pid" "$which")
  if [[ "$mode" == screen ]]; then
    "$TMP/capture" "$pid" "$backdrop" "$which" "$TMP/$name.png" "$MARGIN"
    sips -s format jpeg -s formatOptions 72 -Z 1440 "$TMP/$name.png" --out "$OUT/$name.jpg" >/dev/null
  else
    screencapture -x -o -l"${info%% *}" "$TMP/$name.png"
    "$TMP/composite" "$TMP/$name.png" "$OUT/$name.jpg" "$look" 1440
  fi
  cleanup_apps
  echo "captured $name ($(du -k "$OUT/$name.jpg" | cut -f1) KB)"
}

shot chat-light light largest window -- ATRIUM_THEME=atrium-calm-light ATRIUM_OPACITY=off
shot chat-dark dark largest window -- ATRIUM_THEME=atrium-calm-dark ATRIUM_OPACITY=off
shot settings-advanced-light light largest window -- ATRIUM_THEME=atrium-calm-light ATRIUM_OPACITY=off ATRIUM_SCREEN=settings ATRIUM_ADVANCED=1
shot settings-advanced-dark dark largest window -- ATRIUM_THEME=atrium-calm-dark ATRIUM_OPACITY=off ATRIUM_SCREEN=settings ATRIUM_ADVANCED=1
shot popout-chat-dark dark smallest window -- ATRIUM_THEME=atrium-calm-dark ATRIUM_OPACITY=off ATRIUM_POPOUTS=chat
shot popout-placeholder-light light largest window -- ATRIUM_THEME=atrium-calm-light ATRIUM_OPACITY=off ATRIUM_POPOUTS=chat
shot inbox-light light largest window -- ATRIUM_THEME=atrium-calm-light ATRIUM_OPACITY=off ATRIUM_SCREEN=inbox
shot editor-dark dark largest window -- ATRIUM_THEME=atrium-calm-dark ATRIUM_OPACITY=off ATRIUM_SCREEN=editor
shot opacity60-background-light light largest screen -- ATRIUM_THEME=atrium-calm-light ATRIUM_OPACITY=background
shot opacity60-background-dark dark largest screen -- ATRIUM_THEME=atrium-calm-dark ATRIUM_OPACITY=background
shot opacity60-whole-light light largest screen -- ATRIUM_THEME=atrium-calm-light ATRIUM_OPACITY=whole
shot opacity60-whole-dark dark largest screen -- ATRIUM_THEME=atrium-calm-dark ATRIUM_OPACITY=whole
