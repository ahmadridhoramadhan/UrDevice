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

**Status:** project setup. See [docs/](docs/README.md) for the spec and [docs/development.md](docs/development.md) to build it. Planned work is tracked in [GitHub issues](https://github.com/ahmadridhoramadhan/UrDevice/issues), grouped by the milestones Release 1, Release 2, and Kiroween submission.

## Features by device

`A => B` means you use device A to work with device B. Mobile means Android; iOS is not supported. **R1** is Release 1 and **R2** is Release 2, which comes after Release 1 is stable. Every feature follows the permission settings on the device it acts on: Allow, Always Ask, or Deny.

**All devices (R1)**

- Find each other on the same network, with no IP address to type ([#2](https://github.com/ahmadridhoramadhan/UrDevice/issues/2))
- Pair by scanning a QR code, or by confirming the same 6-digit code on both screens ([#2](https://github.com/ahmadridhoramadhan/UrDevice/issues/2))
- Encrypted connection that reconnects on its own; disconnect without unpairing; see the connection path, latency, and battery ([#2](https://github.com/ahmadridhoramadhan/UrDevice/issues/2))
- Set what other devices may do on this device, with a Stop button while it is being controlled or viewed ([#2](https://github.com/ahmadridhoramadhan/UrDevice/issues/2))
- Automations: rules that turn features on, start port forwards, lock a desktop, and more (R2, [#39](https://github.com/ahmadridhoramadhan/UrDevice/issues/39))

**desktop => desktop**

- Sync clipboard (R1, [#16](https://github.com/ahmadridhoramadhan/UrDevice/issues/16))
- Send files and folders (R1, [#17](https://github.com/ahmadridhoramadhan/UrDevice/issues/17))
- Remote control with mouse and keyboard from a remote-control window (R1, [#19](https://github.com/ahmadridhoramadhan/UrDevice/issues/19))
- View the other desktop's screen and control it from the view (R1, [#21](https://github.com/ahmadridhoramadhan/UrDevice/issues/21))
- Control the other desktop's media: source, play/pause, seek, volume, mute (R1, [#20](https://github.com/ahmadridhoramadhan/UrDevice/issues/20))
- Open the other desktop's localhost dev servers, including a browser's remote-debugging port (R1, [#22](https://github.com/ahmadridhoramadhan/UrDevice/issues/22))
- One mouse and keyboard across desktops by crossing a screen edge (R2, [#30](https://github.com/ahmadridhoramadhan/UrDevice/issues/30))
- Remote terminal, with no SSH setup (R2, [#32](https://github.com/ahmadridhoramadhan/UrDevice/issues/32))
- Port forwarding profiles and an HTTP request inspector (R2, [#33](https://github.com/ahmadridhoramadhan/UrDevice/issues/33), [#34](https://github.com/ahmadridhoramadhan/UrDevice/issues/34))
- Browse and search the other desktop's shared folders (R2, [#36](https://github.com/ahmadridhoramadhan/UrDevice/issues/36))
- Shared clipboard history (R2, [#37](https://github.com/ahmadridhoramadhan/UrDevice/issues/37))
- Make the other desktop ring (R2, [#27](https://github.com/ahmadridhoramadhan/UrDevice/issues/27))
- Play this desktop's sound on the other desktop (R2, [#40](https://github.com/ahmadridhoramadhan/UrDevice/issues/40))

**desktop => mobile**

- Sync clipboard (R1, [#16](https://github.com/ahmadridhoramadhan/UrDevice/issues/16))
- Send files (R1, [#17](https://github.com/ahmadridhoramadhan/UrDevice/issues/17))
- Browse and download the phone's photos and videos (R1, [#17](https://github.com/ahmadridhoramadhan/UrDevice/issues/17))
- Control the phone: taps, swipes, back, home, and recents (R1, [#19](https://github.com/ahmadridhoramadhan/UrDevice/issues/19))
- Type into the phone through the UrDevice keyboard (R1, [#19](https://github.com/ahmadridhoramadhan/UrDevice/issues/19))
- View the phone's screen in a window and control it from there (R1, [#21](https://github.com/ahmadridhoramadhan/UrDevice/issues/21))
- Control media playing on the phone (R1, [#20](https://github.com/ahmadridhoramadhan/UrDevice/issues/20))
- Keep working over Bluetooth when Wi-Fi drops, with fewer features (R1, [#23](https://github.com/ahmadridhoramadhan/UrDevice/issues/23))
- Use the phone's camera and microphone as a webcam (R2, [#28](https://github.com/ahmadridhoramadhan/UrDevice/issues/28))
- See and answer the phone's notifications (R2, [#26](https://github.com/ahmadridhoramadhan/UrDevice/issues/26))
- Read and send SMS and handle calls through the phone, in the `full` Android build (R2, [#29](https://github.com/ahmadridhoramadhan/UrDevice/issues/29))
- Use the phone or tablet as a second display (R2, [#31](https://github.com/ahmadridhoramadhan/UrDevice/issues/31))
- Debug the phone with ADB and Chrome DevTools over Wi-Fi (R2, [#35](https://github.com/ahmadridhoramadhan/UrDevice/issues/35))
- Browse the phone's shared folders (R2, [#36](https://github.com/ahmadridhoramadhan/UrDevice/issues/36))
- Shared clipboard history (R2, [#37](https://github.com/ahmadridhoramadhan/UrDevice/issues/37))
- Make the phone ring, even on silent (R2, [#27](https://github.com/ahmadridhoramadhan/UrDevice/issues/27))
- Play the desktop's sound on the phone (R2, [#40](https://github.com/ahmadridhoramadhan/UrDevice/issues/40))

**mobile => desktop**

- Sync clipboard, sent with a button, the Quick Settings tile, or the share sheet (R1, [#16](https://github.com/ahmadridhoramadhan/UrDevice/issues/16))
- Send files from the share sheet (R1, [#17](https://github.com/ahmadridhoramadhan/UrDevice/issues/17))
- Mousepad: use the phone as a touchpad (R1, [#18](https://github.com/ahmadridhoramadhan/UrDevice/issues/18))
- Presenter: next and previous slide, laser pointer, and timer (R1, [#18](https://github.com/ahmadridhoramadhan/UrDevice/issues/18))
- Keyboard: type on the desktop from the phone (R1, [#18](https://github.com/ahmadridhoramadhan/UrDevice/issues/18))
- View the desktop's screen and control it (R1, [#21](https://github.com/ahmadridhoramadhan/UrDevice/issues/21))
- Control the desktop's media (R1, [#20](https://github.com/ahmadridhoramadhan/UrDevice/issues/20))
- Open the desktop's localhost dev server in the phone's browser (R1, [#22](https://github.com/ahmadridhoramadhan/UrDevice/issues/22))
- Keep working over Bluetooth when Wi-Fi drops, with fewer features (R1, [#23](https://github.com/ahmadridhoramadhan/UrDevice/issues/23))
- See the desktop's notifications, from Windows and Linux (R2, [#26](https://github.com/ahmadridhoramadhan/UrDevice/issues/26))
- Remote terminal on the desktop (R2, [#32](https://github.com/ahmadridhoramadhan/UrDevice/issues/32))
- Port forwarding profiles to reach the desktop's ports (R2, [#33](https://github.com/ahmadridhoramadhan/UrDevice/issues/33))
- Lock the desktop when you walk away, or with a button (R2, [#38](https://github.com/ahmadridhoramadhan/UrDevice/issues/38))
- Browse the desktop's shared folders (R2, [#36](https://github.com/ahmadridhoramadhan/UrDevice/issues/36))
- Shared clipboard history (R2, [#37](https://github.com/ahmadridhoramadhan/UrDevice/issues/37))
- Make the desktop ring (R2, [#27](https://github.com/ahmadridhoramadhan/UrDevice/issues/27))
- Play the phone's sound on the desktop (R2, [#40](https://github.com/ahmadridhoramadhan/UrDevice/issues/40))

**mobile => mobile**

The spec allows these between two Android devices. The first test plan covers Android ↔ desktop and desktop ↔ desktop. Remote control, keyboard, gallery, and notification mirroring are not planned between two phones.

- Sync clipboard (R1, [#16](https://github.com/ahmadridhoramadhan/UrDevice/issues/16))
- Send files (R1, [#17](https://github.com/ahmadridhoramadhan/UrDevice/issues/17))
- View the other phone's screen (R1, [#21](https://github.com/ahmadridhoramadhan/UrDevice/issues/21))
- Control media on the other phone (R1, [#20](https://github.com/ahmadridhoramadhan/UrDevice/issues/20))
- Browse the other phone's shared folders (R2, [#36](https://github.com/ahmadridhoramadhan/UrDevice/issues/36))
- Shared clipboard history (R2, [#37](https://github.com/ahmadridhoramadhan/UrDevice/issues/37))
- Make the other phone ring (R2, [#27](https://github.com/ahmadridhoramadhan/UrDevice/issues/27))
- Play this phone's sound on the other phone (R2, [#40](https://github.com/ahmadridhoramadhan/UrDevice/issues/40))

## License

MIT
