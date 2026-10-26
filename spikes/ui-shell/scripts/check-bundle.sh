#!/usr/bin/env bash
# Verifies that a built app bundle carries the font licence notices (review 2 #9).
#   scripts/check-bundle.sh [path/to/Atrium UI Shell.app]
set -euo pipefail
cd -- "$(dirname -- "$0")/.."
APP="${1:-src-tauri/target/release/bundle/macos/Atrium UI Shell.app}"
status=0
for f in Instrument-Sans-OFL-1.1.txt Geist-Mono-OFL-1.1.txt; do
  p="$APP/Contents/Resources/licenses/$f"
  if [[ -f "$p" ]] && grep -q "SIL OPEN FONT LICENSE Version 1.1" "$p" && grep -q "Copyright" "$p"; then
    echo "ok: $p"
  else
    echo "MISSING or incomplete: $p" >&2
    status=1
  fi
done
exit $status
