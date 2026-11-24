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
├─ .kiro/specs/urdevice/      # this spec (requirements, design, tasks)
└─ docs/
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
- **BLE static key:** an X25519 key pair used for the Noise fallback channel (§8.2). It is derived from the identity key (HKDF-SHA256, salt `urdevice-ble-static-v1`), so it needs no storage and changes when the identity is reset. Its public half travels in `Hello` and `Welcome` (`ble_static`) over the mutually authenticated, pinned QUIC session, so the identity key vouches for it through TLS and no separate signature is needed; the first session runs right after pairing. The receiver pins the first key it gets in `ble_static_pub` (key plus its own HMAC, so older rows keep their row MAC) and never replaces it; Forget deletes it with the row.

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
  auto_connect     INTEGER NOT NULL DEFAULT 1,  -- 0 after the user chose Disconnect (R4.7)
  row_mac          BLOB NOT NULL
)
permission(capability PRIMARY KEY, mode /* allow|ask|deny */, local_only INTEGER)  -- one row per capability, applies to every paired device (R5.1)
activity_log(id, device_id, capability, decision, ts)   -- pruned after 30 days (R5.6)
shared_port(port PRIMARY KEY, label, lan_expose_until INTEGER NULL)  -- NULL = off, 0 = always on, otherwise a Unix time (R13.6)
notif_subscription(host_device_id, app_id, PRIMARY KEY(host_device_id, app_id))  -- apps this device receives from each host (R18.2)
```

The `permission` table is filled during onboarding (R5.2), so it never has a missing row for a Release 1 capability. `local_only` marks settings that only the local UI may change (R21.2).

## 5. Discovery (R2)

- **mDNS:** service `_urdevice._udp.local.` with instance name `<display name>`. TXT records: `v=<proto>`, `id=<short id hex>`, `p=<platform>`. The SRV record carries the QUIC port.
- **UDP broadcast:** sent to `255.255.255.255:47810` and to each interface's directed broadcast address every 5 s. The payload is a protobuf `Announce { v, short_id, name, port, platform }` with a 4-byte magic `URDV` in front and a size limit of 512 bytes.
- **Receiving:** packets are parsed with a hard size limit and rate-limited per source with a token bucket (R2.6). Results go into a `PeerTable` keyed by short ID. If a short ID matches a paired device, the core connects and then checks the full fingerprint during TLS, so a forged announcement can't do more than cause a failed connection attempt.
- **Android:** the app holds a `WifiManager.MulticastLock` only while the app is in the foreground or a session is being looked for, to save battery (R15.4).
- **Hidden mode (R2.7):** stops sending announcements. Known peers can still be reached at their `last_addrs`.
- **Disconnected devices (R2.4, R4.7):** the core does not auto-connect to a paired device whose `auto_connect` is 0.

Discovery data is unauthenticated by design. It only tells devices where to try connecting. All trust decisions happen in TLS.

## 6. Transport and session (R4)

### 6.1 Connection

- Each device listens on one UDP port (default 47811; it picks another if that port is taken) for QUIC.
- ALPN: `urdevice/1`.
- **Mutual TLS 1.3** with custom `rustls` verifiers on both sides:
  - `ServerCertVerifier` / `ClientCertVerifier` accept a certificate only if its fingerprint is in the trust store, **or** a pairing context is active and the fingerprint matches what that context expects (QR flow) or the context is in confirm-code mode.
  - TLS signatures are still checked with the certificate's key, so the peer must hold the private key.
- When both sides find each other at the same moment and two connections open, the connection started by the device with the lower Device ID wins.
- A new connection from a peer replaces a session that is older than 5 s, because the peer only dials when it lost its session or found a better path (§8.1). The replacement keeps the running activities. An IP connection always replaces a Bluetooth LE session, and a Bluetooth LE session replaces a QUIC session only while that one has stopped answering.

### 6.2 Streams

| Stream | Type | Use |
|---|---|---|
| Control | bidirectional stream 0 | Handshake, capabilities, permission requests, clipboard, input, media, presenter commands |
| Bulk | one bidirectional stream per transfer | Files, gallery thumbnails and downloads |
| Video | unidirectional stream per screen session, plus QUIC datagrams for input | Screen sharing (§10) |
| Tunnel | one bidirectional stream per TCP connection | Localhost sharing (§11) |

Control messages are length-prefixed protobuf (`u32` length, max 1 MiB; anything larger is rejected and closes the session, R4.6). Input events use **QUIC datagrams** when the peer supports them, so a lost packet never blocks newer input.

**Feature traffic.** Every feature is a service with a numeric service id (§9). Features never add messages to the core `Control` envelope (its tags 1–63 belong to the core). They use three channels, each tagged with the service id, a direction, and an activity id:

| Channel | Framing | Limit |
|---|---|---|
| Control | `Feature { service, direction, activity, body }` in `Control`; `body` is the feature's own protobuf message. The receiver may answer `FeatureRefused { service, activity, reason }` | 1 MiB frame |
| Stream | Every bidirectional or unidirectional stream other than stream 0 starts with `[u32 length][StreamOpen { service, direction, activity, kind, meta }]`; the feature's bytes follow. `kind` is defined by the feature | 16 KiB header |
| Datagram | `[u16 service][u8 direction][u64 activity][payload]`, big-endian | path MTU |

`direction` is `ToHost` (to the device that offers the capability, checked by its permission engine, §7.4) or `ToController` (replies and pushed data to the device that uses it; not checked, so the receiver only shows it). `activity` is the `request_id` of a granted `ActivityRequest`, or 0. Message field numbers and stream kinds are local to each feature, so the only numbers features share are the service ids:

| Ids | Use |
|---|---|
| 1–8 | Release 1: clipboard 1, files 2, gallery 3, input 4, media 5, screen 6, tunnel 7, Bluetooth fallback 8 |
| 9–15 | Reserved for Release 1 |
| 16–32 | Release 2: notify 16, ring 17, camera 18, mic 19, telephony 20, kvm 21, display 22, terminal 23, port profiles 24, inspector 25 (reserved, local only), adb 26, fs.browse 27, fs.write 28, clipboard history 29, proximity lock 30, automations 31 (reserved), speaker 32 |
| 33–63 | Reserved for Release 2 |
| 0xFF00–0xFFFF | In-process tests only |

The table with owners is in [`docs/dev/adding-a-feature.md`](../../../docs/dev/adding-a-feature.md); the ids are in `urdevice-proto/src/services.rs`.

### 6.3 Handshake

```
Client → Server: Hello { proto_min, proto_max, app_version, capabilities[], name, platform, ble_static }
Server → Client: Welcome { proto, capabilities[], session_id, name, platform, ble_static }
```

Capabilities are strings such as `clipboard.v1`, `files.v1`, `input.v1`, `screen.h264.v1`, `tunnel.v1`, `media.v1`, `ble.fallback.v1`. Each side lists the capabilities it offers as a host, so a host-only capability such as `gallery.v1` appears only in the Android side's list. A device accepts feature traffic for a capability only if it lists that capability itself, and accepts replies only for capabilities the peer listed. Local permissions (§7) are then checked for each request.

Right after `Welcome`, and again whenever something changes, each side sends:

```
PermissionState { entries[{ capability, mode: allow|ask|deny }] }   // within 1 s of a change (R5.7)
DeviceStatus    { battery_pct?, charging? }                          // omitted on devices without a battery (R4.9)
```

The peer uses `PermissionState` only to draw its UI: features set to `deny` appear turned off, and features set to `ask` show that the host will ask. The host still checks every request with its own engine (§7.4).

### 6.4 Reconnect (R4.5)

When a session drops, the core tries `last_addrs`, any newly discovered addresses, and then fallback paths, waiting 1, 2, 4, … up to 30 s between attempts. QUIC connection migration covers address changes such as Wi-Fi to PAN while the connection is still alive. Without it, a fresh handshake runs with the pinned keys and needs no user action.

### 6.5 Disconnect and reconnect by the user (R4.7)

- **Disconnect:** the device sends `Disconnect {}` on the control stream, sets `auto_connect = 0`, and closes the connection. The peer that receives `Disconnect` also sets `auto_connect = 0` and shows the device as "Disconnected by <name>". Neither side reconnects on its own.
- **Reconnect:** the user taps "Reconnect" on either device. That device sets `auto_connect = 1` and connects. The peer accepts the connection because the device is still paired, and sets `auto_connect = 1` again.

### 6.6 Connection path and latency (R4.8)

The UI shows the path the session uses (Wi-Fi/LAN, Bluetooth PAN, or Bluetooth LE, §8) and the round-trip time. On QUIC paths the RTT comes from quinn's `Connection::rtt()`, read every 2 s. On the BLE channel it comes from the keep-alive round trip.

## 7. Pairing and permissions

### 7.1 QR pairing (R3.1–R3.4)

QR payload: a URI `urdevice://pair?d=<base64url(protobuf)>` with

```
PairInvite { v, device_id[32], name, platform, addrs[], port, token[16], expires_at }
```

Flow:

1. The host (A) creates a 16-byte token from the OS CSPRNG, opens a pairing context that expects *any* client certificate, and shows the QR code.
2. The scanner (B) connects to `addrs`. B's verifier accepts A's certificate **only if** its fingerprint equals `device_id` from the QR code. This authenticates A.
3. B computes `binding = TLS-Exporter("EXPORTER-urdevice-pair", 32)` and sends `PairProof { hmac = HMAC-SHA256(token, binding ‖ fp_A ‖ fp_B), name, platform }`. The BLE static keys follow in the first session's `Hello` and `Welcome` (§4.1).
4. A recomputes the HMAC and compares it in constant time. If it matches, A stores B and replies `PairAccept { name, platform }`. B then stores A.
5. The token is destroyed after one attempt, successful or not (R3.2).

The channel binding ties the proof to this specific TLS session, so a captured proof can't be replayed or relayed.

Either device can be the scanner (B): Android, or a desktop with a camera (§7.5, R3.10). The QR flow needs no code confirmation, because the fingerprint pinning and the token proof already authenticate both sides.

### 7.2 Code-confirmation pairing (R3.5)

This flow is used for devices found by discovery. The user selects a device in the "available to pair" list on the Pair screen. Both pairing contexts accept any certificate. After TLS completes, each side computes `code = u32(TLS-Exporter("EXPORTER-urdevice-sas", 4)) mod 10^6` and shows it under that device, with "Codes match" and "Cancel" buttons. The pair is stored only after **both** users tap "Codes match". The code is compared by eye on the two screens, never typed in. A man-in-the-middle would end up with two different TLS sessions and two different codes. The chance of guessing a matching code is 1 in 10⁶ per attempt, and the lockout (R3.7) makes repeated guessing impractical.

### 7.3 Pairing limits

- QR pairing contexts exist only while the pairing screen is open (R3.8), and each lasts at most 120 s. A code pairing asked for while the screen is closed first shows an Accept/Decline prompt (the Always Ask prompt with capability `pair`); Accept opens the screen and the code flow continues. Unknown devices can then connect (unless hidden or locked) but can only send `PairStart` or `PairProof`. The questions are rate-limited (one at a time, 5 a minute, 60 s of quiet after a decline or timeout). When a QR token expires while the screen is still open, the core creates a new token and the UI shows the new QR code with a countdown (R3.11).
- Failure counter: 5 failures within a 5-minute window locks pairing for 5 minutes (R3.7).

### 7.4 Permission engine (R5)

`PermissionEngine::check(requester, capability) -> Allow | Deny | Ask(Future<Decision>)`. The setting is looked up by capability only. It is the same for every paired device (R5.1). `requester` is used for the prompt text and the activity log.

Settings, UI labels, and onboarding defaults (R5.2):

| Setting (UI label) | Capability | Default |
|---|---|---|
| Clipboard | `clipboard.v1` | `allow` |
| Receive files | `files.v1` | `ask` |
| Gallery (Android hosts only) | `gallery.v1` | `ask` |
| Remote control (pointer, keyboard, presenter) | `input.v1` | `ask` |
| Media | `media.v1` | `allow` |
| Screen sharing | `screen.h264.v1` | `ask` |
| Localhost | `tunnel.v1` | `deny` |

The UI shows the modes as **Allow**, **Always Ask**, and **Deny**.

- **Onboarding:** on first start, the app shows the table above with its defaults and saves the user's choices. The user can change any of them later in Settings. There is no separate "default permission" setting.
- **Allow:** the activity starts at once.
- **Always Ask (`ask`):** the engine sends a prompt to the UI through the `UiDelegate` trait (a prompt window on desktop, a notification with Allow and Deny on Android). If the user doesn't answer within 30 s, the request is denied. Each approval applies to one *activity* (one transfer, one screen session, one remote-control session).
- **Deny:** the request is refused with `Denied { capability }`. Peers already show the feature as off, because the host sends `PermissionState` (§6.3).
- **Changes:** a new setting is sent to every connected peer within 1 s (R5.7). Changing a setting to Deny also revokes running activities of that capability (R5.5).
- **Terminal and file changes:** `term.v1` and `fs.write.v1` default to `deny`. Switching either one to `allow` shows a warning that other devices get full control of this device's shell or files, and needs one extra confirmation (R5.2). Both are `local_only` (§4.2).
- Active input, screen, and audio activities register with an `ActivityMonitor`. It drives the on-screen indicator and the Stop button. Calling `revoke()` cancels the activity's tokio tasks and closes its streams. The target is under 1 s, and in practice it is immediate (R5.4, R5.5).

### 7.5 Scanning QR codes on desktop (R3.10)

- **Camera:** the `nokhwa` crate reads frames through Media Foundation (Windows), AVFoundation (macOS), and V4L2 (Linux). The `rqrr` crate decodes QR codes from those frames. After a code is read, the flow continues at step 2 of §7.1.
- **OS permission:** macOS asks for camera access the first time (the app declares `NSCameraUsageDescription`). On Windows, if desktop apps are blocked from the camera, the app explains this and opens `ms-settings:privacy-webcam`. On Linux the user needs access to `/dev/video*`; inside Flatpak the app uses the Camera portal.
- **No camera:** the Pair screen hides "Scan QR". Showing this device's QR code and code confirmation still work.
- **Privacy:** frames stay in memory, are never written or sent, and the camera stops as soon as a code is read or the Pair screen closes.

## 8. Bluetooth fallback (R6)

### 8.1 Bluetooth PAN (IP path)

Bluetooth PAN gives both devices an ordinary IP network, so no protocol changes are needed. Once a second the core reads its interfaces (`if-addrs`; no extra watcher dependency) and each session's traffic:

- **Path label (R4.8, R6.4):** Bluetooth PAN when the session's local address is on a Bluetooth interface: kernel device type `bluetooth` on Linux (NetworkManager names the interface `enx<MAC>`, spike T0.5), `bnep*`, Android's `bt-pan`, or a Windows adapter named "Bluetooth Network Connection". Otherwise Wi-Fi/LAN.
- **Loss (R6.1):** a session that received nothing for 1 s gets an empty probe datagram (service `BLUETOOTH`), which the peer drops but acknowledges. After 3.5 s without a packet, or at once when the session's local address disappears, the core dials the peer's other known addresses (discovery and last seen), for example its PAN address. QUIC migration does not help here because the server's address changes too, so the new connection replaces the session in place (§6.1) and the activities carry on. Before this, a dead path was noticed only by the 30 s idle timeout.
- **Return (R6.5):** a session on PAN, or on the Bluetooth LE channel, dials the peer's LAN addresses as soon as discovery announces them again (every 5 s), at most every 4 s, so it moves back within 10 s.
- The app never makes PAN the default route; the OS keeps routing as the user set it up (spike T0.5).

Platform reality:

| Pair | PAN available? |
|---|---|
| Android (tethering on) ↔ Windows / Linux | Yes. Android acts as the network access point. The user turns on Bluetooth tethering; the app opens the settings screen (R6.6) |
| Android ↔ macOS | **No.** macOS removed Bluetooth PAN in Monterey ([Apple Community](https://discussions.apple.com/thread/253312557)), so the BLE channel is used instead |
| Desktop ↔ desktop | Out of scope for v1 |

### 8.2 BLE control channel

- Android runs a GATT server with a UrDevice service UUID and one characteristic for each direction (write-without-response and notify). The desktop is the GATT central: `btleplug` on Windows, Linux, and macOS.
- Security: **Noise IK** (`snow`, `Noise_IK_25519_ChaChaPoly_SHA256`, prologue `urdevice-ble-v1`) using the pinned BLE static keys (§4.1). The desktop is the initiator and knows the phone's key; the phone answers only when the initiator's static key is pinned for a paired device. It does not depend on Bluetooth-level pairing security, and there is no unencrypted mode.
- GATT: service `6b1f0001-5d7a-4c1e-9b8a-2f6d1c3e8a10`, desktop-to-phone characteristic `…0002` (write without response), phone-to-desktop characteristic `…0003` (notify). The phone advertises only while a paired desktop has no working session (R15.4). Android asks for "Nearby devices" (`BLUETOOTH_CONNECT`, `BLUETOOTH_ADVERTISE`) from the device view.
- When: the desktop tries the channel for a paired phone with auto-connect on when there is no session and no discovery announcement in the last 8 s, or when its QUIC session has stopped answering; retries back off from 4 s to 60 s.
- Framing: Noise messages (≤ 65 535 bytes) split to fit the negotiated MTU. Each packet has a 2-byte sequence number and a flags byte (bit 0: last packet). The receiver accepts packets only in order and holds at most 65 535 bytes; anything else closes the link. Inside a Noise message, a type byte: control-message piece (control messages are cut into 1 KiB pieces, at most 80 KiB in all), datagram (≤ 256 bytes), ping, or pong. Pings go every 2 s and give the RTT; 8 s without a packet ends the link. On top runs the normal session: `Hello`, `Welcome`, permissions, activities, and feature messages.
- Allowed messages: input, presenter, media, keyboard, and clipboard text up to 64 KB (R6.2). Everything else returns `CoreError::FallbackPath` to the UI (R6.4): other services' messages and datagrams, all streams, and clipboard images. A peer that sends them anyway gets `FeatureRefused { reason: "fallback_path" }`.
- **UI on the BLE path (R6.4):** the device view shows a banner under the device header, for example "Connected over Bluetooth LE. Screen sharing, file transfers, and localhost are unavailable until Wi-Fi returns." The affected feature tiles turn grey with the reason "Not available over Bluetooth". The banner and the grey tiles go away when the session moves back to the LAN (R6.5). On the PAN path there is no banner, because every feature still works; only the connection chip changes to Bluetooth (R4.8).
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

**Service mechanism.** Each feature is one module in `urdevice-core/src/services/` that implements the `Service` trait (service id, capability, and handlers for control messages, streams, datagrams, refusals, activity ends, and session changes) and adds itself to the node's `Registry` in its `register` function. The node routes feature traffic (§6.2) by service id. Before a service sees anything sent `ToHost`, the node runs the permission check with the host's own setting for the service's capability:

- Deny refuses it.
- Traffic that names an activity is accepted only if this host granted that activity to that peer for that capability and it has not ended.
- Traffic without an activity is accepted only when the setting is Allow and the capability is not watched. Watched capabilities (input, screen, terminal, camera, mic, kvm, display, speaker) always need an activity, so the indicator and Stop button show (R5.4); Always Ask always needs one, so the user approves each activity (§7.4).

The service receives a `Grant` that only the node can create. Services ask for activities with `ServiceCtx::request`, which sends an `ActivityRequest` and waits for the answer. When an activity ends (Stop, a change to Deny, or the session closing), the services of both devices are told, so they can cancel tasks and close streams within 1 s (R5.5). The traits above move into the service modules as each feature defines what it needs, with settable slots for the platform adapters and UI listeners. Each layer has one file per feature: `urdevice-proto/src/features/`, `urdevice-platform/src/features/`, `urdevice-ffi/src/features/`, `urdevice-desktop/src/features/` with `ui/features/`, and `android/.../ui/features/`.

**Release 2 capabilities** are defined from the start, but a build lists one in the handshake and the settings only after its module marks it shipped. On an install that was onboarded before, the new capability's setting is created once with its default; a default of Allow is stored as Always Ask on upgrade, so a capability is never turned on without the user.

### 9.1 Clipboard (R7)

- On desktop, the watcher uses OS change events where they exist (`AddClipboardFormatListener` on Windows, X11 XFixes) and polls `NSPasteboard.changeCount` every 500 ms on macOS. On Linux the X11 back-end also serves Wayland sessions through XWayland: GNOME's Mutter keeps both clipboards in sync and has no `wlr-data-control`. A native `wlr-data-control` / `ext-data-control` back-end for compositors without XWayland is not built yet.
- Sensitive content is never sent (R7.3). It is recognised by `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory = 0`, and `CanUploadToCloudClipboard = 0` on Windows, `org.nspasteboard.ConcealedType` and `TransientType` on macOS, `x-kde-passwordManagerHint` on X11, and `EXTRA_IS_SENSITIVE` on Android. A marked item is not read at all.
- Echo prevention: the core keeps the hash of the last written, last received, and last sent content and ignores watcher events that match it (R7.4). Text is hashed with line endings normalised, because Windows turns `\n` into `\r\n`. Back-ends also skip the change event of their own write where the OS tells them apart (X11 owner window, Windows sequence number, macOS change count).
- Message: `Clip { id, mime, bytes (≤10 MB), origin_device, alt_text }`. Items up to 512 KiB go in a control message. Larger items go over a unidirectional stream (`StreamOpen.kind = 1`, `meta = ClipHeader { id, mime, size, sha256 }`), which the receiver reads to exactly `size` bytes and checks against the hash before writing. Allowed types: `text/plain`, `text/html`, `image/png`. The receiver checks the type, the size, UTF-8, and the PNG header (at most 100 megapixels), drops what fails, and rate-limits each peer to 20 items per 10 s.
- Always Ask on the receiving device: the sender requests an activity, which covers one item, and stops it after sending. The sender asks again if its copy of the setting was stale (`ask` refusal).
- Settings (R7.6): "Pause clipboard on all devices" and a switch per paired device are stored with the other device settings (`svc1.paused`, `svc1.peer.<id>`). Both directions stop: nothing is sent and nothing received is written.
- Android: content is read only while the app is in the foreground or through a user action. While the app is open, a change of the system clipboard is sent on its own. In the background the user sends with the notification action ("Send clipboard" on the service notification), the Quick Settings tile, or the share sheet; the first two open an invisible activity that reads the clipboard once it has focus. Incoming content is written with `ClipboardManager.setPrimaryClip`, which Android allows from a foreground service (R7.5); images go through a `FileProvider` URI.
- Windows writes images as `PNG` and `CF_DIBV5` (Windows then offers `CF_DIB` and `CF_BITMAP`), and reads `PNG`, `CF_DIBV5`, or `CF_DIB`.

### 9.2 File transfer (R8)

```
Offer   { transfer_id, files[{rel_path, size}], total_size }           ToHost, after an activity
Accept  { transfer_id, resume[{index, offset}] } | Decline { reason }  ToController
Cancel  { transfer_id }                                                either way
Data stream (one per transfer, meta = transfer_id):
  sender → receiver   1 [index u32][offset u64][len u32][bytes]   (chunk = 1 MiB)
                      2 [index u32][sha256 32 bytes]              end of a file
  receiver → sender   1 [index u32][offset u64]                   bytes written (progress)
                      2 [index u32][status u8]                    1 verified, 0 hash failed (sent once more), 2 failed
```

The per-file SHA-256 travels at the end of each file on the data stream instead of a separate `Done` message, so the sender hashes while it reads and never reads a file twice. The receiver's verdicts on the same stream end the transfer.

- **Path safety (R8.6):** each `rel_path` is split into components. The receiver rejects `..`, absolute paths, drive letters, NUL bytes, and Windows reserved names (`CON`, `NUL`, …). It joins the clean path under the incoming folder, then canonicalizes and checks that the result is still inside that folder. Name collisions become `name (1).ext`.
- Data is written to `*.urdevice-part`, hashed while streaming, and renamed only after the hash matches (R8.4).
- **Resume (R8.5):** the receiver keeps a sidecar file (`*.urdevice-part.resume`: sender, transfer id, size, offset; synced every 16 MiB) and sends the offset back in `Accept.resume`. The sender hashes the bytes before the offset again, so the final check covers the whole file. When the session returns, or every 5 s while it is up, the sender offers the unverified files again under the same `transfer_id` (at most 20 times). The earlier activity has ended by then, so under Always Ask the receiver approves the resumed transfer again.
- **Limits:** at most 10 000 files and 1 TiB per file per offer, 8 incoming transfers at once, 32 folder levels and 200-byte names; an offer that breaks a limit is declined with `invalid`.
- **Download folder:** desktop `Downloads/UrDevice`, changeable in Settings (stored in the app data folder). Android receives into app-private storage and then publishes each verified file to `Download/UrDevice/<folder>` through MediaStore, which needs no storage permission and picks `name (1).ext` on a collision (R15.3).
- **Pickers:** desktop file and folder pickers use `rfd` (the XDG portal on Linux). Drag-and-drop uses winit's dropped-file events; winit has no drop support on Wayland, so there the pickers are the way.
- Progress is calculated from acknowledged bytes and passed to the UI about every 100 ms.
- **Throughput (R8.8, T3.10), measured 2026-11-20** on 5 GHz Wi-Fi between the Linux laptop (debug build of the core) and the vivo I2505 (Android 16): Linux → Android 26.0 MB/s (200 MB) and 24.7 MB/s (600 MB); Android → Linux 28.9 MB/s (433 MB video, share sheet), round trip 12–50 ms. That is above the 20 MB/s target with 1 MiB chunks and quinn's default flow-control windows, so neither was changed.
- **Gallery (R8.7):** the Android `Gallery` uses MediaStore (`READ_MEDIA_IMAGES` / `READ_MEDIA_VIDEO` on API 33+, `READ_EXTERNAL_STORAGE` on older versions; asked through a notification the first time a desktop browses). One activity covers one browsing session. Messages are `GalleryList { cursor, limit ≤ 60 }` → `GalleryPage { items[{id, name, mime, size, taken_ms, duration_ms, thumb}], next_cursor, error }` with 256-px JPEG thumbnails (≤ 64 KiB each, page under 1 MiB), then a download stream whose `meta` is `GalleryFetch { items[{id, offset, size}] }` (≤ 200 items). The phone answers on that stream with the file-transfer framing, so downloads get the same part files, SHA-256 check, resume, and transfer list. Item names keep only their last component.

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

**Wire and session (T4.1).** Remote control is a watched capability, so a controller first asks for an activity (`ctx.request`, Always Ask by default). The host then shows the indicator with Stop. Events travel in a `Batch` (max 64 events, field numbers in `proto/features/input.proto`). Events whose loss is harmless (`Move`, `Abs`, `Scroll`, `Laser`) go as datagrams, grouped every 8 ms, with consecutive moves and scrolls merged. Events whose loss would leave a button stuck or drop typed text (`Button`, `Key`, `Text`) go on the control stream instead, after the moves queued before them. The control stream is also the fallback when the path takes no datagrams. This refines the earlier plan to send everything as datagrams. `Key` carries HID usages and optional modifier bits; the host presses the modifiers around the key. `Hello` (controller to host) makes the host prepare its adapter and answer `Status` (ready, missing OS permission, or unavailable) so the phone can explain it (R9.4). The host clamps and rate limits every event, ignores anything without an activity, and releases every held key and button when the activity ends.

**Desktop host (T4.2).** Windows uses `SendInput`. macOS uses `CGEventPost` after an Accessibility check and prompt. X11 uses XTest, with text typed through a spare keycode. Wayland uses the RemoteDesktop portal with `persist_mode` 2; the restore token is a secret in the keystore and is deleted with Delete all data and Reset identity. Where the portal is missing, `uinput` is used, after a udev rule that the app installs through `pkexec`. The portal and `uinput` move the pointer by relative steps only. Absolute positions are not available there.

**Laser overlay (T4.5).** A transparent, frameless, always-on-top window covers the primary display and lets clicks through. Where no window can be made, the pointer is moved to the laser position instead. On Wayland a compositor may keep the window below a full-screen presentation, because there is no layer-shell here.

**Mousepad (R9a), on the Android side:** a Compose `pointerInput` gesture detector turns touches into `Move` deltas with acceleration `v' = v · (1 + k·|v|)` and adjustable `k` and sensitivity. Tap → left click. Two-finger tap or 500 ms long press → right click. Two-finger pan → `Scroll`. Tap-then-hold → button down, move, then button up. Events go out as QUIC datagrams, grouped every 8 ms.

**Presenter (R9b):** buttons send `KeyEvent`s (Right/PageDown, Left/PageUp, `B`, `F5`, `Esc`). The volume keys are captured with `onKeyDown` while the screen is open. The laser pointer sends `Laser { x, y }` in normalized display coordinates at 60 Hz, using gyroscope data through `SensorManager` (game rotation vector) or touch. The desktop `Overlay` draws a click-through, always-on-top, transparent window: a layered window with `WS_EX_TRANSPARENT` on Windows, an `NSWindow` at `.screenSaver` level that ignores mouse events on macOS, and an X11 override-redirect window or Wayland layer-shell surface (wlroots-based compositors) on Linux. Where none of these work, it moves the system cursor instead. The timer runs only on the phone.

### 9.4 Media control (R11)

| Host | Implementation |
|---|---|
| Windows | `GlobalSystemMediaTransportControlsSessionManager` (WinRT, `windows` crate) |
| Linux | MPRIS over D-Bus (`zbus`) |
| macOS | Media keys sent through `NSEvent` system-defined events, and system volume and mute through the system's `volume settings` (AppleScript), which also covers output devices without a master channel, where a raw CoreAudio scalar does not work. Now-playing info is not shown, because the MediaRemote framework is private API (R11.3) |
| Android | `MediaSessionManager.getActiveSessions` with a `NotificationListenerService` (R11.4), and `AudioManager` for volume |

Messages:

```
MediaSubscribe { on: bool }                         // controller → host (R11.6)
MediaState {                                        // host → controller, pushed on change while subscribed
  sessions[{ id, app_name, title?, artist?, artwork_png? ≤256px,
             state: playing|paused|stopped, position_ms?, duration_ms?,
             can_seek, can_next, can_prev }],
  volume: f32, muted: bool }
MediaCmd { session_id, cmd: PlayPause | Next | Prev | Seek(ms) }   // R11.1, R11.2
VolumeCmd { VolSet(f32) | VolStep(i8) | Mute(bool) }               // system output of the host
```

- **Sessions (R11.2):** one entry per media app. Windows lists `GetSessions()`, Linux lists each `org.mpris.MediaPlayer2.*` bus name, and Android lists `getActiveSessions`. The controller shows them as source tabs and sends commands with the selected `session_id`. Selecting a source only changes the controller's view, so no extra message is needed.
- **Capability flags (R11.5):** `can_seek` comes from SMTC `IsPlaybackPositionEnabled`, MPRIS `CanSeek`, and Android `PlaybackState.ACTION_SEEK_TO`, and the next/previous flags come from the matching properties. The controller hides controls whose flag is false. Shuffle and repeat are not part of the protocol.
- **Updates (R11.6):** a subscribed host reads its sessions once a second and pushes a `MediaState` only when something other than the playback clock changed (a position that stays within 2 s of where the clock predicts is not a change). The controller moves the position of a playing session forward between pushes. A host stops pushing when the controller turns the player off, when the session closes, when the host's setting becomes Deny, and when an Always Ask approval ends.
- **Limits:** a `MediaState` has at most 16 sessions, 96 KiB of artwork per session and 512 KiB in total, and the controller drops artwork that is not a PNG. Commands from the controller are applied in order, at most 64 queued.
- **Artwork:** hosts send PNGs of at most 256 px. Linux reads `file://` cover URLs only, and Android reads bitmaps and `content://` URIs only. A host never fetches a remote URL for a player, so remote-only covers (for example Spotify on Linux) show the app icon.
- **macOS (R11.3):** the host sends one session named "System" with no title or artwork, `can_seek = false`, and next and previous set to true. Its commands become media keys.
- **On and off (R11.6):** the controller sends `MediaSubscribe { on: false }` when the user turns the player off. The host then stops sending `MediaState` to that controller. Turning it on again sends `MediaSubscribe { on: true }`, and the host replies with the current state.

### 9.5 Battery status (R4.9)

| Device | Source |
|---|---|
| Windows, macOS, Linux | The `starship-battery` crate, which reads `GetSystemPowerStatus` / IOKit power sources / UPower and `/sys/class/power_supply` |
| Android | The sticky `ACTION_BATTERY_CHANGED` broadcast and `BatteryManager` |

The core sends `DeviceStatus` (§6.3) when the session starts and when the level changes by at least 1%, at most once a minute. Desktops without a battery leave the fields empty, and the peer's UI shows no battery.

## 10. Screen sharing (R12)

**Pipeline:** capture → color conversion (to NV12 or I420) → H.264 encode (low-latency settings: no B-frames, a keyframe every 2 s or on request, CBR) → video stream → decode → render.

| Host | Capture | Encode |
|---|---|---|
| Windows | Windows.Graphics.Capture, free-threaded frame pool, staging-texture copy | Media Foundation hardware encoder MFT (async) → `openh264` |
| macOS | ScreenCaptureKit (needs Screen Recording permission) | `openh264` (VideoToolbox later, see below) |
| Linux | xdg-desktop-portal ScreenCast → PipeWire (libpipewire loaded at run time); X11: `GetImage` per RandR monitor | `openh264` (VA-API later, see below) |
| Android | MediaProjection (system consent every session) in a `mediaProjection` foreground service → `MediaCodec` input Surface | MediaCodec |

The encoder abstraction is `urdevice-media` (`VideoEncoder`, `AutoEncoder`): it tries the OS hardware encoder first and falls back to `openh264` (built from its bundled BSD-2-Clause source), also at run time when a hardware encoder fails mid-stream. Colour conversion and scaling run on the CPU in one pass. VA-API (Linux) and VideoToolbox (macOS) are not built yet: VA-API needs slice-level bitstream code per driver and VideoToolbox needs a Mac to test on; both slot in behind `VideoEncoder` later (T9 hardening).

On Linux, the portal session asks for one monitor with the cursor embedded and `persist_mode` "while the app runs": the dialog shows once per app run, and the restore token stays in memory only. The PipeWire stream offers BGRx/BGRA/RGBx/RGBA without a DMA-BUF modifier, so the compositor sends shared-memory buffers. libpipewire is loaded with `dlopen` because building against it would need its headers and libclang on every build machine.

**Framing:** a `Start` (with the activity) makes the host open one QUIC unidirectional stream (`Direction::ToController`, kind 1, meta `StreamInfo`). Each frame is a big-endian `u32` length (1 byte to 4 MiB) and a `VideoFrame { pts_us, keyframe, data, width, height }`; size is set on keyframes. A second `Start` in the same activity switches the display: the old stream ends and a new one opens. To keep latency low, the **sender** keeps at most 2 encoded frames waiting for the network. A frame that does not fit is dropped, the encoder is asked for a keyframe, and every frame up to that keyframe is dropped too, so the viewer never gets a broken reference chain. Capture also skips frames before copying them while the queue is full. When the receiver falls behind or a frame does not decode, it drops to the next keyframe and sends `RequestKeyframe`; the host honours at most one per 250 ms.

**Adaptation (R12.4):** every 500 ms the host session feeds its sent and dropped frame counts to a ladder of bitrate × resolution steps (1080p30 at 8 Mbps → 720p30 at 4 Mbps → 540p24 at 2 Mbps). When 10 % or more of a window's frames were dropped (the QUIC send window was full: congestion or a slow link) it moves down one step, at most once a second; it moves back up after 5 s without drops. The drop count is used instead of reading quinn's RTT and congestion window, because a full send queue is the direct effect of both and the service cannot reach the connection. The viewer's `Start` can cap width, height, and frame rate.

**Viewing:** on Android, `MediaCodec` decodes straight to a `TextureView` surface (a TextureView rather than a SurfaceView, so pinch-to-zoom can scale it); a tap maps back through the zoom to a position on the host's display. On desktop, `openh264` produces frames that are uploaded into a Slint `Image` through a `SharedPixelBuffer`. The UI shows a display picker (R12.6). When the viewer has input permission, clicks in the view are mapped to `Abs` pointer events on the selected display (R12.5); this needs the input service (#18) and is wired once that is merged.

**Privacy:** screen is a watched capability, so every session is an activity: the host shows its "X is using Screen" indicator with Stop (R5.4), and Stop, Deny, or a disconnect ends capture at once. On Android, the system's own MediaProjection indicator also appears; on Windows 11, the yellow capture border. Frames are never stored or logged.

## 11. Localhost sharing (R13)

```
Device B (viewer)                                   Device A (host)
browser → 127.0.0.1:3000 ─► B listener ─► QUIC stream ─► A: check port in shared_port ─► TCP 127.0.0.1:3000
```

- **Host A:** the user shares port P with an optional label. Shared ports are offered to every paired device (R13.1), and the Localhost setting `tunnel.v1` (§7.4) decides whether a request is allowed, asked, or denied. A accepts `TunnelOpen { port }` only if P is in `shared_port` and the permission check passes (R13.5), and then connects only to `127.0.0.1:P` / `[::1]:P`. Remote devices can't choose the target host, so this can't be turned into an open proxy.
- **Viewer B:** when B starts using a share, it binds `127.0.0.1:P`, or the next free port if P is taken, which the UI shows. Each accepted TCP connection opens a new QUIC stream with `TunnelOpen` and then copies bytes in both directions (`tokio::io::copy_bidirectional`). This works for any TCP protocol, including HTTP/2, WebSockets, and hot-reload (R13.4).
- **Secure context (R13.3):** the browser on B sees `http://localhost:P`, which browsers treat as a secure context.
- **Android viewer:** the core binds the loopback listener inside the app's foreground service, so Chrome on the phone can open `http://localhost:P`.
- **LAN exposure (R13.6):** a per-port setting on the host A. When it is on, A opens a listener on its LAN interfaces that forwards to `127.0.0.1:P`. The user picks "1 hour" (`lan_expose_until` = now + 1 h, then it turns off) or "Always on" (`lan_expose_until` = 0, kept after restart). Always on needs one extra confirmation. While any port is exposed, the tray icon and the "This device" view show an indicator. Viewers keep using their loopback listeners.
- **Stats (R13.7):** each tunnel counts active connections and bytes moved and reports them to the UI.
- **Wire (service 7):** the host pushes `SharedPorts { ports[{port, label}] }` (`ToController`, at most 64) when a session opens and when its list or its Localhost setting changes; the list is empty while the setting is Deny. Each viewer TCP connection is a bidirectional stream of kind 1 with `TunnelOpen { port }` as `meta`, sent `ToHost` under the tunnel's activity (one `ActivityRequest` per tunnel, so Always Ask prompts once per tunnel, not per connection). A port that is not shared resets the stream with `REFUSED`, and the viewer shows "does not share this port any more". Stopping the activity on either device closes the listener and every connection. Limits: 256 connections per tunnel, 512 forwarded connections per host.
- **Free port (R13.2):** the viewer tries P, then P+1 … P+20, then lets the OS pick one.
- **LAN listener:** bound on each LAN IPv4 address from the interface list (not `0.0.0.0`), without address sharing, so it fails if something else already serves that address. A "1 hour" exposure ends within 1 s of its time; exposures that could not bind (no network yet) are retried every 10 s. On Android the indicator is an ongoing notification; on the desktop it is a banner on every page.
- **Known issue:** dev servers that check the `Host` header (for example Vite's `server.allowedHosts`) only need changes when LAN exposure is on.

## 12. Desktop app (R14)

- **Slint UI** (`.slint` files in `crates/urdevice-desktop/ui/`). The layout follows the mockup in [`docs/mockup/urdevice-layout.html`](../../../docs/mockup/urdevice-layout.html). The mockup is a guide, not a final design. A sidebar lists this device, the paired devices (connected, disconnected, or offline with last seen), "Add device", and "Settings" at the bottom. The main area shows:
  - **Onboarding** (first start): one panel with four steps on desktop and Android: device name, the permission settings with their defaults (§7.4), pairing the first device (QR or nearby devices, which can be skipped), and a done screen. The Device ID is not shown here; it stays in Settings (owner decision, 2026-11-24: first-time users do not need it). The full flow is in [`docs/mockup/onboarding.html`](../../../docs/mockup/onboarding.html).
  - **This device:** name, platform, IP, battery; shared localhost ports with "Expose on LAN"; running activities with Stop.
  - **Paired device:** Disconnect / Reconnect and Forget device at the top; a header with platform, connection path and latency, IP, and battery; the BLE banner when needed (§8.2); the media player (§9.4); feature tiles (Clipboard, Send files, Gallery for Android hosts, Remote control, Keyboard, Screen sharing, Localhost) that open the feature views and reflect the host's `PermissionState`: normal for Allow, a "Needs approval" mark for Always Ask, grey for Deny; notifications (Release 2, §18.2).
  - **Android Home** (owner decision 2026-11-24): the device list is one dropdown at the top of Home. It lists this device, every paired device, and "Add Device". The picked device shows in the dropdown with its name, connection line, and battery, and the page below shows what belongs to it: the This device page, or the Paired device page without its own header. This device is picked at start, and the pick is kept while the app is open. The closed dropdown is a card with the device icon and its online dot, name, connection line, battery, and a chevron that turns; the open list is rounded, as wide as the card, with the picked row tinted and checked.
  - **Android motion** (owner decision 2026-11-24): pages slide and fade (forward from the right, back to the right) and the top bar title cross-fades; the app asks Android for the display's fastest mode at the current resolution (120 Hz where the phone has it; Android may still choose less to save power). Compose follows the system "Remove animations" setting. On Home, picking another device slides its page in (from the right for a device further down the list, else from the left). On desktop, the main content fades and rises in again whenever the page, the device, or the feature changes.
  - **Media player** (owner decision 2026-11-24, desktop and Android): not a tile. It is a "Media player" panel on the paired-device page, under the device info and above the file transfers, with an Active switch on the right (off stops the host's updates, R11.6). A card holds one tab per media app (only when the host has more than one), the cover (a music note when there is none), app, title and artist, the seek bar, previous / play-pause / next, and the volume with mute. No shuffle or repeat (R11). The panel shows for a connected device whose host lists media; when the host sets Media to Deny, the switch is off and disabled and the panel says so. The notification-access card for this phone (R11.4) moved to the This device page.
  - **Screen sharing on desktop** (owner decision 2026-11-24): the Screen sharing tile starts the session at once and opens a window of its own, with no page and no extra click. The window is the remote-control window with the picture under the pad: it is sized to the shared screen at 1:1, shrunk (same shape) to fit this computer's screen, and shows the line about the session (asking, starting, ended, refused) and a button per display when the other device has more than one. A click on the picture asks for remote control (the other device approves); once allowed, the pointer, wheel, and keys are captured as in §9.3 and Ctrl+Alt+G lets go. Closing the window ends the session. A static screen sends no new frames, so a keyframe asked for by a late viewer is made from the last picture sent. On Android, a phone that shares its screen follows its rotation: the virtual display and the encoder take the new shape, and the viewer window refits. The Android viewer has a Control chip: while it is on, one finger moves the pointer like a touchpad (relative, so it works on hosts that cannot place the pointer), a tap clicks, and two fingers zoom and pan; with it off, one finger pans and a double tap resets the zoom. If the other device lacks a permission for control, the line says which one.
  - **Add device (Pair):** "Scan QR" when a camera exists (§7.5), this device's QR code (`qrcode` crate) with a countdown, and the "available to pair" list where selecting a device shows the 6-digit code with "Codes match" and "Cancel" (§7.2).
  - **Feature views:** Transfers, Gallery, Remote view (screen plus input capture), Localhost.
  - **Settings:** device name, Device ID, the permission settings (Allow / Always Ask / Deny) for other devices (desktop and Android, owner decision 2026-11-24), hidden mode, start at login, download folder, pause clipboard on all devices, language, activity log, export data, delete all data, and reset identity. The download folder and the clipboard pause are added with file transfer (R8.6) and clipboard sharing (R7.6); until then Settings does not show them.
  - **Prompts and indicators:** the Always Ask prompt window, and the "X is controlling / viewing this device" indicator with Stop (R5.3, R5.4).
- **Tray and menu bar:** `tray-icon`. Start at login: `auto-launch`.
- **Threads:** the Slint event loop runs on the main thread, and the core runs on a tokio runtime in background threads. They exchange messages through `slint::invoke_from_event_loop` and `tokio::sync::mpsc`.
- **Packaging (R14.3):** `cargo-packager` or `cargo-bundle` plus platform signing. CI also handles notarization (macOS) and Authenticode signing (Windows).

## 13. Android app (R15)

- **Structure:** `MainActivity` (Compose, Navigation) and `UrDeviceService` (foreground service, `connectedDevice` type; adds the `mediaProjection` type while sharing the screen). The core runs inside the service.
- **Screens:** Onboarding (as on desktop, plus Gallery), Devices (this device, then paired devices), a bottom sheet with this device's permission settings and shared ports, Device view (same parts as on desktop, §12), Pair (CameraX + ML Kit barcode scanning, the app's own QR with a countdown, and code confirmation), Mousepad, Presenter, Keyboard, Media remote, Screen viewer, Send files, Localhost, and Settings (gear icon at the top right of the app bar). The mockup in `docs/mockup/urdevice-layout.html` shows two navigation options: v1 with pages and a back button, and v2 with a bottom tab bar.
- **Always Ask prompts (R5.3):** a high-priority notification with Allow and Deny actions, and the same prompt in the app when it is open.
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
| Permission prompt times out | Deny and record it in the activity log. The setting stays Always Ask |
| Peer requests a capability the host set to Deny | Reply `Denied { capability }`; record it in the activity log; no prompt |
| No camera on the desktop | Hide "Scan QR"; QR display and code confirmation still work |

Errors in the core use `thiserror` enums. They cross UniFFI as typed Kotlin exceptions.

## 15. Security summary

The full threat model goes in `docs/security/threat-model.md` (task T1.9). Summary:

| Threat | Mitigation |
|---|---|
| Someone on the same Wi-Fi tries to connect | Mutual TLS with pinned keys; pairing only while the pairing screen is open; lockout |
| MITM during pairing | QR: fingerprint pinning plus token HMAC over channel binding. Discovery: 6-digit code shown on both screens |
| Spoofed discovery announcements | Discovery only suggests addresses; identity is checked in TLS |
| Stolen or lost device | Unpair it from any other device. Remote control and screen sharing default to Always Ask (R5.2), so a lost device can't take control without a confirmation on the host, unless the user changed them to Allow. Because settings apply to every paired device, Allow also applies to a lost one; the indicator and Stop (R5.4) stay as the last safeguard. The identity key is in secure storage and protected by the OS lock screen |
| Modified peer ignores the host's Deny | The host's engine checks every request (§7.4); `PermissionState` only drives the peer's UI |
| Malicious peer sends crafted messages | Size limits, protobuf validation, fuzzed parsers, path cleaning, no remote control over tunnel targets |
| Silent surveillance through screen or input | Always Ask by default for screen sharing and remote control, a persistent indicator, a one-click Stop, and an activity log |
| Desktop camera used for QR scanning | Camera on only while scanning on the Pair screen; frames stay in memory and are never stored or sent (R3.10) |
| Clipboard leaks passwords | Sensitive flags respected; global pause |
| Localhost tunnel used as a pivot | Only ports the host shared; Localhost defaults to Deny; loopback targets only; no LAN exposure by default. When it is on, it is either limited to 1 hour or "Always on" after an extra confirmation, with a persistent indicator |
| Logs leak data | Logging (`tracing`) redacts content; field allow-list |

The project also runs `cargo audit` and `cargo deny` in CI, uses only well-known crypto libraries and no custom primitives, and should get an external security review before 1.0.

## 16. Testing strategy

- **Unit tests:** protocol encode/decode, the permission engine, path cleaning, pairing state machines, and backoff timing.
- **Property tests (`proptest`):** path cleaning never leaves the incoming folder; encode→decode returns the same message; framing handles arbitrary splits.
- **Integration tests:** two or three in-process cores on loopback test pairing (both flows), permission settings (Allow, Always Ask with the 30 s timeout, Deny, and the `PermissionState` push), Disconnect and Reconnect, wrong-fingerprint rejection, token replay, lockout, reconnect, file resume, and tunnel allow-listing.
- **Fuzzing (`cargo-fuzz`):** control-message parser, discovery packet parser, BLE reassembly, and QR payload parser.
- **Platform smoke tests:** a device matrix checklist (Windows 11, macOS 14/15, Ubuntu GNOME Wayland, KDE X11, Android 10/13/15) for each feature.
- **Performance checks:** input latency (R9.5), file throughput (R8.8), screen latency (R12.4), and audio delay (T16.5) measured with timestamps embedded in the stream.

## 17. Decisions

1. **Ports:** UDP 47810 for discovery and UDP 47811 for QUIC are final. If 47811 is taken, the app picks another port and advertises it.
2. **Wayland:** supported. The portals come first, with a `uinput` fallback for input injection (§9.3).
3. **Order of work:** project setup ([#1](https://github.com/ahmadridhoramadhan/UrDevice/issues/1)), then Android ↔ desktop ([#2](https://github.com/ahmadridhoramadhan/UrDevice/issues/2)), then desktop ↔ desktop, then the Release 2 features (§18).
4. **Owner decisions after the UI mockup review (17 Nov 2026):**
   - Permissions are one setting per capability on each host, applied to every paired device, with the labels Allow, Always Ask, and Deny. Defaults are set in onboarding (§7.4). Deny turns the feature off on the requesting device.
   - Desktops with a camera can scan pairing QR codes (§7.5).
   - Pairing uses the two flows in §7.1 and §7.2 only. Typing in a code is not supported.
   - Bluetooth is an automatic fallback, not a transport the user picks.
   - Media control has source tabs, seek, and mute, but no shuffle or repeat (§9.4).
   - Localhost "Expose on LAN" offers "1 hour" and "Always on" (§11).
   - Battery level, connection latency, and Disconnect without unpairing are part of the spec (R4.7–R4.9).
   - Notification mirroring also works from desktop to Android (§18.2).

## 18. Release 2 features

Each feature below is a core service (§9) with its own capability string and service id (§6.2), which is negotiated in the handshake (§6.3) and checked by the permission engine (§7.4). Section 18.N covers Requirement 16+N.

"Default when built" means the default is chosen when the feature is implemented and then added to onboarding (R5.2). `Capability::default_mode` holds provisional values until then: Ask for notify, camera, mic, telephony, kvm, display, speaker, and fs.browse; Allow for ring, cliphist, and lock; Deny for adb.

| Requirement | Capability | Default and notes |
|---|---|---|
| R17 Webcam and microphone | `camera.v1`, `mic.v1` | Default when built |
| R18 Notification mirroring | `notify.v1` | Default when built; receiving is also off per host on the receiver (R18.2) |
| R19 Shared mouse and keyboard | `kvm.v1` | Default when built (uses the input setting) |
| R20 Second display | `display.v1` | Default when built |
| R21 Remote terminal | `term.v1` | Deny; warning and extra confirmation before Allow; changed only on the host |
| R22 DevTools and ADB | `adb.v1` | Default when built |
| R23 Port profiles | `tunnel.v1` (existing) | Follows localhost |
| R24 Request inspector | local only | Off |
| R25 Find my device | `ring.v1` | Default when built |
| R26 File browser | `fs.browse.v1`, `fs.write.v1` | `fs.browse.v1` default when built; `fs.write.v1` Deny, with a warning and extra confirmation before Allow |
| R27 Clipboard history | `cliphist.v1` | Follows clipboard |
| R28 Proximity lock | `lock.v1` | Default when built |
| R29 SMS and calls | `telephony.v1` | Default when built |
| R30 Automations | local only | n/a |
| R31 Speaker | `audio.out.v1` | Default when built |

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

- **Subscription (R18.2):** the receiver sends `NotifSubscribe { on, apps[] }` to the host. Receiving is off until the user turns it on for that host, and `apps[]` lists the apps the user picked (`notif_subscription`, §4.2). The host sends its app list with `NotifApps { apps[{ id, name, icon_png ≤64px }] }` so the receiver can show the picker. The host filters before sending, so notifications from other apps never leave the host. The host's `notify.v1` setting (§7.4) is checked as well.
- **Android host:** the existing `MediaListenerService` (a `NotificationListenerService`) also handles `onNotificationPosted` and `onNotificationRemoved`. It sends `Notif { key, app, title, text, icon_png ≤64px, actions[], reply_capable, visibility }`. Filtering by visibility happens on the phone before sending (R18.3).
- **Desktop host (desktop → Android):**

| Desktop | How notifications are read | Notes |
|---|---|---|
| Windows 10 1709+ | `UserNotificationListener` (WinRT), polled about once a second | Works from an unpackaged exe on Windows 11 build 26200, with the user's notification access setting. The change event needs package identity, so UrDevice polls and diffs by id. Checked in spike T10.5 |
| Linux | D-Bus monitor (`BecomeMonitor`) on `org.freedesktop.Notifications.Notify` with `zbus` | Works on desktops that use the standard notification service. Checked in spike T10.5 |
| macOS | Not supported | No public API. The UI says so (R18.1) |

  Desktop notifications are sent without actions or reply (R18.4).
- **Receiver:** desktop shows native notifications through `notify-rust` (Linux, macOS) and WinRT toast notifications (Windows). Android posts them in a "From <device>" notification channel. Both also list them in the host's device view.
- **Reply (R18.4):** for Android hosts only. The receiver sends `NotifAction { key, action_index, reply_text? }`. The phone fills the `RemoteInput` results and fires the action's `PendingIntent`.
- **Dismissal sync (R18.5):** `NotifDismiss { key }` in both directions. An Android host calls `cancelNotification(key)`. Desktop hosts can't dismiss another app's notification on every OS, so on desktop hosts dismissal sync only works from host to receiver.
- **Retention (R18.6):** the receiver keeps notifications in memory only and drops them after dismissal or 24 h.

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
- **Android:** a full-screen notification and `MediaPlayer` with `USAGE_ALARM`. The alarm volume is raised for the duration of the ring and then restored. Do Not Disturb bypass needs notification-policy access. Android 14 limits full-screen intents: `USE_FULL_SCREEN_INTENT` is not granted by default to an app that is not an alarm or calling app (Google Play policy, spike T10.4). The app checks `canUseFullScreenIntent()`, offers the system settings screen once, and falls back to a high-priority notification when it isn't allowed. The alarm-stream ring does not depend on it.
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

### 18.15 Speaker (R31)

- **Capability:** `audio.out.v1`. It is negotiated in the handshake (§6.3) and checked by the permission engine (§7.4). The target's single setting applies to every paired source. When it is `allow`, audio plays at once with no prompt; `ask` shows the Always Ask prompt (R31.3). A change to the setting takes effect within 1 s (R31.4, R5.5).
- **Platforms (R31.1):** host and target can each be Windows, macOS, Linux, or Android.
- **Transport:** audio travels over the existing session (R31.2). The codec is Opus. Whether it uses a QUIC stream or datagrams is decided after T16.2.
- **Host (source):** a per-OS adapter captures system audio. Whether each OS can do this is the result of spike T16.1. IF an OS cannot capture audio THEN the UI says so and explains what the user must enable (R31.7).
- **Target:** a per-OS adapter plays the audio, with its own volume and a mute for each source (R31.6).
- **Indicator and Stop (R31.5):** the playing session registers with the `ActivityMonitor` (§7.4), which drives the indicator naming the source and the Stop button.
- **Activity log (R31.8):** each start and stop is recorded with the device and time, without audio content.

### 18.16 Release 2 risks

| Risk | Affects | Plan |
|---|---|---|
| macOS private APIs (`CGVirtualDisplay`, `SACLockScreenImmediate`) | R20, R28 | Feature flags, fallbacks, retest on every macOS release |
| Windows driver signing (IddCx, virtual audio) | R17.4, R20 | Get an EV certificate. Virtual audio waits until the IddCx process is in place |
| Wayland InputCapture portal not available on all desktops | R19 | Spike T10.1 and document which desktops work |
| Google Play policy (SMS, call permissions, full-screen intents) | R25, R29 | Separate `full` build, with fallbacks in the Play build |
| Reading notifications on desktop hosts (Windows package identity, Linux D-Bus monitoring) | R18 | Spike T10.5; macOS stays unsupported |
| New attack surface (terminal, file browser, run command) | R21, R26, R30 | Terminal and file changes default to Deny, with a warning and extra confirmation before Allow, settings changeable only on the device itself, `cap-std` sandboxing, fuzzing, and a second external security review before Release 2 |
