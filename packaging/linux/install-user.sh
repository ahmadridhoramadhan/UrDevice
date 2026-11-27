#!/bin/sh
# Installs the UrDevice launcher entry and icon for the current user, so the desktop shows the
# app icon in the dock and the overview (on Wayland the window icon is not used; the desktop
# looks the icon up through urdevice.desktop).
#
# Usage: packaging/linux/install-user.sh [path-to-urdevice-binary]
# Default binary: target/debug/urdevice in this checkout.
# Undo: rm ~/.local/share/applications/urdevice.desktop ~/.local/share/icons/hicolor/*/apps/urdevice.png
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
bin=${1:-$root/target/debug/urdevice}
apps=${XDG_DATA_HOME:-$HOME/.local/share}/applications
icons=${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor

mkdir -p "$apps" "$icons/32x32/apps" "$icons/512x512/apps"
cp "$root/assets/urdevice-icon-32.png" "$icons/32x32/apps/urdevice.png"
cp "$root/assets/urdevice-icon-512.png" "$icons/512x512/apps/urdevice.png"
sed "s|^Exec=.*|Exec=$bin|" "$root/packaging/linux/urdevice.desktop" > "$apps/urdevice.desktop"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f "$icons" || true
echo "Installed: $apps/urdevice.desktop"
