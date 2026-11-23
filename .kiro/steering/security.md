---
inclusion: always
---

# UrDevice: security rules

These rules apply to every change. The full model is in section 15 of [design.md](../specs/urdevice/design.md).

- All traffic between devices is end-to-end encrypted and pinned to the paired device's key. Never add a plain-text or unauthenticated path.
- Check permissions in the core (`PermissionEngine`), not only in the UI. A capability set to Deny must never run, and one set to Always Ask runs only after the user approves it on this device. Never trust the peer's copy of the settings (`PermissionState` is for drawing the UI only).
- Permission settings apply to every paired device, so a lost device gets whatever is set to Allow. Keep Remote control and Screen sharing at Always Ask by default, and keep the indicator and Stop button working.
- Never log secrets, clipboard content, file contents, keystrokes, audio, or camera frames. Logs go through the redaction layer.
- Treat every network message, QR payload, mDNS and UDP announcement, and file name as hostile: enforce size limits, validate, rate-limit.
- QR pairing works only while the Pair screen is open. A code pairing asked for while it is closed needs the user's Accept on this device first (one question at a time, 5 a minute, 60 s quiet after a no). Hidden mode and the lockout still refuse everything. QR tokens are single use and expire after 120 s. Pairing codes are compared by eye; never add a way to type one in.
- The desktop camera is on only while scanning a QR code. Frames stay in memory and are never stored or sent.
- File transfer and the file browser must clean paths and refuse anything outside the chosen folder, including through symbolic links.
- Localhost sharing connects to loopback only, and only to ports the host shared. "Expose on LAN" is off by default; "Always on" needs an extra confirmation and a persistent indicator.
- Remote terminal and file-change permissions can only be changed on the device itself, never by a remote request.
- Store keys in the OS keystore. Never commit keys, tokens, `.env` files, or keystores.
- Any feature that sees or controls another device (input, screen, audio, terminal) shows a visible indicator and a Stop button on the device being affected.
