# UrDevice: Requirements

## Introduction

UrDevice joins a user's own devices (macOS, Windows, Linux, Android) into one private, peer-to-peer mesh on the local network. Paired devices share clipboard, files, and photos. They can also control each other, view each other's screens, use each other as keyboards and media remotes, and reach each other's localhost dev servers. Nothing goes through a cloud server.

Desktop apps use a Rust core with a Slint UI. The Android app uses native Kotlin for the UI and platform integration.

Acceptance criteria use EARS phrasing (WHEN / IF / WHILE ... THEN the system SHALL ...).

### Glossary

- **Device**: one installation of UrDevice. It has a long-term cryptographic identity.
- **Device ID**: the SHA-256 fingerprint of a device's public identity key.
- **Paired device**: a device whose identity key the local device has stored and trusts.
- **Host**: the device being controlled, viewed, or serving a resource.
- **Controller**: the device sending input or viewing a screen.
- **Session**: one authenticated, encrypted connection between two paired devices.
- **Capability**: a feature one device grants another, such as clipboard, files, input, screen, localhost, or media.

### Scope

In scope for v1: every feature listed below on Windows 10+, macOS 13+, Linux (X11 and Wayland via portals), and Android 10+ (API 29+).

Out of scope for v1: iOS, use over the internet or across subnets (no relay or NAT traversal), multiple user accounts sharing one mesh, audio streaming during screen sharing, and Bluetooth fallback between two desktops.

---

## Requirement 1: Device identity

**User story:** As a user, I want each device to have a stable identity that can't be forged, so that only my devices can connect to each other.

1. WHEN UrDevice starts for the first time THEN the system SHALL generate a long-term Ed25519 identity key pair and a self-signed X.509 certificate bound to that key.
2. The system SHALL store the private key in the platform's secure storage: Keychain on macOS, DPAPI/Credential Manager on Windows, Secret Service on Linux (with an encrypted-file fallback that has 0600 permissions), and an Android Keystore–wrapped key on Android.
3. The system SHALL never send the private key off the device, write it to logs, or include it in backups.
4. The system SHALL show the Device ID to the user as a short, readable fingerprint (for example, 8 groups of 4 characters).
5. WHEN the user chooses "Reset identity" THEN the system SHALL create a new key pair, delete all paired-device records, and warn that every device must pair again.

## Requirement 2: Automatic discovery

**User story:** As a user, I want my devices to find each other on the same network on their own, so that I never type an IP address.

1. WHILE the app is running and discovery is on THEN the system SHALL advertise itself over mDNS/DNS-SD as service type `_urdevice._udp.local.`.
2. WHILE the app is running and discovery is on THEN the system SHALL also send a UDP broadcast announcement every 5 seconds on a fixed port, for networks that block multicast.
3. Announcements SHALL include only the protocol version, a short Device ID prefix, the display name, the listening port, and the platform type. They SHALL NOT include secrets or pairing tokens.
4. WHEN an announcement arrives from a paired device THEN the system SHALL connect to it automatically within 3 seconds.
5. WHEN an announcement arrives from an unpaired device THEN the system SHALL list it as "available to pair" and SHALL NOT connect without the user's action.
6. The system SHALL rate-limit inbound discovery packets (at most 20 per second per source address) and ignore malformed ones.
7. WHERE the user enables "Hidden mode" THEN the system SHALL stop announcing itself and accept connections only from paired devices.

## Requirement 3: Pairing

**User story:** As a user, I want to pair two devices quickly and safely, by scanning a QR code or confirming a code, so that strangers on the same Wi-Fi can't join.

1. WHEN the user opens "Pair new device" THEN the system SHALL show a QR code. The QR code contains the protocol version, the full Device ID, the display name, the current IP addresses and port, a random 128-bit one-time pairing token, and an expiry time.
2. Pairing tokens SHALL expire after 120 seconds or after one use, whichever comes first.
3. WHEN a device scans the QR code THEN it SHALL connect and SHALL abort IF the host's certificate fingerprint does not match the Device ID in the QR code.
4. The scanning device SHALL prove it knows the token by sending an HMAC of the token over a TLS channel-binding value. The raw token SHALL never be sent.
5. WHEN the user starts pairing with a discovered device without a QR code THEN both devices SHALL show the same 6-digit verification code derived from the TLS session. Pairing SHALL complete only after the user confirms the codes match on both devices.
6. WHEN pairing succeeds THEN both devices SHALL store each other's Device ID, certificate fingerprint, display name, and platform, and SHALL show a confirmation.
7. IF pairing fails 5 times in 5 minutes THEN the system SHALL stop accepting pairing attempts for 5 minutes and tell the user.
8. The system SHALL accept pairing attempts only while the "Pair new device" screen is open.
9. WHEN the user removes a paired device THEN the system SHALL delete its record, close any open session with it, and refuse its future connections until it pairs again.

## Requirement 4: Secure connections

**User story:** As a user, I want all traffic between my devices encrypted and authenticated, so that nobody on the network can read or tamper with it.

1. All traffic between devices (other than discovery announcements) SHALL use QUIC with TLS 1.3 and mutual certificate authentication.
2. The system SHALL accept a peer only IF the peer's certificate fingerprint matches a stored paired device, or IF a pairing exchange is in progress.
3. The system SHALL refuse connections that offer an unsupported protocol version and SHALL show a "please update" message.
4. WHEN a session is established THEN the devices SHALL exchange their supported capabilities, and each side SHALL use only features both support and the user has allowed.
5. WHEN a connection drops THEN the system SHALL try to reconnect with exponential backoff (1 s up to 30 s) and SHALL resume the session without asking the user again.
6. The system SHALL treat every received message as untrusted. It SHALL validate sizes and fields and close the session IF a message is malformed.

## Requirement 5: Per-device permissions

**User story:** As a user, I want to choose what each paired device may do on this device, so that a lost phone can't silently control my computer.

1. The system SHALL keep a separate permission setting for each capability for each paired device. The options are Allow, Ask every time, and Deny.
2. Defaults for a newly paired device SHALL be: clipboard Allow, files Ask, gallery browsing Ask, media control Allow, keyboard/mouse input Ask, screen viewing Ask, localhost access Ask.
3. WHEN a capability set to Ask is requested THEN the host SHALL show a prompt naming the requesting device and the capability. IF the user does not answer within 30 seconds THEN the system SHALL deny the request.
4. WHILE another device is controlling input or viewing the screen THEN the host SHALL show a persistent, visible indicator and a one-click "Stop" control.
5. WHEN the user presses Stop or revokes a permission THEN the system SHALL end the affected activity within 1 second.
6. The system SHALL keep a local activity log of granted and denied requests (device, capability, time) for the last 30 days. Log entries SHALL NOT contain clipboard or file contents.

## Requirement 6: Bluetooth fallback

**User story:** As a user, I want my devices to stay connected if Wi-Fi drops, so that remote control keeps working during a presentation.

1. WHEN a paired session loses its LAN path and both devices have a Bluetooth PAN link available THEN the system SHALL move the session to the PAN network path without asking the user to pair again.
2. WHERE PAN is not available (for example, macOS has no Bluetooth PAN since Monterey) AND one side is Android THEN the system SHALL fall back to a Bluetooth LE control channel. This channel carries only low-bandwidth features: input, presenter commands, media control, and clipboard text up to 64 KB.
3. The Bluetooth fallback SHALL use the same identity keys and SHALL be end-to-end encrypted and authenticated just like the LAN path.
4. WHILE running on the fallback path THEN the system SHALL tell the user which features are unavailable (for example, screen sharing and large file transfers).
5. WHEN the LAN path returns THEN the system SHALL move the session back to it within 10 seconds.
6. WHERE the OS does not let the app turn on Bluetooth tethering itself (for example, Android) THEN the system SHALL guide the user to the right settings screen.

## Requirement 7: Clipboard sharing

**User story:** As a user, I want to copy on one device and paste on another.

1. WHEN clipboard sync is on and the local clipboard changes THEN the system SHALL send the new content to connected paired devices that have clipboard permission, within 1 second.
2. The system SHALL support plain text, rich text/HTML, and images (PNG) up to 10 MB.
3. The system SHALL NOT sync content marked as sensitive or concealed by the OS or the source app. Examples: `ExcludeClipboardContentFromMonitorProcessing` on Windows, `org.nspasteboard.ConcealedType` on macOS, `ClipDescription.EXTRA_IS_SENSITIVE` on Android.
4. The system SHALL prevent echo loops, so content received from a peer is not sent back out.
5. WHERE the OS blocks background clipboard access (Android 10+) THEN the Android app SHALL provide manual send actions: a notification action, a Quick Settings tile, and the share sheet. It SHALL write incoming clipboard content when allowed.
6. The user SHALL be able to switch clipboard sync on or off per device and pause it globally.

## Requirement 8: File and photo sharing

**User story:** As a user, I want to send files and photos between any of my devices and see progress.

1. The user SHALL be able to send one or more files or folders to a paired device by file picker, drag-and-drop (desktop), or share sheet (Android).
2. WHEN a transfer is offered THEN the receiver SHALL show the sender, file names, and total size. It SHALL accept or decline according to the files permission.
3. WHILE a transfer is running THEN both devices SHALL show progress (percent, speed, time remaining) and allow cancelling.
4. The system SHALL verify each file with SHA-256 after transfer and SHALL discard files that fail the check.
5. IF a transfer is interrupted THEN the system SHALL resume it from the last verified chunk once the connection returns.
6. The receiver SHALL save files to a configurable folder (default `Downloads/UrDevice`). It SHALL clean incoming file names to block path traversal (`..`, absolute paths, reserved names) and SHALL NOT overwrite existing files silently.
7. WHERE the gallery permission is granted THEN a desktop SHALL be able to browse an Android device's photos and videos as thumbnails and download selected items.
8. On an average Wi-Fi 5 network, transfers SHOULD reach at least 20 MB/s.

## Requirement 9: Remote control (mouse and touch)

**User story:** As a user, I want to control any of my devices from any other device.

1. The system SHALL let a controller send pointer movement, clicks (left, right, middle), scroll, and drag to a desktop host.
2. On a desktop host, the system SHALL inject input with the native API: SendInput on Windows, CGEvent on macOS (needs the Accessibility permission), XTest on X11, and the RemoteDesktop portal/libei on Wayland.
3. WHERE the controller is a desktop and the host is Android THEN the system SHALL send taps, swipes, and the back/home/recents actions through an Android AccessibilityService that the user has enabled.
4. IF the host lacks the required OS permission THEN the system SHALL explain which permission is missing and open the settings screen for it.
5. Input latency from controller to host SHOULD be under 50 ms (95th percentile) on the LAN.

### 9a: Mousepad mode (Android → desktop)

1. Swiping one finger SHALL move the cursor, with adjustable sensitivity and pointer acceleration.
2. A single tap SHALL left-click. A two-finger tap SHALL right-click. A long press (500 ms) SHALL right-click.
3. Swiping two fingers SHALL scroll vertically and horizontally.
4. Tap then hold-and-drag SHALL perform a drag.
5. The mousepad SHALL keep the phone screen awake while it is open.

### 9b: Presenter mode (Android → desktop)

1. The screen SHALL provide Next and Previous slide buttons. The phone's volume keys SHALL also work as next/previous while the screen is open.
2. The screen SHALL provide Blank screen (sends `B`), Start presentation (`F5`), and End presentation (`Esc`).
3. The laser pointer SHALL draw a visible dot on top of all windows on the host. It SHALL follow the phone's gyroscope or touch movement and disappear when released.
4. The slide timer SHALL run on the phone. It SHALL show elapsed time and optionally vibrate at user-set alerts.

## Requirement 10: Keyboard

**User story:** As a user, I want to type on one device and have the text appear on another.

1. Android → desktop: the app SHALL provide a keyboard screen that sends typed Unicode text, plus special keys (Enter, Backspace, Tab, Esc, arrows, Fn keys) and modifier shortcuts (Ctrl/Cmd/Alt/Shift combinations).
2. Desktop → desktop: the controller SHALL capture keystrokes while its remote-control window is focused and forward them to the host.
3. Desktop → Android: the system SHALL type into the focused field through an optional UrDevice input method (IME) that the user enables.
4. The system SHALL send Unicode text as text, not as layout-dependent key codes, so that it works across different keyboard layouts.

## Requirement 11: Media control

**User story:** As a user, I want to control media playing on another device.

1. The controller SHALL be able to send Play/Pause, Next, Previous, Volume up/down/set, and Mute to a host.
2. WHERE the host OS exposes now-playing information (Windows SMTC, Linux MPRIS, Android MediaSession) THEN the controller SHALL show the title, artist, artwork, and playback state.
3. WHERE now-playing information is not available (macOS) THEN the host SHALL fall back to sending system media keys and controlling the system volume.
4. On Android hosts, the system SHALL need the notification-listener permission to read media sessions and SHALL explain why.

## Requirement 12: Screen sharing

**User story:** As a user, I want to watch another device's screen live.

1. A paired device SHALL be able to request to view a host's screen. The host SHALL follow the screen permission.
2. Desktop hosts SHALL capture with the native API: Windows Graphics Capture, ScreenCaptureKit on macOS, and PipeWire through the ScreenCast portal on Linux (X11 capture as a fallback). Android hosts SHALL use MediaProjection, which always shows the system consent dialog.
3. The host SHALL encode video as H.264 using hardware encoding when available and a software fallback otherwise.
4. On the LAN, the stream SHOULD reach 1080p at 30 fps with glass-to-glass latency under 150 ms. It SHALL lower resolution or bitrate when bandwidth drops.
5. WHERE the viewer also has input permission THEN the viewer SHALL be able to control the host from the viewing window (combining Requirements 9 and 12).
6. On multi-monitor hosts, the viewer SHALL be able to choose which display to watch.
7. Android → desktop: the desktop SHALL show the phone screen in a resizable window.

## Requirement 13: Localhost sharing

**User story:** As a developer, I want to open a dev server running on one device from my other devices, without deploying it.

1. The user SHALL be able to share a local TCP port (for example `localhost:3000`) on the host with selected paired devices.
2. WHEN a paired device opens a shared port THEN the system SHALL create a listener on that device's loopback (`127.0.0.1:<port>`, or the next free port) and tunnel connections over the encrypted session to the host's `localhost:<port>`.
3. Because the tunnel ends at `localhost` on the viewing device, browsers SHALL treat the page as a secure context, so service workers, camera access, and similar APIs work the same as on the host.
4. The tunnel SHALL support HTTP/1.1, HTTP/2, WebSockets, and any other TCP protocol, including dev-server hot reload.
5. The host SHALL only forward to the ports the user shared. It SHALL refuse requests for any other port or host.
6. By default, shared ports SHALL NOT be reachable from the LAN by unpaired devices. An optional "Expose on LAN" setting SHALL show a clear warning and turn itself off after 1 hour.
7. The system SHALL show active tunnels, how many connections each has, and the traffic used, and SHALL let the user stop any of them.

## Requirement 14: Desktop app

1. The desktop app SHALL run in the system tray or menu bar and SHALL be able to start at login.
2. The main window SHALL list paired and available devices with their connection state and the features open to each.
3. The app SHALL ship as a signed, notarized `.dmg` (macOS), a signed `.msi` (Windows), and `.deb`, `.rpm`, and AppImage/Flatpak packages (Linux).

## Requirement 15: Android app

1. The app SHALL use a foreground service with a persistent notification while connected, so the OS does not kill it.
2. The app SHALL ask for each runtime permission only when the user first uses the feature that needs it, and SHALL explain why.
3. The app SHALL support a share-sheet target ("Send to UrDevice") and a Quick Settings tile.
4. While idle and connected, the app SHOULD use less than 2% battery per hour.

## Requirement 16: Privacy and quality

1. UrDevice SHALL work fully offline. It SHALL NOT send telemetry, analytics, or crash reports unless the user opts in.
2. Logs SHALL NOT contain clipboard content, file contents, keystrokes, pairing tokens, or private keys.
3. Users SHALL be able to export and delete all local data from settings.
4. All UI text SHALL be translatable, with English as the first language.
