# Getting started

UrDevice pairs your own devices over your local network. Nothing goes through a server, and it does not work across the internet or between subnets.

## Install

Download the package for your OS from the project's [Releases](https://github.com/ahmadridhoramadhan/UrDevice/releases) page. Current builds are **unsigned test builds**, so each OS warns on first run.

| OS | Package | First run |
|----|---------|-----------|
| Windows 10/11 (x86_64) | `.msi` (recommended) or `.zip` | SmartScreen: **More info**, then **Run anyway**. The `.msi` also adds the firewall rule pairing needs (see [permissions](permissions.md#windows)). |
| macOS (Apple silicon or Intel) | `.dmg` | Drag UrDevice to Applications. Right-click the app, choose **Open**. |
| Linux (x86_64) | `.deb`, `.AppImage`, or `.tar.gz` | `sudo apt install ./urdevice-linux-amd64.deb`, or `chmod +x` the AppImage. Needs `libfontconfig1` and `libdbus-1-3`. |
| Android | `.apk` | Allow **Install unknown apps** for the app you opened it from. |

## Pair two devices

Both devices must be on the same Wi-Fi (or LAN) and on a network you marked as private or home.

1. Open UrDevice on both devices and finish the first-launch steps. Each step that needs an OS permission asks for it only then. See [permissions](permissions.md).
2. On one device choose **Pair**, then either:
   - **Scan QR**: scan the QR code shown on the other device. No code to confirm.
   - **Pick a device on the network**: select the other device in the list.
3. When you pick a device from the list, both screens show the same 6-digit code. Compare them by eye and press **Codes match** on **both** devices. Pairing finishes only after both presses.
4. The paired device appears in your device list. Each feature (clipboard, files, screen, input and so on) has its own permission setting per device. Anything sensitive asks before it is allowed.

If the other device does not appear, or pairing fails with "unreachable", see [troubleshooting](troubleshooting.md).
