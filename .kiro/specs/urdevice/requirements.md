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
- **Permission setting**: the host's choice for one capability: Allow, Always Ask, or Deny. It applies to every paired device that requests that capability on the host (R5).

### Scope

Platforms: Windows 10+, macOS 13+, Linux (X11 and Wayland), and Android 10+ (API 29+). Individual requirements note where a feature needs a newer OS version.

Releases and order of work:

- **Release 1 (R1–R16):** project setup, then Android ↔ desktop, then desktop ↔ desktop.
- **Release 2 (R17–R31):** additional features, built after Release 1 is stable.

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
4. WHEN an announcement arrives from a paired device THEN the system SHALL connect to it automatically within 3 seconds, unless the user disconnected that device (R4.7).
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
8. The system SHALL accept QR pairing only while the "Pair new device" screen is open. WHEN another device asks to pair by code while that screen is closed THEN the system SHALL ask the user to accept or decline (a prompt window on desktop, a notification on Android), SHALL open the screen only after Accept, and SHALL treat no answer within 30 seconds as declined. The system SHALL ask one question at a time, at most 5 a minute, and SHALL refuse a device for 60 seconds after its request was declined or ignored (owner decision 2026-11-24).
9. WHEN the user removes a paired device THEN the system SHALL delete its record, close any open session with it, and refuse its future connections until it pairs again.
10. Any device with a camera SHALL be able to scan another device's pairing QR code from its "Pair new device" screen. This includes desktops: WHERE a desktop has a camera THEN the screen SHALL offer "Scan QR" and SHALL ask for the OS camera permission the first time it is used. WHERE no camera is available THEN the screen SHALL hide the scan option, and the QR code and code confirmation SHALL still work. Camera frames SHALL be processed in memory only and SHALL NOT be stored or sent.
11. WHEN a pairing token expires while the "Pair new device" screen is open THEN the system SHALL show a new QR code with a new token, and the screen SHALL show how long the current code is valid.

## Requirement 4: Secure connections

**User story:** As a user, I want all traffic between my devices encrypted and authenticated, so that nobody on the network can read or tamper with it.

1. All traffic between devices (other than discovery announcements) SHALL use QUIC with TLS 1.3 and mutual certificate authentication.
2. The system SHALL accept a peer only IF the peer's certificate fingerprint matches a stored paired device, or IF a pairing exchange is in progress.
3. The system SHALL refuse connections that offer an unsupported protocol version and SHALL show a "please update" message.
4. WHEN a session is established THEN the devices SHALL exchange their supported capabilities, and each side SHALL use only features both support and the user has allowed.
5. WHEN a connection drops THEN the system SHALL try to reconnect with exponential backoff (1 s up to 30 s) and SHALL resume the session without asking the user again.
6. The system SHALL treat every received message as untrusted. It SHALL validate sizes and fields and close the session IF a message is malformed.
7. WHEN the user chooses "Disconnect" for a paired device THEN both devices SHALL close the session and SHALL NOT reconnect automatically. The pairing SHALL stay stored. WHEN the user chooses "Reconnect" on either device THEN the devices SHALL connect again and resume automatic reconnection.
8. WHILE a session is open THEN each device SHALL show, for that peer, the current connection path (Wi-Fi/LAN, Bluetooth PAN, or Bluetooth LE) and the round-trip latency.
9. Each device SHALL send its battery level and charging state to connected paired devices when the session starts and when the level changes by at least 1%, at most once a minute. The peer SHALL show it. A device without a battery SHALL send nothing, and the UI SHALL show no battery for it.

## Requirement 5: Access permissions

**User story:** As a user, I want to choose what my other devices may do on this device, so that a lost phone can't silently control my computer.

1. Each device SHALL keep one permission setting for each capability. The setting applies to every paired device that requests that capability on this device. The options are Allow, Always Ask, and Deny.
2. WHEN UrDevice starts for the first time THEN it SHALL show an onboarding screen where the user sets each capability, starting from these defaults:

   | Capability | Default |
   |---|---|
   | Clipboard | Allow |
   | Receive files | Always Ask |
   | Gallery (Android hosts only) | Always Ask |
   | Remote control (pointer, keyboard, and presenter) | Always Ask |
   | Media | Allow |
   | Screen sharing | Always Ask |
   | Localhost | Deny |

   The user SHALL be able to change any setting later. Release 2 capabilities SHALL get their default when they are built, except the remote terminal (R21) and file changes (R26: upload, rename, delete), which default to Deny. For these two, choosing Allow SHALL show a clear warning that the other device gets full control of this device's shell or files, and SHALL need one extra confirmation.
3. WHEN a capability set to Allow is requested THEN the feature SHALL start with no prompt. WHEN a capability set to Always Ask is requested THEN the host SHALL show a confirmation (a prompt window on desktop, a notification on Android) naming the requesting device and the feature, with Allow and Deny. IF the user does not answer within 30 seconds THEN the system SHALL deny the request. Each approval covers one activity: one transfer, one screen session, or one remote-control session.
4. WHILE another device is controlling input, viewing the screen, or playing its audio on this device (R31.5) THEN the host SHALL show a persistent, visible indicator and a one-click "Stop" control.
5. WHEN the user presses Stop or revokes a permission THEN the system SHALL end the affected activity within 1 second.
6. The system SHALL keep a local activity log of granted and denied requests (device, capability, time) for the last 30 days. Log entries SHALL NOT contain clipboard or file contents.
7. WHEN a capability is set to Deny THEN the host SHALL refuse every request for it, and connected devices SHALL show that feature as turned off by the host. The host SHALL send its permission settings to each connected paired device when the session starts and within 1 second of any change. The host SHALL still check every request itself and SHALL NOT rely on the peer's copy of the settings.

## Requirement 6: Bluetooth fallback

**User story:** As a user, I want my devices to stay connected if Wi-Fi drops, so that remote control keeps working during a presentation.

1. WHEN a paired session loses its LAN path and both devices have a Bluetooth PAN link available THEN the system SHALL move the session to the PAN network path without asking the user to pair again.
2. WHERE PAN is not available (for example, macOS has no Bluetooth PAN since Monterey) AND one side is Android THEN the system SHALL fall back to a Bluetooth LE control channel. This channel carries only low-bandwidth features: input, presenter commands, media control, and clipboard text up to 64 KB.
3. The Bluetooth fallback SHALL use the same identity keys and SHALL be end-to-end encrypted and authenticated just like the LAN path.
4. WHILE running on the Bluetooth LE fallback path THEN the view of that device SHALL show a banner that names the features that are unavailable (for example, screen sharing, file transfers, and localhost), and those features SHALL appear turned off with the reason. WHILE running on the PAN path THEN all features SHALL stay available, and the connection path (R4.8) SHALL show Bluetooth.
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
2. On a desktop host, the system SHALL inject input with the native API: SendInput on Windows, CGEvent on macOS (needs the Accessibility permission), XTest on X11, and the RemoteDesktop portal/libei on Wayland. WHERE a Wayland compositor has no RemoteDesktop portal THEN the system SHALL fall back to a `uinput` virtual device and SHALL guide the user through the one-time setup it needs (a udev rule that gives the user access to `/dev/uinput`).
3. WHERE the controller is a desktop and the host is Android THEN the system SHALL send taps, swipes, and the back/home/recents actions through an Android AccessibilityService that the user has enabled. The phone SHALL draw a pointer on its screen that the controller's mouse moves (relative motion, like a touchpad), so the user sees where a click lands. Remote control and Keyboard are one tile and one window on the controller.
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

1. The controller SHALL be able to send Play/Pause, Next, Previous, Seek, Volume up/down/set, and Mute to a host. Volume and Mute act on the host's system output.
2. WHERE the host OS exposes media sessions (Windows SMTC, Linux MPRIS, Android MediaSession) THEN the host SHALL send the list of active sessions, each with its app name, title, artist, artwork, playback state, position, and duration. The controller SHALL show them as sources the user can switch between (for example Spotify, YouTube, Chrome), and SHALL send commands to the session the user selected.
3. WHERE now-playing information is not available (macOS) THEN the host SHALL offer one "System" session that sends system media keys (Play/Pause, Next, Previous) and controls the system volume and mute.
4. On Android hosts, the system SHALL need the notification-listener permission to read media sessions and SHALL explain why.
5. Each session SHALL report which commands it supports (for example Seek, Next, Previous). The controller SHALL show only the controls the selected session supports.
6. The user SHALL be able to turn the media player for a host off and on again on the controller. WHILE it is off THEN the host SHALL stop sending media updates to that controller.

## Requirement 12: Screen sharing

**User story:** As a user, I want to watch another device's screen live.

1. A paired device SHALL be able to request to view a host's screen. The host SHALL follow the screen permission.
2. Desktop hosts SHALL capture with the native API: Windows Graphics Capture, ScreenCaptureKit on macOS, and PipeWire through the ScreenCast portal on Linux (X11 capture as a fallback). Android hosts SHALL use MediaProjection, which always shows the system consent dialog.
3. The host SHALL encode video as H.264 using hardware encoding when available and a software fallback otherwise.
4. On the LAN, the stream SHOULD reach 1080p at 30 fps with glass-to-glass latency under 150 ms. It SHALL lower resolution or bitrate when bandwidth drops.
5. WHERE the viewer also has input permission THEN the viewer SHALL be able to control the host from the viewing window (combining Requirements 9 and 12).
6. On multi-monitor hosts, the viewer SHALL be able to choose which display to watch. WHERE the host is a Linux Wayland session THEN the host's ScreenCast portal dialog SHALL choose the display instead, because Wayland does not let apps list or pick monitors without it.
7. Android → desktop: the desktop SHALL show the phone screen in a resizable window.

## Requirement 13: Localhost sharing

**User story:** As a developer, I want to open a dev server running on one device from my other devices, without deploying it.

1. The user SHALL be able to share a local TCP port (for example `localhost:3000`) on the host, with an optional label. A shared port SHALL be available to every paired device, subject to the host's Localhost permission setting (R5).
2. WHEN a paired device opens a shared port THEN the system SHALL create a listener on that device's loopback (`127.0.0.1:<port>`, or the next free port) and tunnel connections over the encrypted session to the host's `localhost:<port>`.
3. Because the tunnel ends at `localhost` on the viewing device, browsers SHALL treat the page as a secure context, so service workers, camera access, and similar APIs work the same as on the host.
4. The tunnel SHALL support HTTP/1.1, HTTP/2, WebSockets, and any other TCP protocol, including dev-server hot reload.
5. The host SHALL only forward to the ports the user shared. It SHALL refuse requests for any other port or host.
6. By default, shared ports SHALL NOT be reachable from the LAN by unpaired devices. The host SHALL offer an optional "Expose on LAN" setting for each shared port. Turning it on SHALL show a clear warning and let the user choose "1 hour" or "Always on". The 1-hour choice SHALL turn itself off after 1 hour. "Always on" SHALL need one extra confirmation and SHALL stay on, including after a restart, until the user turns it off. WHILE any port is exposed on the LAN THEN the host SHALL show a persistent indicator.
7. The system SHALL show active tunnels, how many connections each has, and the traffic used, and SHALL let the user stop any of them.

## Requirement 14: Desktop app

1. The desktop app SHALL run in the system tray or menu bar and SHALL be able to start at login.
2. The main window SHALL list this device and the paired devices with their state (connected, disconnected by the user, or offline with the time they were last seen) and the features each host allows (R5.7). Devices available to pair SHALL be listed on the "Pair new device" screen.
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

---

# Release 2: Additional features

These requirements build on the Release 1 core: the encrypted session, access permissions (R5), the video pipeline (R12), input injection (R9), and tunnels (R13). Every new capability goes through the permission engine and follows the indicator and Stop rules in R5.4 and R5.5 when it is active.

## Requirement 17: Phone as webcam and microphone

**User story:** As a user, I want to use my phone's camera and microphone on my desktop, so that I get better video calls without buying a webcam.

1. The Android app SHALL stream the front or back camera, chosen by the user, to a paired desktop. The user SHALL be able to switch cameras, toggle the flash, and set zoom during the stream.
2. The desktop SHALL expose the stream as a virtual camera that other apps can select: a CoreMediaIO Camera Extension on macOS, `MFCreateVirtualCamera` on Windows 11, and `v4l2loopback` on Linux (the app guides the user to install the module).
3. WHERE the OS has no supported virtual camera (Windows 10) THEN the desktop SHALL show the camera in a window only and SHALL explain the limitation.
4. The Android app SHALL optionally stream the microphone. Linux SHALL expose it as a PipeWire virtual source. On macOS and Windows, the microphone SHALL be available only once a virtual audio driver ships, and until then the app SHALL say so.
5. The stream SHALL reach at least 720p at 30 fps, SHOULD reach 1080p at 30 fps, and SHOULD have under 150 ms latency on the LAN.
6. WHILE the camera or microphone is streaming THEN the phone SHALL show a persistent notification with Stop, in addition to Android's own privacy indicator.

## Requirement 18: Notification mirroring

**User story:** As a user, I want to see and answer my phone's notifications on my desktop, and see my desktop's notifications on my phone, so that I don't have to switch devices.

1. WHEN a notification is posted on a host and the receiving device has turned on receiving from that host THEN the system SHALL send its app name, title, text, and icon to that device, subject to the host's notifications permission setting (R5). The receiver SHALL show it as a native notification and in that host's device view. Supported directions: Android → desktop, and desktop → Android WHERE the desktop OS lets the app read notifications (Windows and Linux). macOS hosts SHALL NOT mirror their notifications, because macOS has no public API for it, and the UI SHALL say so.
2. Receiving SHALL be off by default for each host. On the receiver, the user SHALL turn receiving on and off for each host and SHALL choose which of the host's apps are mirrored. Newly seen apps SHALL NOT be mirrored by default.
3. The system SHALL NOT send the content of notifications marked `VISIBILITY_SECRET`, and SHALL send only "New notification from <app>" for `VISIBILITY_PRIVATE` notifications unless the user allows full content.
4. WHERE a notification from an Android host has a reply action (`RemoteInput`) THEN the receiver SHALL allow an inline reply. WHERE it has other action buttons THEN the receiver SHALL show them. Notifications from desktop hosts SHALL be display-only in this release.
5. WHEN a mirrored notification is dismissed on either device THEN the system SHALL dismiss it on the other device, if the user turned on dismissal sync.
6. Notification contents SHALL NOT be stored on the receiver after they are dismissed or after 24 hours.

## Requirement 19: Shared mouse and keyboard across desktops

**User story:** As a user with several computers on my desk, I want one mouse and keyboard to move between them, as if they were one computer.

1. The desktop app SHALL provide a layout editor where the user places the screens of paired desktops next to each other.
2. WHEN the cursor reaches a screen edge that the layout links to another desktop THEN the system SHALL hide the local cursor, capture local mouse and keyboard input, and send it to that desktop, with the cursor entering at the matching position.
3. The system SHALL capture local input with the native API: low-level input hooks on Windows, a `CGEventTap` on macOS (needs the Input Monitoring and Accessibility permissions), XInput2 on X11, and the InputCapture portal on Wayland.
4. A user-configurable hotkey SHALL return control to the local desktop, and another SHALL lock the cursor to the current screen.
5. WHEN control moves to another desktop THEN the clipboard SHALL move with it, following R7.
6. Input latency SHOULD be under 30 ms (95th percentile) on the LAN.
7. Dragging files across a screen edge to copy them SHOULD be supported in a later version.

## Requirement 20: Phone or tablet as a second display

**User story:** As a user, I want to use my tablet or phone as an extra monitor for my desktop.

1. The desktop SHALL create a virtual display that matches the Android device's resolution and pixel density, and SHALL stream it to that device with the R12 pipeline.
2. The system SHALL create virtual displays with: the ScreenCast portal "virtual" source type on Linux (GNOME and KDE), an Indirect Display Driver (IddCx) on Windows, and the virtual display API on macOS (a private API, see design §18.4).
3. Touch on the Android device SHALL become pointer input on the virtual display, and a stylus SHALL become pen input where the host supports it.
4. WHEN the stream stops THEN the system SHALL remove the virtual display, so windows move back to real screens.

## Requirement 21: Remote terminal

**User story:** As a user, I want to open a shell on another of my computers without setting up SSH.

1. A paired device with the terminal permission SHALL be able to open an interactive terminal on a desktop host, running as the host's logged-in user and never with elevated rights.
2. The terminal permission SHALL default to Deny (R5.2). Choosing Allow SHALL show a warning and need one extra confirmation. The user SHALL be able to change the permission only on the host itself, not from a remote device.
3. Each terminal session SHALL follow R5.3 (Always Ask) and R5.4 (indicator and Stop) unless the host's terminal permission is Allow.
4. The host SHALL use a pseudo-terminal: `openpty` on macOS and Linux, ConPTY on Windows.
5. The desktop and Android apps SHALL provide a terminal view with resizing, copy and paste, and on Android an extra row of keys (Esc, Tab, Ctrl, arrows).
6. The activity log SHALL record when a session starts and ends, and SHALL NOT record what was typed or shown.

## Requirement 22: Remote DevTools and ADB

**User story:** As a web or Android developer, I want to debug my phone from my desktop over Wi-Fi.

1. WHERE the user has turned on Wireless debugging on the Android device THEN UrDevice SHALL help the desktop's `adb` pair and connect to it, and SHALL carry the ADB connection over the encrypted session.
2. Once `adb` is connected, the desktop SHALL list the phone's Chrome tabs and debuggable WebViews and open DevTools for a selected one.
3. Between desktops, the user SHALL be able to share a browser's remote-debugging port (for example 9222) using R13 localhost sharing.
4. The app SHALL explain that Developer options and Wireless debugging must be turned on by the user, because Android does not let apps turn them on.

## Requirement 23: Port forwarding profiles

**User story:** As a developer, I want my usual port forwards to start on their own, so that my phone always reaches my dev servers.

1. The user SHALL be able to save a profile: host device, host ports, viewer devices, and the local port on each viewer.
2. WHEN all devices in a profile are connected and the profile is enabled THEN the system SHALL start its tunnels automatically.
3. The user SHALL be able to set up a forward from either side: from the viewer ("reach my desktop's :3000") or from the host ("give my phone my :8080", like `adb reverse`). The host SHALL still approve sharing its ports, following R13.5.
4. Each profile SHALL be turned on or off with one tap.

## Requirement 24: HTTP request inspector

**User story:** As a developer, I want to see the HTTP requests going through a localhost tunnel, so that I can debug the mobile view of my site.

1. The user SHALL be able to turn on inspection for a tunnel. Inspection SHALL be off by default.
2. WHILE inspection is on THEN the system SHALL record, for HTTP/1.1 and WebSocket traffic: method, URL, status, headers, timing, size, and bodies up to 1 MB.
3. The system SHALL hide the values of `Authorization`, `Cookie`, and `Set-Cookie` headers by default, with a per-session option to show them.
4. Records SHALL be kept in memory only and SHALL be cleared when the tunnel or inspector stops.
5. The inspector SHALL let the user filter records and SHOULD let the user resend a request.
6. Encrypted traffic (TLS to the dev server) and HTTP/2 SHALL be shown as raw TCP connections without request details.

## Requirement 25: Find my device

**User story:** As a user, I want to make a lost phone or laptop ring, even if it's on silent.

1. A paired device SHALL be able to make another device ring at full volume. On Android, it SHALL use the alarm audio stream and, where the user granted Do Not Disturb access, SHALL ring during Do Not Disturb.
2. The ringing device SHALL show a full-screen message, and the user SHALL be able to stop the ringing from either device.
3. The app SHALL show when each device was last seen and on which network, and SHALL make clear that it can only reach devices on the LAN or in Bluetooth range.

## Requirement 26: Remote file browser and search

**User story:** As a user, I want to browse and search another device's files without sending them first.

1. A paired device with the file browsing permission SHALL be able to list folders and files on a host, inside folders the host user has chosen to share. The default on desktop is the home folder. On Android, the user picks folders through the system folder picker.
2. The host SHALL refuse any path outside the shared folders, including through symbolic links.
3. The user SHALL be able to search shared folders by file name, with results shown as they are found.
4. The user SHALL be able to download files and see image thumbnails. Uploading, renaming, and deleting SHALL need the separate file changes permission. It defaults to Deny, and choosing Allow SHALL show a warning and need one extra confirmation (R5.2).
5. Downloads SHALL use the R8 transfer mechanism, including progress and hash checks.

## Requirement 27: Shared clipboard history

**User story:** As a user, I want to find something I copied earlier on any of my devices.

1. The system SHALL keep the last 50 clipboard items (adjustable) and sync the history to devices that have the clipboard permission.
2. The user SHALL be able to search and pin items, and paste any item on the current device.
3. Items SHALL expire after 24 hours unless pinned. The user SHALL be able to change the expiry time.
4. Sensitive items (R7.3) SHALL never be stored. The history SHALL be encrypted on disk with a key kept in secure storage (R1.2).
5. The user SHALL be able to clear the history on all devices at once.

## Requirement 28: Lock the desktop with the phone

**User story:** As a user, I want my computer to lock itself when I walk away with my phone.

1. WHEN proximity lock is on and the paired phone stays out of range for a set time (default 30 s), based on Bluetooth signal strength or a lost connection, THEN the desktop SHALL lock its screen.
2. The desktop SHALL lock with: `LockWorkStation` on Windows, `loginctl lock-session` or the screensaver D-Bus API on Linux, and display sleep with "require password immediately" on macOS (see design §18.12).
3. The phone SHALL provide a button to lock a paired desktop immediately.
4. The system SHALL NOT lock while the user is presenting or sharing the screen, unless the user turns this exception off.
5. Unlocking the desktop with the phone is out of scope for this release, because it needs OS sign-in plug-ins and a separate security review.

## Requirement 29: SMS and calls from the desktop

**User story:** As a user, I want to read and send text messages and handle calls from my desktop.

1. The desktop SHALL show SMS conversations and SHALL be able to send SMS through the phone.
2. WHEN a call comes in THEN the desktop SHALL show the caller and SHALL offer Answer, Decline, and Silence. In this release, call audio stays on the phone.
3. Because Google Play restricts SMS and call-log permissions, these features SHALL be in a separate "full" build of the Android app, distributed outside Google Play (GitHub Releases, F-Droid). The Play build SHALL offer replies to messaging apps through notification mirroring (R18.4) instead.
4. Messages cached on the desktop SHALL be encrypted on disk and deleted when the phone is unpaired.

## Requirement 30: Automations

**User story:** As a user, I want UrDevice to act on its own in common situations, so that I don't repeat the same steps.

1. The user SHALL be able to create rules made of a trigger and one or more actions.
2. Triggers SHALL include: a device connects or disconnects, the device joins a chosen Wi-Fi network, a desktop app starts (for example a slide-show app), a time of day, and a battery level.
3. Actions SHALL include: turning a feature on or off, starting a port-forwarding profile, locking a desktop, showing a notification on another device, and offering to open Mousepad or Presenter on the phone.
4. Rules SHALL run on the device that owns them. Actions on another device SHALL go through that device's permission checks.
5. A "run command" action SHALL exist on desktop only, SHALL be off until the user turns it on in settings, and SHALL run as the logged-in user.
6. WHERE Android blocks an action from the background (opening a screen, reading the Wi-Fi name without location permission) THEN the system SHALL show a notification the user can tap instead, or SHALL explain which permission is needed.

## Requirement 31: Use another device as a speaker

**User story:** As a user, I want sound from one of my devices to play on another paired device, so that I can switch speakers quickly without extra setup.

1. The source device (Host) SHALL be able to send its system audio to any paired target device. Host and target MAY each be Windows, macOS, Linux, or Android.
2. The audio SHALL travel over the existing encrypted session. No new connection or pairing step SHALL be needed.
3. The `audio.out.v1` permission SHALL follow R5: the target's setting (Allow, Always Ask, or Deny) applies to every paired source. WHEN it is Allow THEN audio SHALL play at once, with no prompt. Its default SHALL be chosen when the feature is built (R5.2).
4. The user SHALL be able to change `audio.out.v1` at any time, and a change SHALL take effect within 1 second (R5.5).
5. WHILE a device is playing audio from another device THEN the target SHALL show a visible indicator naming the source and a one-click "Stop" control.
6. The target SHALL keep its own volume control, and the user SHALL be able to mute a source from the target.
7. WHERE the host OS cannot capture system audio THEN the UI SHALL say so and explain what the user must enable.
8. The system SHALL record each start and stop in the activity log (device, time), without audio content.
