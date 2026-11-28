---
inclusion: always
---

# UrDevice: product

UrDevice joins a user's own devices (Windows, macOS, Linux, Android) into one network over the local network. There is no cloud server. All devices are equal peers; each feature has a Host (the device that provides or is controlled) and a Controller or Target.

Stack in one line: one Rust core, a Slint desktop app, and a native Kotlin Android app that uses the core through UniFFI. Details are in `tech.md`.

Hackathon: Kiroween 2026, category Frankenstein. Deadline: 4 December 2026 14:00 Pacific (5 December 06:00 WITA). Feature work stops 3 December 06:00 WITA. The last two days are for the video, README, demo, and submission.

The spec is the source of truth: [requirements](../specs/urdevice/requirements.md), [design](../specs/urdevice/design.md), [tasks](../specs/urdevice/tasks.md). When a decision changes, update the requirement first, then the design, then the tasks. Owner decisions are listed with dates in design §17.

## Decisions already made

- **Permissions (R5):** each device has one setting per capability: Allow, Always Ask, or Deny. It applies to every paired device that asks this device. Defaults are set in onboarding (Clipboard and Media Allow; Receive files, Gallery, Remote control, and Screen sharing Always Ask; Localhost Deny). Deny shows the feature as off on the other device. There is no "Not set" state, no first-use prompt, and no separate "default permission" setting. Terminal and file changes default to Deny, and switching them to Allow needs a warning and one extra confirmation.
- **Pairing (R3):** two flows only. QR: one device shows a QR code and the other scans it, with no code to confirm. Desktops with a camera can scan too (R3.10). Nearby devices: both screens show the same 6-digit code and both users tap "Codes match". Codes are compared by eye, never typed in.
- **Connection (R4, R6):** Disconnect keeps the pairing and stops auto-reconnect until Reconnect. The UI shows the connection path, latency, and battery. Bluetooth is an automatic fallback, not a setting. On Bluetooth LE a banner names the features that are unavailable.
- **Media (R11):** source tabs, seek, volume, and mute. No shuffle or repeat.
- **Localhost (R13):** shared ports are offered to every paired device, gated by the Localhost setting. "Expose on LAN" is per port on the host: "1 hour" or "Always on".
- **Notifications (R18, Release 2):** Android → desktop and desktop → Android (Windows and Linux only). On by default, all apps, no picker (owner decision 2026-11-28); the host's permission setting still gates it.
- **Speaker (R31):** audio from a source device plays on a target device over the existing session.
- **UI:** the mockups in `docs/mockup/` are a guide, not a final design. See `ui.md`.
- Demo target: 2 real devices. The app is still built to support many paired devices.
- Not in v1: iOS, use over the internet, relay or NAT traversal.

## Owner rules

- The owner decides goals and scope. Do not add features, delete data, or merge code without the owner's explicit approval.
- Do the review of a pull request only when the owner asks to merge it.
- Review and merge are two steps. After the review, report every finding (blocking and minor, including inconsistencies outside the diff) to the owner and wait for an explicit go. Merge only after that go. Never review and merge in one step.
- Do not claim in docs or the write-up that a Kiro feature was used unless it was.
