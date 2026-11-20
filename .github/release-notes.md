## UrDevice test build (unsigned)

These packages are **unsigned test builds**. No signing certificates are used yet, so each OS warns on first run.

- **Windows** (`.msi`, `.zip`): SmartScreen shows "Windows protected your PC". Choose **More info**, then **Run anyway**. The MSI adds a Windows Firewall rule that allows `urdevice.exe` inbound (UDP) on **Private** networks, which pairing needs. With the `.zip`, allow the firewall prompt on a Private network.
- **macOS** (`.dmg`, Apple silicon and Intel): not notarized. Drag UrDevice to Applications, then **right-click the app and choose Open** the first time (or allow it in System Settings > Privacy & Security).
- **Linux** (`.deb`, `.AppImage`, `.tar.gz`): not signed. `.AppImage` needs `libfontconfig1` and `libdbus-1-3` on the host. Run `chmod +x` on it first.
- **Android** (`.apk`, arm64-v8a, armeabi-v7a, x86_64): signed with a throwaway **debug key** generated on the build runner, so it is a test build and not for Google Play. Allow **Install unknown apps** for your browser or file manager. A later build signed with a different key must be installed after uninstalling this one.

Setup, per-OS permissions and troubleshooting: see `docs/getting-started.md`, `docs/permissions.md` and `docs/troubleshooting.md` in the repository.
