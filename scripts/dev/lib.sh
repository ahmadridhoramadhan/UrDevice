# Shared settings for the scripts in scripts/dev. Source it; do not run it.
#
# Environment:
#   URDEVICE_WIN_HOST   Windows test machine (default ADVAN@192.168.18.9)
#   URDEVICE_WIN_REPO   checkout on Windows (default D:\dev\UrDevice)
#   URDEVICE_WIN_GIT    bare repo on Windows that the checkout fetches from
#                       (default D:/dev/UrDevice.git)
#   URDEVICE_LOCK_DIR   if set, each device is used under flock on
#                       $URDEVICE_LOCK_DIR/<device>.lock (android, windows, gradle)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WIN_HOST="${URDEVICE_WIN_HOST:-ADVAN@192.168.18.9}"
WIN_REPO="${URDEVICE_WIN_REPO:-D:\\dev\\UrDevice}"
WIN_GIT="${URDEVICE_WIN_GIT:-D:/dev/UrDevice.git}"
# `-F /dev/null`: ignore the local ssh config, which can break batch logins.
SSH_OPTS=(-F /dev/null -o BatchMode=yes -o ConnectTimeout=10)

# Run a command under the device lock when URDEVICE_LOCK_DIR is set.
with_lock() {
    local name="$1"
    shift
    if [[ -n "${URDEVICE_LOCK_DIR:-}" ]]; then
        flock "$URDEVICE_LOCK_DIR/$name.lock" "$@"
    else
        "$@"
    fi
}

# Run a cmd.exe command line on the Windows machine.
win() {
    ssh "${SSH_OPTS[@]}" "$WIN_HOST" "$@"
}

# Start a program on the Windows user's desktop. SSH runs in session 0, which
# has no desktop, so this goes through a one-off interactive scheduled task.
win_gui() {
    local exe="$1" args="${2:-}"
    win "powershell -NoProfile -ExecutionPolicy Bypass -File $WIN_REPO\\scripts\\dev\\win\\runi.ps1 -Exe \"$exe\" -ArgLine \"$args\""
}
