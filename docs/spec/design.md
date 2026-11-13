# UrDevice: Design

This document describes how UrDevice meets [requirements.md](requirements.md). References such as **R3.4** mean Requirement 3, criterion 4.

## 1. Overview

UrDevice is a set of equal peers. Every device runs the same Rust core. The core handles identity, discovery, pairing, encrypted transport, permissions, and the feature protocols. Each platform supplies a UI and a set of OS adapters (clipboard, input injection, screen capture, media, and so on) through traits defined in the core.

```
┌─────────────────────────────┐        ┌─────────────────────────────┐
│ Desktop app (Slint UI)      │        │ Android app (Kotlin/Compose)│
│  ├─ OS adapters (Rust)      │        │  ├─ OS adapters (Kotlin)    │
│  └─ urdevice-core (Rust) ◄──┼─QUIC──►┼──└─ urdevice-core via       │
│                             │ TLS1.3 │      UniFFI (.so + Kotlin)  │
└─────────────────────────────┘        └─────────────────────────────┘
          ▲  mDNS + UDP broadcast discovery  ▲
          └──── Bluetooth PAN / BLE fallback ┘
```

### Key decision: one Rust core shared with Android

The Android app stays native Kotlin for UI and Android APIs, but it does not reimplement the protocol. It loads `urdevice-core` as a native library through [UniFFI](https://mozilla.github.io/uniffi-rs/).

| Option | Pros | Cons |
|---|---|---|
| **Shared Rust core via UniFFI (chosen)** | One implementation of crypto, pairing, and protocol, so the platforms can't drift apart; fuzzing and audits cover every platform at once | Adds a JNI boundary, cross-compiling for 3–4 Android ABIs, and roughly 5–8 MB more APK size |
| Reimplement the protocol in Kotlin | Pure Kotlin toolchain | Two copies of security-critical code, and two copies of every protocol change to test |

Security-critical code must be correct on every platform, so the shared core wins.

## 2. Repository layout

```
UrDevice/
├─ Cargo.toml                 # Rust workspace
├─ crates/
│  ├─ urdevice-proto/         # Protobuf schemas + generated types (prost)
│  ├─ urdevice-core/          # identity, trust store, discovery, transport, session, services
│  ├─ urdevice-platform/      # desktop OS adapters (cfg: windows / macos / linux)
│  ├─ urdevice-media/         # capture + H.264 encode/decode wrappers
│  ├─ urdevice-ffi/           # UniFFI bindings for Android
│  └─ urdevice-desktop/       # Slint app (bin)
├─ android/                   # Gradle project, Kotlin + Jetpack Compose
│  ├─ app/
│  └─ core-bindings/          # generated Kotlin + jniLibs from urdevice-ffi
├─ fuzz/                      # cargo-fuzz targets
└─ docs/
   ├─ spec/                   # this spec
   └─ security/threat-model.md
```

## 3. Technology choices

| Area | Choice | Reason |
|---|---|---|
| Async runtime | `tokio` | Standard choice; works with quinn |
| Transport | `quinn` (QUIC) + `rustls` | Multiplexed streams (control, files, video, tunnels) on one encrypted connection; built-in connection migration helps with the PAN fallback (R6.1) |
| Crypto | `ed25519-dalek`, `rcgen`, `ring`/`aws-lc-rs` through rustls, `snow` (Noise) for BLE | Well-reviewed, pure-Rust or FIPS-capable libraries |
| Serialization | Protobuf (`prost`) | Handles schema changes across versions; message sizes are explicit |
| Discovery | `mdns-sd` + raw UDP socket | Pure Rust, works on all desktop OSes; Android uses NsdManager through an adapter, because it handles multicast locks better |
| Secure storage | `keyring` crate (desktop); Android Keystore (AES-GCM key that wraps the identity key) | R1.2 |
| Desktop UI | Slint | Required by the project |
| Android UI | Kotlin + Jetpack Compose, CameraX + ML Kit for QR scanning | Current native standard |
| Clipboard (desktop) | `arboard` plus platform code for change events and sensitive flags | R7 |
| Video | Hardware H.264: Media Foundation / VideoToolbox / VA-API / MediaCodec; `openh264` as the software fallback | R12.3 |

## 4. Identity and trust

### 4.1 Identity (R1)

- **Identity key:** Ed25519 key pair, created on first run.
- **Certificate:** a self-signed X.509 certificate made with `rcgen` from the identity key. Its CN is the Device ID, and it is valid for 20 years. Trust comes from pinning, not from the certificate chain, so expiry doesn't matter.
- **Device ID:** `SHA-256(SubjectPublicKeyInfo)`, shown in base32 as 8 groups of 4 characters. Discovery uses the first 8 bytes as a short ID.
- **BLE static key:** an X25519 key pair, signed by the identity key, used for the Noise fallback channel (§8.2). It is exchanged during pairing.

### 4.2 Trust store

The trust store is a SQLite database (`rusqlite`) in the app data directory, with 0600 permissions on desktop and app-private storage on Android. It holds no secrets, but changes are checked: every row includes an HMAC keyed by a secret kept in secure storage, which detects tampering by other local processes.

```
paired_device(
  device_id        BLOB PRIMARY KEY,   -- 32 bytes
  cert_fingerprint BLOB NOT NULL,
  ble_static_pub   BLOB,
  name             TEXT, platform TEXT,
  paired_at        INTEGER, last_seen INTEGER,
  last_addrs       TEXT,               -- JSON list of SocketAddr
  row_mac          BLOB NOT NULL
)
permission(device_id, capability, mode /* allow|ask|deny */, PRIMARY KEY(device_id, capability))
activity_log(id, device_id, capability, decision, ts)   -- pruned after 30 days (R5.6)
shared_port(port, label, allowed_devices JSON, lan_expose_until INTEGER NULL)
```

## 5. Discovery (R2)

- **mDNS:** service `_urdevice._udp.local.` with instance name `<display name>`. TXT records: `v=<proto>`, `id=<short id hex>`, `p=<platform>`. The SRV record carries the QUIC port.
- **UDP broadcast:** sent to `255.255.255.255:47810` and to each interface's directed broadcast address every 5 s. The payload is a protobuf `Announce { v, short_id, name, port, platform }` with a 4-byte magic `URDV` in front and a size limit of 512 bytes.
- **Receiving:** packets are parsed with a hard size limit and rate-limited per source with a token bucket (R2.6). Results go into a `PeerTable` keyed by short ID. If a short ID matches a paired device, the core connects and then checks the full fingerprint during TLS, so a forged announcement can't do more than cause a failed connection attempt.
- **Android:** the app holds a `WifiManager.MulticastLock` only while the app is in the foreground or a session is being looked for, to save battery (R15.4).
- **Hidden mode (R2.7):** stops sending announcements. Known peers can still be reached at their `last_addrs`.

Discovery data is unauthenticated by design. It only tells devices where to try connecting. All trust decisions happen in TLS.

## 6. Transport and session (R4)

### 6.1 Connection

- Each device listens on one UDP port (default 47811; it picks another if that port is taken) for QUIC.
- ALPN: `urdevice/1`.
- **Mutual TLS 1.3** with custom `rustls` verifiers on both sides:
  - `ServerCertVerifier` / `ClientCertVerifier` accept a certificate only if its fingerprint is in the trust store, **or** a pairing context is active and the fingerprint matches what that context expects (QR flow) or the context is in confirm-code mode.
  - TLS signatures are still checked with the certificate's key, so the peer must hold the private key.
- When both sides find each other at the same moment and two connections open, the connection started by the device with the lower Device ID wins.

### 6.2 Streams

| Stream | Type | Use |
|---|---|---|
| Control | bidirectional stream 0 | Handshake, capabilities, permission requests, clipboard, input, media, presenter commands |
| Bulk | one bidirectional stream per transfer | Files, gallery thumbnails and downloads |
| Video | unidirectional stream per screen session, plus QUIC datagrams for input | Screen sharing (§10) |
| Tunnel | one bidirectional stream per TCP connection | Localhost sharing (§11) |

Control messages are length-prefixed protobuf (`u32` length, max 1 MiB; anything larger is rejected and closes the session, R4.6). Input events use **QUIC datagrams** when the peer supports them, so a lost packet never blocks newer input.

### 6.3 Handshake

```
Client → Server: Hello { proto_min, proto_max, app_version, capabilities[] }
Server → Client: Welcome { proto, capabilities[], session_id }
```

Capabilities are strings such as `clipboard.v1`, `files.v1`, `input.v1`, `screen.h264.v1`, `tunnel.v1`, `media.v1`, `ble.fallback.v1`. The session enables only the capabilities both sides list. Local permissions (§7) are then checked for each request.

### 6.4 Reconnect (R4.5)

When a session drops, the core tries `last_addrs`, any newly discovered addresses, and then fallback paths, waiting 1, 2, 4, … up to 30 s between attempts. QUIC connection migration covers address changes such as Wi-Fi to PAN while the connection is still alive. Without it, a fresh handshake runs with the pinned keys and needs no user action.

## 7. Pairing and permissions

### 7.1 QR pairing (R3.1–R3.4)

QR payload: a URI `urdevice://pair?d=<base64url(protobuf)>` with

```
PairInvite { v, device_id[32], name, platform, addrs[], port, token[16], expires_at }
```

Flow:

1. The host (A) creates a 16-byte token from the OS CSPRNG, opens a pairing context that expects *any* client certificate, and shows the QR code.
2. The scanner (B) connects to `addrs`. B's verifier accepts A's certificate **only if** its fingerprint equals `device_id` from the QR code. This authenticates A.
3. B computes `binding = TLS-Exporter("EXPORTER-urdevice-pair", 32)` and sends `PairProof { hmac = HMAC-SHA256(token, binding ‖ fp_A ‖ fp_B), name, platform, ble_static_pub, sig }`.
4. A recomputes the HMAC and compares it in constant time. If it matches, A stores B and replies `PairAccept { name, platform, ble_static_pub, sig }`. B then stores A.
5. The token is destroyed after one attempt, successful or not (R3.2).

The channel binding ties the proof to this specific TLS session, so a captured proof can't be replayed or relayed.

### 7.2 Code-confirmation pairing (R3.5)

This flow is used for devices found by discovery. Both pairing contexts accept any certificate. After TLS completes, each side computes `code = u32(TLS-Exporter("EXPORTER-urdevice-sas", 4)) mod 10^6` and shows it. The pair is stored only after **both** users tap "Codes match". A man-in-the-middle would end up with two different TLS sessions and two different codes. The chance of guessing a matching code is 1 in 10⁶ per attempt, and the lockout (R3.7) makes repeated guessing impractical.

### 7.3 Pairing limits

- Pairing contexts exist only while the pairing screen is open (R3.8), and each lasts at most 120 s.
- Failure counter: 5 failures within a 5-minute window locks pairing for 5 minutes (R3.7).

### 7.4 Permission engine (R5)

`PermissionEngine::check(device_id, capability) -> Allow | Deny | Ask(Future<Decision>)`.

- `Ask` sends a prompt to the UI through a `UiDelegate` trait. If the user doesn't answer within 30 s, the request is denied.
- Each grant applies to one *activity* (one transfer, one screen session, one input session), unless the user checks "Always allow".
- Active input and screen activities register with an `ActivityMonitor`. It drives the on-screen indicator and the Stop button. Calling `revoke()` cancels the activity's tokio tasks and closes its streams. The target is under 1 s, and in practice it is immediate (R5.4, R5.5).
- Defaults follow R5.2.

## 8. Bluetooth fallback (R6)

### 8.1 Bluetooth PAN (IP path)

Bluetooth PAN gives both devices an ordinary IP network, so no protocol changes are needed. The core watches network interfaces (`if-watch`) and tries a peer's PAN-side address when the LAN path fails. QUIC migration or a reconnect then moves the session over.

Platform reality:

| Pair | PAN available? |
|---|---|
| Android (tethering on) ↔ Windows / Linux | Yes. Android acts as the network access point. The user turns on Bluetooth tethering; the app opens the settings screen (R6.6) |
| Android ↔ macOS | **No.** macOS removed Bluetooth PAN in Monterey ([Apple Community](https://discussions.apple.com/thread/253312557)), so the BLE channel is used instead |
| Desktop ↔ desktop | Out of scope for v1 |

### 8.2 BLE control channel

- Android runs a GATT server with a UrDevice service UUID and one characteristic for each direction (write-without-response and notify). The desktop is the GATT central: `btleplug` on Windows, Linux, and macOS.
- Security: **Noise IK** (`snow`, `Noise_IK_25519_ChaChaPoly_SHA256`) using the BLE static keys exchanged and signed during pairing (§4.1). It does not depend on Bluetooth-level pairing security.
- Framing: Noise messages split to fit the negotiated MTU, with a 2-byte sequence number and reassembly.
- Allowed messages: input, presenter, media, keyboard, and clipboard text up to 64 KB (R6.2). Everything else returns `Unavailable(FallbackPath)` to the UI (R6.4).
- When the LAN or PAN path comes back, the session is moved to it within 10 s (R6.5).

## 9. Feature services

All services are implemented in the core against platform traits:

```rust
trait Clipboard   { fn read(&self) -> Result<ClipContent>; fn write(&self, c: ClipContent) -> Result<()>;
                    fn watch(&self) -> BoxStream<ClipEvent>; /* ClipEvent carries `sensitive: bool` */ }
trait InputSink   { fn pointer(&self, e: PointerEvent) -> Result<()>; fn key(&self, e: KeyEvent) -> Result<()>;
                    fn text(&self, s: &str) -> Result<()>; fn permission_status(&self) -> PermStatus; }
trait ScreenSource{ fn displays(&self) -> Vec<DisplayInfo>; fn start(&self, d: DisplayId, cfg: CaptureCfg) -> Result<FrameStream>; }
trait MediaHost   { fn now_playing(&self) -> Option<NowPlaying>; fn command(&self, c: MediaCmd) -> Result<()>; }
trait Overlay     { fn laser(&self, pos: Option<(f32,f32)>) -> Result<()>; }   // presenter laser
trait FileStore   { fn incoming_dir(&self) -> PathBuf; fn gallery(&self) -> Option<Box<dyn Gallery>>; }
trait UiDelegate  { fn ask(&self, req: PermissionPrompt) -> BoxFuture<Decision>; fn notify(&self, n: Notice); }
```

On desktop these traits are implemented in `urdevice-platform`. On Android they are UniFFI **callback interfaces** implemented in Kotlin.

### 9.1 Clipboard (R7)

- On desktop, the watcher uses OS change events where they exist (`AddClipboardFormatListener` on Windows, X11 XFixes / Wayland `wlr-data-control`) and polls `NSPasteboard.changeCount` every 500 ms on macOS.
- Sensitive content is never sent (R7.3).
- Echo prevention: the core keeps the hash of the last written and last received content and ignores watcher events that match it (R7.4).
- Message: `Clip { id, mime, bytes (≤10 MB), origin_device }`. Images over 1 MB go over a bulk stream.
- Android: content is read only while the app is in the foreground or through a user action (notification button, Quick Settings tile, share sheet). Incoming content is written with `ClipboardManager.setPrimaryClip`, which Android allows from a foreground service (R7.5).

### 9.2 File transfer (R8)

```
Offer   { transfer_id, files[{rel_path, size, sha256?}], total_size }
Accept  { transfer_id, resume[{index, offset}] } | Decline { reason }
Data    on bulk stream: [index u32][offset u64][len u32][bytes]   (chunk = 1 MiB)
Done    { transfer_id, per-file sha256 }
```

- **Path safety (R8.6):** each `rel_path` is split into components. The receiver rejects `..`, absolute paths, drive letters, NUL bytes, and Windows reserved names (`CON`, `NUL`, …). It joins the clean path under the incoming folder, then canonicalizes and checks that the result is still inside that folder. Name collisions become `name (1).ext`.
- Data is written to `*.urdevice-part`, hashed while streaming, and renamed only after the hash matches (R8.4).
- **Resume (R8.5):** the receiver keeps a sidecar file with the verified offset for each file and sends it back in `Accept.resume`.
- Progress is calculated from acknowledged bytes and passed to the UI about every 100 ms.
- **Gallery (R8.7):** the Android `Gallery` uses MediaStore (`READ_MEDIA_IMAGES` / `READ_MEDIA_VIDEO` on API 33+, `READ_EXTERNAL_STORAGE` on older versions). Messages are `GalleryList { cursor, page }` → items with 256-px JPEG thumbnails, then `GalleryFetch { id }`, which reuses the file-transfer path.

### 9.3 Remote input (R9, R10)

```
PointerEvent { kind: Move{dx,dy} | Abs{x,y,display} | Button{btn,down} | Scroll{dx,dy} }
KeyEvent     { key: KeyCode (USB HID usage), down, modifiers }
TextInput    { text }    // Unicode text, sent as-is (R10.4)
```

| Host | Injection |
|---|---|
| Windows | `SendInput`; text uses `KEYEVENTF_UNICODE`. Note: Windows blocks injection into elevated windows unless UrDevice also runs elevated (documented limitation) |
| macOS | `CGEventPost`, which needs the Accessibility permission; text uses `CGEventKeyboardSetUnicodeString` |
| Linux X11 | XTest |
| Linux Wayland | `org.freedesktop.portal.RemoteDesktop` (with libei where available). The user approves once and the portal restore token is saved. On compositors without the portal (some wlroots-based ones), a `uinput` virtual keyboard and mouse is used instead. It needs a udev rule that gives the user's group access to `/dev/uinput`, which the app offers to install with a one-time admin prompt |
| Android | `AccessibilityService.dispatchGesture` for taps and swipes, and `performGlobalAction` for back/home/recents. Text goes through the UrDevice IME (`InputMethodService.currentInputConnection.commitText`) |

**Mousepad (R9a), on the Android side:** a Compose `pointerInput` gesture detector turns touches into `Move` deltas with acceleration `v' = v · (1 + k·|v|)` and adjustable `k` and sensitivity. Tap → left click. Two-finger tap or 500 ms long press → right click. Two-finger pan → `Scroll`. Tap-then-hold → button down, move, then button up. Events go out as QUIC datagrams, grouped every 8 ms.

**Presenter (R9b):** buttons send `KeyEvent`s (Right/PageDown, Left/PageUp, `B`, `F5`, `Esc`). The volume keys are captured with `onKeyDown` while the screen is open. The laser pointer sends `Laser { x, y }` in normalized display coordinates at 60 Hz, using gyroscope data through `SensorManager` (game rotation vector) or touch. The desktop `Overlay` draws a click-through, always-on-top, transparent window: a layered window with `WS_EX_TRANSPARENT` on Windows, an `NSWindow` at `.screenSaver` level that ignores mouse events on macOS, and an X11 override-redirect window or Wayland layer-shell surface (wlroots-based compositors) on Linux. Where none of these work, it moves the system cursor instead. The timer runs only on the phone.

### 9.4 Media control (R11)

| Host | Implementation |
|---|---|
| Windows | `GlobalSystemMediaTransportControlsSessionManager` (WinRT, `windows` crate) |
| Linux | MPRIS over D-Bus (`zbus`) |
| macOS | Media keys sent through `NSEvent` system-defined events, and system volume through CoreAudio. Now-playing info is not shown, because the MediaRemote framework is private API (R11.3) |
| Android | `MediaSessionManager.getActiveSessions` with a `NotificationListenerService` (R11.4), and `AudioManager` for volume |

Messages: `MediaCmd { PlayPause | Next | Prev | VolSet(f32) | VolStep(i8) | Mute }` and `NowPlaying { title, artist, artwork_png ≤256px, state, position }`. The host pushes `NowPlaying` updates when they change.

## 10. Screen sharing (R12)

**Pipeline:** capture → color conversion (to NV12, on the GPU where possible) → H.264 encode (low-latency settings: no B-frames, a keyframe every 2 s or on request, CBR/VBV) → video stream → decode → render.

| Host | Capture | Encode |
|---|---|---|
| Windows | Windows.Graphics.Capture | Media Foundation hardware encoder → `openh264` |
| macOS | ScreenCaptureKit (needs Screen Recording permission) | VideoToolbox |
| Linux | xdg-desktop-portal ScreenCast → PipeWire (X11: XShm fallback) | VA-API → `openh264` |
| Android | MediaProjection (system consent every session) → `MediaCodec` input Surface | MediaCodec |

**Framing:** `VideoFrame { pts_us, keyframe, data }` on one QUIC unidirectional stream per session. This keeps the frames in order. To keep latency low, the **sender** drops frames when the stream's send buffer holds more than about 2 frames. When the receiver falls behind, it asks for a keyframe with `RequestKeyframe`.

**Adaptation (R12.4):** the sender reads quinn's RTT and congestion window every 500 ms. It moves down a ladder of bitrate × resolution steps (1080p30 at 8 Mbps → 720p30 at 4 Mbps → 540p24 at 2 Mbps) and moves back up after 5 s of stable conditions.

**Viewing:** on Android, `MediaCodec` decodes straight to a `SurfaceView`. On desktop, `openh264` (or a platform hardware decoder later) produces frames that are uploaded into a Slint `Image` through a `SharedPixelBuffer`. The UI shows a display picker (R12.6). When the viewer has input permission, clicks and keys in the view window are mapped to `Abs` pointer events on the selected display (R12.5).

**Privacy:** the host shows a visible "Screen is being shared with X" indicator and a Stop button (R5.4). On Android, the system's own MediaProjection indicator also appears.

## 11. Localhost sharing (R13)

```
Device B (viewer)                                   Device A (host)
browser → 127.0.0.1:3000 ─► B listener ─► QUIC stream ─► A: check port in shared_port ─► TCP 127.0.0.1:3000
```

- **Host A:** the user shares port P with a label and a list of allowed devices. A accepts `TunnelOpen { port }` only if P is shared with the requesting device (R13.5), and then connects only to `127.0.0.1:P` / `[::1]:P`. Remote devices can't choose the target host, so this can't be turned into an open proxy.
- **Viewer B:** when B starts using a share, it binds `127.0.0.1:P`, or the next free port if P is taken, which the UI shows. Each accepted TCP connection opens a new QUIC stream with `TunnelOpen` and then copies bytes in both directions (`tokio::io::copy_bidirectional`). This works for any TCP protocol, including HTTP/2, WebSockets, and hot-reload (R13.4).
- **Secure context (R13.3):** the browser on B sees `http://localhost:P`, which browsers treat as a secure context.
- **Android viewer:** the core binds the loopback listener inside the app's foreground service, so Chrome on the phone can open `http://localhost:P`.
- **LAN exposure (R13.6):** an optional setting binds `0.0.0.0:P` on B instead, after a warning. It turns off after 1 h (`lan_expose_until`).
- **Stats (R13.7):** each tunnel counts active connections and bytes moved and reports them to the UI.
- **Known issue:** dev servers that check the `Host` header (for example Vite's `server.allowedHosts`) only need changes when LAN exposure is on.

## 12. Desktop app (R14)

- **Slint UI** (`.slint` files in `crates/urdevice-desktop/ui/`): Devices list, Device detail (feature buttons, permissions), Pair (QR display via the `qrcode` crate, code confirm), Transfers, Remote view (screen plus input capture), Localhost shares, Settings, Activity log.
- **Tray and menu bar:** `tray-icon`. Start at login: `auto-launch`.
- **Threads:** the Slint event loop runs on the main thread, and the core runs on a tokio runtime in background threads. They exchange messages through `slint::invoke_from_event_loop` and `tokio::sync::mpsc`.
- **Packaging (R14.3):** `cargo-packager` or `cargo-bundle` plus platform signing. CI also handles notarization (macOS) and Authenticode signing (Windows).

## 13. Android app (R15)

- **Structure:** `MainActivity` (Compose, Navigation) and `UrDeviceService` (foreground service, `connectedDevice` type; adds the `mediaProjection` type while sharing the screen). The core runs inside the service.
- **Screens:** Devices, Pair (CameraX + ML Kit barcode scanning, plus the app's own QR for desktop scanning), Mousepad, Presenter, Keyboard, Media remote, Screen viewer, Send files, Localhost.
- **Platform services:** `UrDeviceAccessibilityService` (input when the phone is the host), `UrDeviceIme` (text input), `MediaListenerService` (notification listener), a Quick Settings `TileService`, and the share-sheet target `ACTION_SEND` / `ACTION_SEND_MULTIPLE`.
- **Permissions asked when first needed (R15.2):** camera (QR), notifications, nearby devices/Bluetooth, media read access, and the special accessibility, IME, and notification-listener settings. Each is introduced by an explanation screen.
- **Battery (R15.4):** discovery stops when the app is in the background and all paired devices are connected; QUIC keep-alive is sent every 25 s while idle.
- **Google Play policy:** using an AccessibilityService for remote control has to be declared in Play Console, with a clear in-app disclosure. This is a release risk tracked in tasks (T0.4).

## 14. Error handling

| Situation | Behavior |
|---|---|
| Unknown certificate outside pairing | TLS handshake fails; record the event; no UI prompt (prevents spam) |
| Unsupported protocol version | Close with an application error code; UI says "Update UrDevice on <device>" |
| Malformed or oversized message | Close the session, record the event, reconnect with backoff; repeated violations (3 in 10 min) pause auto-connect to that peer |
| Missing OS permission | `PermStatus::Missing(kind)` → UI explanation plus a button that opens settings |
| Hash mismatch during transfer | Delete the part file, retry that file once, then report a failure |
| Port conflict on the viewer | Bind the next free port and show it |
| Permission prompt times out | Deny; record in the activity log |

Errors in the core use `thiserror` enums. They cross UniFFI as typed Kotlin exceptions.

## 15. Security summary

The full threat model goes in `docs/security/threat-model.md` (task T1.9). Summary:

| Threat | Mitigation |
|---|---|
| Someone on the same Wi-Fi tries to connect | Mutual TLS with pinned keys; pairing only while the pairing screen is open; lockout |
| MITM during pairing | QR: fingerprint pinning plus token HMAC over channel binding. Discovery: 6-digit code shown on both screens |
| Spoofed discovery announcements | Discovery only suggests addresses; identity is checked in TLS |
| Stolen or lost device | Unpair it from any other device; Requirement 5 prompts for sensitive capabilities; the identity key is in secure storage and protected by the OS lock screen |
| Malicious peer sends crafted messages | Size limits, protobuf validation, fuzzed parsers, path cleaning, no remote control over tunnel targets |
| Silent surveillance through screen or input | Prompt per session by default, a persistent indicator, a one-click Stop, and an activity log |
| Clipboard leaks passwords | Sensitive flags respected; global pause |
| Localhost tunnel used as a pivot | Allow-list per port and device; loopback targets only; no LAN exposure by default, and it expires when enabled |
| Logs leak data | Logging (`tracing`) redacts content; field allow-list |

The project also runs `cargo audit` and `cargo deny` in CI, uses only well-known crypto libraries and no custom primitives, and should get an external security review before 1.0.

## 16. Testing strategy

- **Unit tests:** protocol encode/decode, the permission engine, path cleaning, pairing state machines, and backoff timing.
- **Property tests (`proptest`):** path cleaning never leaves the incoming folder; encode→decode returns the same message; framing handles arbitrary splits.
- **Integration tests:** two or three in-process cores on loopback test pairing (both flows), wrong-fingerprint rejection, token replay, lockout, reconnect, file resume, and tunnel allow-listing.
- **Fuzzing (`cargo-fuzz`):** control-message parser, discovery packet parser, BLE reassembly, and QR payload parser.
- **Platform smoke tests:** a device matrix checklist (Windows 11, macOS 14/15, Ubuntu GNOME Wayland, KDE X11, Android 10/13/15) for each feature.
- **Performance checks:** input latency (R9.5), file throughput (R8.8), and screen latency (R12.4) measured with timestamps embedded in the stream.

## 17. Decisions

1. **Ports:** UDP 47810 for discovery and UDP 47811 for QUIC are final. If 47811 is taken, the app picks another port and advertises it.
2. **Wayland:** supported. The portals come first, with a `uinput` fallback for input injection (§9.3).
3. **Order of work:** project setup ([#1](https://github.com/ahmadridhoramadhan/UrDevice/issues/1)), then Android ↔ desktop ([#2](https://github.com/ahmadridhoramadhan/UrDevice/issues/2)), then desktop ↔ desktop, then the Release 2 features (§18).

## 18. Release 2 features

Each feature below is a core service with its own capability string, which is negotiated in the handshake (§6.3) and checked by the permission engine (§7.4). Section 18.N covers Requirement 16+N.

| Requirement | Capability | Default permission |
|---|---|---|
| R17 Webcam and microphone | `camera.v1`, `mic.v1` | Ask |
| R18 Notification mirroring | `notify.v1` | Ask |
| R19 Shared mouse and keyboard | `kvm.v1` | Ask (uses input permission) |
| R20 Second display | `display.v1` | Ask |
| R21 Remote terminal | `term.v1` | Deny, changed only on the host |
| R22 DevTools and ADB | `adb.v1` | Ask |
| R23 Port profiles | `tunnel.v1` (existing) | Follows localhost |
| R24 Request inspector | local only | Off |
| R25 Find my device | `ring.v1` | Allow |
| R26 File browser | `fs.browse.v1`, `fs.write.v1` | Ask / Deny |
| R27 Clipboard history | `cliphist.v1` | Follows clipboard |
| R28 Proximity lock | `lock.v1` | Allow |
| R29 SMS and calls | `telephony.v1` | Ask |
| R30 Automations | local only | n/a |

### 18.1 Phone as webcam and microphone (R17)

**Pipeline:** CameraX on Android → `MediaCodec` H.264 (same low-latency settings as §10) → QUIC video stream → desktop decoder → virtual camera. Camera controls (switch, flash, zoom) are control-stream messages. Audio: `AudioRecord` → `MediaCodec` Opus encoder (API 29+) → QUIC stream → Opus decoder on the desktop.

| Desktop | Virtual camera | Virtual microphone |
|---|---|---|
| macOS 13+ | CoreMediaIO Camera Extension, packaged as a system extension inside the app. The user approves it once in System Settings, and the app must be in `/Applications` | Needs an Audio Server plug-in (HAL driver). Deferred |
| Windows 11 | `MFCreateVirtualCamera` with a Media Foundation media source registered by the app. No kernel driver is needed | Needs a virtual audio driver. Deferred |
| Windows 10 | Not supported. The app shows a viewer window only (R17.3) | Deferred |
| Linux | `v4l2loopback` device, filled through V4L2 output. The app checks whether the module is loaded and explains how to install it | PipeWire virtual source (`pipewire-rs`) |

The camera service runs in the foreground service with the `camera` and `microphone` service types.

### 18.2 Notification mirroring (R18)

- **Android:** the existing `MediaListenerService` (a `NotificationListenerService`) also handles `onNotificationPosted` and `onNotificationRemoved`. It sends `Notif { key, app, title, text, icon_png ≤64px, actions[], reply_capable, visibility }`. Filtering by visibility and app happens on the phone before sending (R18.2, R18.3).
- **Desktop:** native notifications through `notify-rust` (Linux, macOS) and WinRT toast notifications (Windows).
- **Reply (R18.4):** the desktop sends `NotifAction { key, action_index, reply_text? }`. The phone fills the `RemoteInput` results and fires the action's `PendingIntent`.
- **Dismissal sync (R18.5):** `NotifDismiss { key }` in both directions. The phone calls `cancelNotification(key)`.
- **Retention (R18.6):** the desktop keeps notifications in memory only and drops them after dismissal or 24 h.

### 18.3 Shared mouse and keyboard across desktops (R19)

- **Layout:** each desktop shares its displays (size, scale, position). The layout editor stores edge links (`device A, display 1, right edge → device B, display 2, left edge`) on every desktop in the layout.
- **Edge handoff:** the desktop with the physical mouse watches the cursor position. When the cursor crosses a linked edge, it starts capturing input, hides its own cursor, and sends `KvmEnter { display, y_ratio }` followed by input datagrams (§6.2). The remote desktop sends `KvmLeave` when the cursor crosses back.
- **Input capture:**

| Desktop | Capture API | Notes |
|---|---|---|
| Windows | `SetWindowsHookEx` with `WH_MOUSE_LL` and `WH_KEYBOARD_LL` | Can't capture input aimed at elevated windows unless UrDevice is elevated |
| macOS | `CGEventTap` | Needs Input Monitoring and Accessibility permissions |
| X11 | XInput2 raw events plus a pointer grab | |
| Wayland | `org.freedesktop.portal.InputCapture` with libei | Needs a desktop that implements the portal (GNOME 45+). Checked in spike T10.1; desktops without it can be controlled but can't share their own mouse |

- **Clipboard:** on handoff, the sending desktop pushes its current clipboard (R19.5).
- **Hotkeys (R19.4):** handled in the capture layer before input is forwarded, so they always work.

### 18.4 Second display (R20)

| Desktop | Virtual display | Risk |
|---|---|---|
| Linux (GNOME, KDE) | ScreenCast portal with `SourceType` virtual. The compositor creates a monitor and gives back a PipeWire stream | Low |
| Windows | An Indirect Display Driver (IddCx, user-mode). The driver must be signed through Microsoft's attestation signing, which needs an EV code-signing certificate | Medium: signing cost and process |
| macOS | `CGVirtualDisplay`, a private API that other display apps use | High: can break in any macOS update. Ships behind a feature flag |

The stream uses the §10 pipeline at the Android device's native resolution. Touch and stylus events map to `Abs` pointer and pen events on the virtual display.

### 18.5 Remote terminal (R21)

- **Host:** `portable-pty` (ConPTY on Windows, `openpty` on macOS and Linux) starts the user's default shell without elevation. Terminals run on desktop hosts only.
- **Protocol:** `TermOpen { cols, rows }` → one bidirectional stream carries raw bytes in both directions, and `TermResize { cols, rows }` goes on the control stream.
- **Viewer:** on desktop, `alacritty_terminal` parses the escape codes and a Slint grid draws the result. On Android, a VT parser in the core (`vt100`) feeds a Compose grid view with an extra-keys row.
- **Permission (R21.2):** the `term.v1` setting is stored with a flag that only the local UI can change. Remote settings requests for this capability are rejected.

### 18.6 Remote DevTools and ADB (R22)

1. The user turns on Wireless debugging on Android 11+. The phone app finds its own `adbd` ports through NsdManager (`_adb-tls-pairing._tcp`, `_adb-tls-connect._tcp`).
2. The desktop opens a tunnel (§11) whose target is the phone's `adbd` port, and runs `adb pair 127.0.0.1:<local>` with the pairing code the user types, then `adb connect`.
3. The desktop lists tabs with `adb forward tcp:<port> localabstract:chrome_devtools_remote` and the `/json` endpoint, and opens DevTools for the selected tab (or links to `chrome://inspect`).

The tunnel targets the phone's own loopback only, as in §11. UrDevice uses the `adb` binary from the user's Android SDK, or asks where it is.

### 18.7 Port forwarding profiles (R23)

- New trust-store table: `port_profile(id, name, host_device, host_ports JSON, viewers JSON, local_ports JSON, enabled)`.
- **Auto-start:** when every device in an enabled profile is connected, the viewers start their listeners (§11).
- **Host-initiated forwards (R23.3):** the host sends `ForwardOffer { ports }`. The viewer asks the user once and then stores the profile.

### 18.8 HTTP request inspector (R24)

- The viewer side copies tunnel bytes through a read-only parser and never changes them. The first bytes decide the mode: `0x16` means TLS and the HTTP/2 preface means HTTP/2. Both are shown as raw TCP (R24.6). Otherwise `httparse` parses HTTP/1.1, and WebSocket frames are recorded after an upgrade.
- Records go into an in-memory ring buffer capped at 50 MB per inspector. Values of sensitive headers are hidden in the UI unless the user shows them (R24.3).
- **Resend:** a request is rebuilt from the record and sent through a new tunnel stream.

### 18.9 Find my device (R25)

- **Messages:** `Ring { start | stop }`.
- **Android:** a full-screen notification and `MediaPlayer` with `USAGE_ALARM`. The alarm volume is raised for the duration of the ring and then restored. Do Not Disturb bypass needs notification-policy access. Android 14 limits full-screen intents, so the app falls back to a high-priority notification when it isn't allowed.
- **Desktop:** plays a sound at a raised volume and shows a window on top of other windows.
- `last_seen` and the network name come from the trust store.

### 18.10 Remote file browser and search (R26)

- **Protocol:** `FsList { path, cursor }`, `FsStat { path }`, `FsSearch { root, query }`, and `FsThumb { path }`, with results streamed back. Write operations (`FsPut`, `FsRename`, `FsDelete`) need `fs.write.v1`.
- **Desktop:** directories are accessed through `cap-std`, which opens paths relative to a shared root and blocks `..` and symbolic links that point outside it (R26.2). Search walks the tree with `walkdir`, stopping at 10,000 results and skipping hidden folders unless asked.
- **Android:** a Kotlin adapter uses the Storage Access Framework tree URIs that the user picked (`ACTION_OPEN_DOCUMENT_TREE`). It does not use the restricted `MANAGE_EXTERNAL_STORAGE` permission.
- Downloads and uploads reuse the §9.2 transfer code.

### 18.11 Shared clipboard history (R27)

- **Storage:** trust-store table `clip_history(id UUID, origin, ts, mime, ciphertext, pinned, expires_at)`. Each item is encrypted with AES-256-GCM, using a history key kept in secure storage.
- **Sync:** devices exchange item IDs on connect and send the missing items. Deletions and "Clear all" spread as tombstones that are kept for 24 h. Pin changes use the newest timestamp.
- Sensitive items (R7.3) never enter the history. Expired items are pruned every hour.

### 18.12 Proximity lock (R28)

- **Distance:** the desktop, acting as the BLE central (§8.2), reads the phone's signal strength (RSSI) once a second and averages it over 10 s. The user sets the threshold with a "walk away" calibration step. A lost connection also counts as out of range.
- **Lock APIs:** `LockWorkStation` on Windows. `loginctl lock-session` or `org.freedesktop.ScreenSaver.Lock` on Linux. On macOS there is no public lock API, so the app puts the display to sleep and tells the user to set "Require password" to "Immediately". An opt-in setting uses the private `SACLockScreenImmediate` call instead.
- **Exceptions (R28.4):** the lock is skipped while the presenter or screen sharing is active.
- **Battery:** BLE advertising on the phone runs only while proximity lock is on.

### 18.13 SMS and calls (R29)

- **Android build flavors:** `play` and `full`. Only `full` asks for `READ_SMS`, `SEND_SMS`, `RECEIVE_SMS`, `READ_PHONE_STATE`, and `ANSWER_PHONE_CALLS`.
- **SMS:** reads conversations from the `Telephony.Sms` provider, listens for incoming texts with `SMS_RECEIVED`, and sends with `SmsManager`.
- **Calls:** call state through `TelephonyCallback` (API 31+) or `PhoneStateListener`. Answer and decline through `TelecomManager.acceptRingingCall()` and `endCall()`, and silence through `silenceRinger()`.
- **Desktop cache:** encrypted the same way as the clipboard history (§18.11), and deleted on unpair (R29.4).

### 18.14 Automations (R30)

- **Rule engine in the core:** `Rule { trigger, conditions[], actions[] }` runs on events from the core's event bus.
- **Trigger sources:** session events. Wi-Fi network changes (on Android this needs the location permission). Process starts on desktop (`sysinfo` polled every 2 s). Timers and battery level.
- **Remote actions** are sent as normal requests, so the receiving device's permission checks apply (R30.4).
- **"Run command"** goes through `std::process::Command`, without a shell, using an argument list the user entered. It is off until enabled in settings (R30.5).
- **Android background limits (R30.6):** Android doesn't let apps open screens from the background. Actions like "open Presenter" become a notification that opens the screen when tapped.

### 18.15 Release 2 risks

| Risk | Affects | Plan |
|---|---|---|
| macOS private APIs (`CGVirtualDisplay`, `SACLockScreenImmediate`) | R20, R28 | Feature flags, fallbacks, retest on every macOS release |
| Windows driver signing (IddCx, virtual audio) | R17.4, R20 | Get an EV certificate. Virtual audio waits until the IddCx process is in place |
| Wayland InputCapture portal not available on all desktops | R19 | Spike T10.1 and document which desktops work |
| Google Play policy (SMS, call permissions, full-screen intents) | R25, R29 | Separate `full` build, with fallbacks in the Play build |
| New attack surface (terminal, file browser, run command) | R21, R26, R30 | Deny by default, settings changeable only on the device itself, `cap-std` sandboxing, fuzzing, and a second external security review before Release 2 |
