#!/usr/bin/env bash
# Build and run the desktop app on this Linux machine.
#
#   scripts/dev/linux-run.sh [instance]
#
# With an instance name, the app gets its own data directory and QUIC port,
# so two instances can run side by side (docs/development.md).
source "$(dirname "$0")/lib.sh"

if [[ -n "${1:-}" ]]; then
    export URDEVICE_DATA_DIR="${URDEVICE_DATA_DIR:-/tmp/urdevice-$1}"
    sum=$(printf '%s' "$1" | cksum | cut -d" " -f1)
    export URDEVICE_PORT="${URDEVICE_PORT:-$((47900 + sum % 100))}"
fi
cd "$REPO_ROOT"
exec cargo run -p urdevice-desktop
