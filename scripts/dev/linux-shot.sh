#!/usr/bin/env bash
# Screenshot this Linux desktop (works on Wayland through the portal).
#
#   scripts/dev/linux-shot.sh [out.png]     (default: linux-shot.png)
source "$(dirname "$0")/lib.sh"

exec python3 "$REPO_ROOT/scripts/dev/linux-shot.py" "${1:-linux-shot.png}"
