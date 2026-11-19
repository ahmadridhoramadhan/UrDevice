#!/usr/bin/env bash
# Screenshot the USB phone.
#
#   scripts/dev/android-shot.sh [out.png]     (default: android-shot.png)
source "$(dirname "$0")/lib.sh"

out="${1:-android-shot.png}"
shot() { adb exec-out screencap -p >"$out"; }
with_lock android shot
echo "$out"
