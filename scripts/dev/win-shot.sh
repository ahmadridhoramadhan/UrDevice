#!/usr/bin/env bash
# Screenshot the Windows desktop and copy it here.
#
#   scripts/dev/win-shot.sh [out.png]     (default: win-shot.png)
source "$(dirname "$0")/lib.sh"

out="${1:-win-shot.png}"
run() {
    win_gui powershell.exe "-WindowStyle Hidden -NoProfile -ExecutionPolicy Bypass -File $WIN_REPO\\scripts\\dev\\win\\shot.ps1" >/dev/null
    scp "${SSH_OPTS[@]}" "$WIN_HOST:shot.png" "$out"
}
with_lock windows run
echo "$out"
