---
inclusion: fileMatch
fileMatchPattern: ["**/*.slint", "crates/urdevice-desktop/**", "android/app/src/main/**", "docs/mockup/**"]
---

# UrDevice: UI rules

## Mockups

- [`docs/mockup/urdevice-layout.html`](../../docs/mockup/urdevice-layout.html): desktop, Android v1 (pages with a back button), and Android v2 (bottom tab bar).
- [`docs/mockup/onboarding.html`](../../docs/mockup/onboarding.html): the first-launch flow, with notes in [`onboarding.md`](../../docs/mockup/onboarding.md).

The mockups are a guide, not a final design. Build the UI to match them. You may add or remove parts when needed, but **ask the owner first** and say why. When a mockup and the spec disagree, the spec wins; tell the owner so the mockup can be fixed.

## Behavior the UI must keep

- Permission labels are exactly **Allow**, **Always Ask**, and **Deny**.
- Feature tiles on a paired device follow that host's `PermissionState`: normal for Allow, a "Needs approval" mark for Always Ask, grey and disabled for Deny ("Turned off by <device>"). Tapping a tile opens the feature view. The gear opens settings for that feature on this device.
- Pairing: "Scan QR" (hidden when there is no camera), this device's QR code with a countdown, and the nearby-devices list where a selected device shows the 6-digit code with "Codes match" and "Cancel".
- Show the connection path and latency, the battery, Disconnect or Reconnect, and Forget device on each paired device. On Bluetooth LE, show the banner and grey out the unavailable features.
- Settings opens from the bottom of the sidebar on desktop, and from a gear icon at the top right on Android.

## Text and look

- All UI text goes through translation (Slint `@tr`, Android `strings.xml`). English is the first language (R16.4). The mockups are written in English, and their text can be used as the English source strings.
- Brand colors: dark green `#0F1F10` and lime `#C6F19D`. Logos are in `assets/`. Support light and dark themes.
- Touch targets are at least 44 px on Android. Respect reduced-motion settings.
