#!/usr/bin/env bash
# Stop the desktop app on the Windows machine.
source "$(dirname "$0")/lib.sh"

with_lock windows win "taskkill /IM urdevice.exe /F" || true
