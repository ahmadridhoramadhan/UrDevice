# UrDevice: security rules

These rules apply to every change. The full model is in section 15 of [design.md](../specs/urdevice/design.md).

- All traffic between devices is end-to-end encrypted and pinned to the paired device's key. Never add a plain-text or unauthenticated path.
- Check permissions in the core (`PermissionEngine`), not only in the UI. A capability that is Not set, Ask, or Deny must not run.
- Never log secrets, clipboard content, file contents, or audio. Logs go through the redaction layer.
- Treat every network message, QR payload, mDNS and UDP announcement, and file name as hostile: enforce size limits, validate, rate-limit.
- File transfer and the file browser must clean paths and refuse anything outside the chosen folder, including through symbolic links.
- Localhost sharing connects to loopback only, and only to ports the user shared with that device.
- Remote terminal and file-change permissions can only be changed on the device itself, never by a remote request.
- Store keys in the OS keystore. Never commit keys, tokens, `.env` files, or keystores.
- Any feature that sees or controls another device (input, screen, audio, terminal) shows a visible indicator and a Stop button on the device being affected.
