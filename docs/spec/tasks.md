# UrDevice: Implementation tasks

The tasks follow [design.md](design.md) and are grouped into phases. Each phase ends with something that works end to end. `_Req:` lists the requirements a task covers. Tasks marked `*` are optional test tasks that can be deferred.

## Phase 0: Setup and risk spikes

- [ ] T0.1 Create the Cargo workspace with the crates from design §2, add `rustfmt`/`clippy` config, and add a CI workflow (build + clippy + `cargo deny` + `cargo audit` on Linux, macOS, and Windows). _Req: R16_
- [ ] T0.2 Create the Android Gradle project (Kotlin, Compose, minSdk 29) and build `urdevice-ffi` for `arm64-v8a`, `armeabi-v7a`, and `x86_64` with `cargo-ndk`. Generate UniFFI Kotlin bindings and call one `core_version()` function from the app. _Req: R15_
- [ ] T0.3 Spike: try QUIC between Android and a desktop on real Wi-Fi, including multicast discovery through `MulticastLock` and NsdManager. Measure round-trip time. _Req: R2, R4_
- [ ] T0.4 Spike: check the Google Play policy for AccessibilityService remote-control use and write a short disclosure plan. Record the decision in `docs/`. _Req: R9.3, R15_
- [ ] T0.5 Spike: Bluetooth PAN between Android and Windows/Linux, and a `btleplug` GATT connection between macOS and Android. Confirm that the fallback in design §8 works. _Req: R6_
- [ ] T0.6 Spike: Wayland RemoteDesktop and ScreenCast portals on GNOME and KDE. Write down which compositors work. _Req: R9.2, R12.2_

## Phase 1: Core security foundation

- [ ] T1.1 `urdevice-proto`: protobuf schemas for Announce, Hello/Welcome, PairInvite/PairProof/PairAccept, and the control envelope. Add length-prefixed framing with a 1 MiB limit. _Req: R4.6_
- [ ] T1.2 Identity: create the Ed25519 key and the `rcgen` certificate, compute the Device ID and readable fingerprint, store the key through `keyring` (with an encrypted-file fallback), and support reset. _Req: R1_
- [ ] T1.3 Trust store: SQLite schema from design §4.2 with row HMACs. Add CRUD for paired devices, permissions, the activity log (30-day pruning), and shared ports. _Req: R3.6, R3.9, R5.1, R5.6_
- [ ] T1.4 Transport: quinn endpoint, ALPN `urdevice/1`, custom client and server certificate verifiers with pinning, a pairing-context hook, a rule for duplicate connections, and keep-alive. _Req: R4.1, R4.2_
- [ ] T1.5 Session: Hello/Welcome handshake, capability negotiation, version rejection, the stream multiplexer, the datagram channel, and reconnect with backoff. _Req: R4.3–R4.5_
- [ ] T1.6 Discovery: mDNS advertise and browse, UDP broadcast announcements, a rate-limited parser, the PeerTable, auto-connect to paired peers, and hidden mode. _Req: R2_
- [ ] T1.7 QR pairing: create the invite and token, encode the URI, verify the HMAC over the TLS exporter, make tokens single-use with expiry, and add the lockout counter. _Req: R3.1–R3.4, R3.7, R3.8_
- [ ] T1.8 Code-confirmation pairing: derive the 6-digit code from the exporter and require confirmation on both sides. _Req: R3.5_
- [ ] T1.9 Write `docs/security/threat-model.md` from design §15. _Req: R4, R5_
- [ ] T1.10 Permission engine and ActivityMonitor: Allow/Ask/Deny, the 30 s timeout, revoke and stop, and the defaults from R5.2. _Req: R5_
- [ ] T1.11 Logging with `tracing` and a redaction layer. _Req: R16.2_
- [ ]* T1.12 Integration tests: both pairing flows, wrong-fingerprint rejection, token replay, lockout, reconnect, and unpair-then-reject.
- [ ]* T1.13 Fuzz targets: control-message framing, Announce parser, and QR payload parser.

## Phase 2: App shells

- [ ] T2.1 Desktop: Slint app with a tray icon and start at login. Add the Devices list, the Pair screen (QR display and code confirmation), Device detail with permission settings, and the Settings screen (name, hidden mode, reset identity, data export and delete). _Req: R1.4, R1.5, R2, R3, R5, R14.1, R14.2, R16.3_
- [ ] T2.2 Desktop: permission prompt dialog, a "being controlled/viewed" indicator with a Stop button, and an Activity log view. _Req: R5.3–R5.6_
- [ ] T2.3 Android: foreground service that hosts the core, NsdManager discovery adapter, and Compose Devices/Pair screens (CameraX + ML Kit QR scanner, the app's own QR display, code confirmation). _Req: R2, R3, R15.1_
- [ ] T2.4 Android: permission screens, a notification with Stop, the permission prompt UI, and the Settings screen. _Req: R5, R15.2, R16.3_
- [ ] T2.5 Localization setup on both platforms (Slint `@tr`, Android `strings.xml`). _Req: R16.4_

## Phase 3: Clipboard and files

- [ ] T3.1 Clipboard service in the core: the Clip message, echo prevention, the size limit, and large images over bulk streams. _Req: R7.1, R7.2, R7.4_
- [ ] T3.2 Desktop clipboard adapters with change events and sensitive-flag detection on each OS. _Req: R7.3_
- [ ] T3.3 Android clipboard: write incoming content; send through the notification action, Quick Settings tile, and share sheet; detect `EXTRA_IS_SENSITIVE`; add per-device and global toggles. _Req: R7.3, R7.5, R7.6_
- [ ] T3.4 File transfer service: Offer/Accept/Data/Done, `.urdevice-part` files with streaming SHA-256, resume sidecars, cancel, and progress events. _Req: R8.2–R8.5_
- [ ] T3.5 Path cleaning and collision renaming. _Req: R8.6_
- [ ]* T3.6 Property tests for path cleaning and fuzzing for transfer framing.
- [ ] T3.7 Desktop transfers UI: file picker, drag-and-drop, progress list, and incoming-folder setting. _Req: R8.1, R8.3, R8.6_
- [ ] T3.8 Android transfers: share-sheet target, picker, progress notifications, and saving through MediaStore Downloads. _Req: R8.1, R8.3, R15.3_
- [ ] T3.9 Gallery: Android MediaStore provider with thumbnails and paging, and a desktop gallery browser with download. _Req: R8.7_
- [ ] T3.10 Measure transfer throughput and tune chunk size and QUIC window settings. _Req: R8.8_

## Phase 4: Remote control and keyboard

- [ ] T4.1 Input protocol (Pointer, Key, Text, Laser) over datagrams, with a fallback to the control stream. _Req: R9, R10_
- [ ] T4.2 Desktop `InputSink`: Windows SendInput, macOS CGEvent (plus the Accessibility permission check and prompt), X11 XTest, the Wayland RemoteDesktop portal with a saved restore token, and the `uinput` fallback with its udev-rule setup. _Req: R9.2, R9.4_
- [ ] T4.3 Android Mousepad screen: gestures, acceleration and sensitivity settings, keep-screen-on. _Req: R9a_
- [ ] T4.4 Android Presenter screen: buttons, volume-key capture, laser (gyroscope and touch), and the timer with vibration alerts. _Req: R9b_
- [ ] T4.5 Desktop laser overlay window on each OS, with the cursor-move fallback. _Req: R9b.3_
- [ ] T4.6 Android Keyboard screen: Unicode text, special keys, and modifier shortcuts. _Req: R10.1, R10.4_
- [ ] T4.7 Android host: AccessibilityService for gestures and global actions, and the UrDevice IME for text. _Req: R9.3, R10.3_
- [ ] T4.8 Desktop → desktop: remote-control window that captures pointer and keyboard input. _Req: R9.1, R10.2_
- [ ] T4.9 Measure input latency (95th percentile, target under 50 ms). _Req: R9.5_

## Phase 5: Media control

- [ ] T5.1 Media protocol (MediaCmd, NowPlaying push) and the core service. _Req: R11.1_
- [ ] T5.2 Hosts: Windows SMTC, Linux MPRIS, macOS media keys and CoreAudio volume, Android MediaSession with a NotificationListener. _Req: R11.2–R11.4_
- [ ] T5.3 Media remote UI on Android and desktop. _Req: R11.1, R11.2_

## Phase 6: Screen sharing

- [ ] T6.1 `urdevice-media`: encoder abstraction with low-latency H.264 settings, hardware back-ends, and the `openh264` fallback. _Req: R12.3_
- [ ] T6.2 Desktop `ScreenSource`: Windows Graphics Capture, ScreenCaptureKit, the PipeWire portal, and the X11 fallback, plus the display list. _Req: R12.2, R12.6_
- [ ] T6.3 Video streaming: frame format, sender-side frame dropping, keyframe requests, and the adaptive bitrate ladder. _Req: R12.4_
- [ ] T6.4 Desktop viewer: decode into a Slint image, display picker, and input forwarding when allowed. _Req: R12.5–R12.7_
- [ ] T6.5 Android host: MediaProjection with the foreground-service type, MediaCodec Surface encoding. _Req: R12.2, R12.3_
- [ ] T6.6 Android viewer: MediaCodec to SurfaceView, plus pinch-to-zoom and touch-to-pointer mapping. _Req: R12.1, R12.5_
- [ ] T6.7 Measure screen latency (target under 150 ms at 1080p30). _Req: R12.4_

## Phase 7: Localhost sharing

- [ ] T7.1 Host side: shared-port allow-list per device, TunnelOpen checks, connections to loopback only. _Req: R13.1, R13.5_
- [ ] T7.2 Viewer side: loopback listener, choosing a free port when P is taken, bidirectional copying over QUIC streams, connection and traffic stats. _Req: R13.2–R13.4, R13.7_
- [ ] T7.3 Optional LAN exposure with a warning and 1-hour expiry. _Req: R13.6_
- [ ] T7.4 Localhost UI on both platforms, including "Open in browser". _Req: R13.1, R13.7_
- [ ]* T7.5 Integration tests: refusal of ports that aren't shared, WebSocket and HTTP/2 traffic, and hot reload with a Vite dev server.

## Phase 8: Bluetooth fallback

- [ ] T8.1 Interface watcher and PAN path selection, with QUIC migration and reconnect. _Req: R6.1, R6.5_
- [ ] T8.2 BLE channel: Android GATT server and `btleplug` central, Noise IK with the pairing-time static keys, MTU framing and reassembly. _Req: R6.2, R6.3_
- [ ] T8.3 Routing of only the allowed features over BLE, `Unavailable(FallbackPath)` messages in the UI, and the guide to Bluetooth tethering settings. _Req: R6.2, R6.4, R6.6_
- [ ]* T8.4 Fuzz target for BLE reassembly.

## Phase 9: Hardening and release

- [ ] T9.1 Battery profiling on Android and tuning of discovery and keep-alive. _Req: R15.4_
- [ ] T9.2 Run the device-matrix smoke tests (design §16) and fix issues found.
- [ ] T9.3 Packaging and signing: macOS `.dmg` with notarization, Windows `.msi` with Authenticode, Linux `.deb`/`.rpm`/AppImage/Flatpak, and a signed Android AAB. _Req: R14.3_
- [ ] T9.4 External security review of pairing, transport, and permissions, and fixes for its findings. _Req: R3, R4, R5_
- [ ] T9.5 User documentation in `docs/`: getting started, permissions on each OS, and troubleshooting.

---

# Release 2

Start after Release 1 (Phases 0–9) is stable. Design references are to [design §18](design.md#18-release-2-features).

## Phase 10: Release 2 risk spikes

- [ ] T10.1 Spike: Wayland InputCapture portal on GNOME and KDE. List which desktops can share their mouse. _Req: R19.3_
- [ ] T10.2 Spike: virtual cameras: a macOS Camera Extension, `MFCreateVirtualCamera` on Windows 11, and `v4l2loopback` on Linux, each showing a test pattern in a video-call app. _Req: R17.2_
- [ ] T10.3 Spike: virtual displays: the ScreenCast virtual source on GNOME and KDE, a sample IddCx driver on Windows (including the signing steps and cost), and `CGVirtualDisplay` on the current macOS. _Req: R20.2_
- [ ] T10.4 Spike: Google Play policy for full-screen intents, SMS, and call permissions. Confirm the `play`/`full` build split. _Req: R25.1, R29.3_

## Phase 11: Phone-centered features

- [ ] T11.1 Notification mirroring: listener service changes, per-app and visibility filtering, desktop native notifications, reply and actions, dismissal sync, 24 h retention. _Req: R18_
- [ ] T11.2 Find my device: Ring messages, Android alarm-stream ringing with the full-screen fallback, desktop ringing window, last-seen display. _Req: R25_
- [ ] T11.3 Webcam: CameraX → MediaCodec stream, camera controls, desktop viewer window. _Req: R17.1, R17.3, R17.5, R17.6_
- [ ] T11.4 Virtual camera back-ends on macOS, Windows 11, and Linux. _Req: R17.2_
- [ ] T11.5 Microphone: Opus stream and PipeWire virtual source on Linux, with the "not yet available" message on macOS and Windows. _Req: R17.4_
- [ ] T11.6 Android `play` and `full` build flavors. _Req: R29.3_
- [ ] T11.7 SMS and calls in the `full` build: conversations, sending, incoming-call controls, encrypted desktop cache. _Req: R29_

## Phase 12: Desktop ↔ desktop features

- [ ] T12.1 Display layout sharing and the layout editor UI. _Req: R19.1_
- [ ] T12.2 Input capture back-ends: Windows hooks, macOS `CGEventTap`, X11 XInput2, Wayland InputCapture. _Req: R19.3_
- [ ] T12.3 Edge handoff (KvmEnter/KvmLeave), hotkeys, clipboard handoff, latency measurement. _Req: R19.2, R19.4–R19.6_
- [ ] T12.4 Second display: virtual display back-ends from the T10.3 results, streaming to Android, touch and pen mapping, cleanup when the stream stops. _Req: R20_
- [ ] T12.5 Remote terminal host: `portable-pty`, TermOpen/TermResize, local-only permission flag. _Req: R21.1–R21.4, R21.6_
- [ ] T12.6 Terminal viewers: desktop (`alacritty_terminal` + Slint grid) and Android (`vt100` + Compose grid with extra keys). _Req: R21.5_
- [ ]* T12.7 Integration tests: a remote request can't change the terminal permission; terminal sessions end on Stop.

## Phase 13: Developer features

- [ ] T13.1 Port forwarding profiles: storage, auto-start, host-initiated ForwardOffer, one-tap toggles. _Req: R23_
- [ ] T13.2 HTTP request inspector: read-only parser, TLS/HTTP/2 detection, ring buffer, hidden header values, filters, resend. _Req: R24_
- [ ] T13.3 ADB over the tunnel: find `adbd` ports, pairing helper, Chrome tab list, DevTools launch. _Req: R22_
- [ ]* T13.4 Fuzz target for the HTTP inspector parser.

## Phase 14: Files, history, lock, and automations

- [ ] T14.1 Remote file browser: Fs protocol, `cap-std` roots on desktop, SAF adapter on Android, name search, thumbnails, write operations behind `fs.write.v1`. _Req: R26_
- [ ]* T14.2 Property tests: file-browser paths never escape a shared root, including through symbolic links.
- [ ] T14.3 Clipboard history: encrypted storage, sync with tombstones, search, pin, expiry, Clear all. _Req: R27_
- [ ] T14.4 Proximity lock: RSSI averaging, calibration UI, lock back-ends for each OS, presenting and screen-sharing exceptions, lock-now button. _Req: R28_
- [ ] T14.5 Automation engine: rules, triggers, actions, Android notification fallback, desktop-only "run command" setting. _Req: R30_
- [ ] T14.6 Automation UI on desktop and Android. _Req: R30.1_

## Phase 15: Release 2 hardening

- [ ] T15.1 External security review of the terminal, file browser, run command, and SMS features. _Req: R21, R26, R29, R30_
- [ ] T15.2 Extend the device-matrix smoke tests and user docs to the Release 2 features.
