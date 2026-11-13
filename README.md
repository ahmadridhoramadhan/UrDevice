# UrDevice

Integrate all your devices into one network.

UrDevice connects your Mac, Windows, Linux, and Android devices peer-to-peer over your local network. With it you can share clipboard, files, and photos, control one device from another, view screens, and open each other's localhost dev servers. Everything is end-to-end encrypted between paired devices, and nothing goes through a cloud server.

- **Device integration:** automatic discovery (mDNS + UDP broadcast), QR-code or code-confirmation pairing, and a Bluetooth fallback.
- **Resource sharing:** clipboard, files, and gallery photos, with progress shown.
- **Remote control:** Mousepad and Presenter modes from Android, plus full remote control.
- **Screen sharing:** watch any device's screen live.
- **Localhost sharing:** open a dev server from another device at `http://localhost:<port>`.
- **Keyboard and media control.**

Desktop apps use Rust and [Slint](https://slint.dev). The Android app uses native Kotlin. Both share one Rust core.

**Status:** specification. See [docs/](docs/README.md).

## License

MIT
