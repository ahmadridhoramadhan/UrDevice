#!/usr/bin/env bash
# Deploy the current commit to the Windows machine, build it there, and start
# the desktop app on the user's desktop.
#
#   scripts/dev/win-run.sh [branch]     (default: the current branch)
#
# Pushes HEAD to the bare repo on Windows, checks the branch out, stops a
# running urdevice.exe, builds with the MSVC environment, and starts it.
source "$(dirname "$0")/lib.sh"

branch="${1:-$(git -C "$REPO_ROOT" rev-parse --abbrev-ref HEAD)}"
vcvars="${URDEVICE_WIN_VCVARS:-D:\\dev\\vsbuildtools\\VC\\Auxiliary\\Build\\vcvars64.bat}"
toolchain="${URDEVICE_WIN_TOOLCHAIN:-1.95.0}"

run() {
    echo "==> push $branch"
    GIT_SSH_COMMAND="ssh ${SSH_OPTS[*]}" git -C "$REPO_ROOT" push \
        --receive-pack="powershell -NoProfile -Command git-receive-pack" \
        "$WIN_HOST:$WIN_GIT" "+HEAD:refs/heads/$branch"
    echo "==> checkout and build"
    win "taskkill /IM urdevice.exe /F >nul 2>&1 & cd /d $WIN_REPO && git fetch -q origin && git checkout -q -f -B $branch origin/$branch && call \"$vcvars\" >nul && cargo +$toolchain build -p urdevice-desktop"
    echo "==> start"
    win_gui "$WIN_REPO\\target\\debug\\urdevice.exe"
}
with_lock windows run
