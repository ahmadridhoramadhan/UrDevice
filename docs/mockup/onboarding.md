# Onboarding: layout

The first-launch flow, for desktop and Android. This is a layout mockup, not final art. Open [`onboarding.html`](onboarding.html) in a browser. It has no scripts and loads nothing from the internet, so it also works in file viewers. Click the step tabs or the buttons inside the screens.

The look follows the brand colors (dark green `#0F1F10`, lime `#C6F19D`) in a dark theme, with dashed "stitch" borders as a nod to the Frankenstein category. Mockup text is in English, like [`urdevice-layout.html`](urdevice-layout.html), because English is the app's first language (R16.4).

## Steps

> **Desktop and Android (2026-11-24):** one panel, four steps: 1 name, 2 permissions, 3 pair (QR and nearby devices on one screen, with "Skip for now"), 4 done. The "Safe and local" screen and the Device ID are removed from onboarding; the Device ID stays in Settings. The mockup HTML and the table below still show the older six steps.

| # | Screen | Desktop | Android |
|---|---|---|---|
| 1 | Welcome | Name this device | Name this device |
| 2 | Safe and local | Same network, no server, Bluetooth fallback. The device key is already made; shows the Device ID | Same, with Android Keystore |
| 3 | Permissions for other devices | Allow / Always Ask / Deny for each capability, with the defaults from R5.2. No Gallery | Same, plus Gallery |
| 4 | Pair the first device | This device's QR code with a countdown. "Scan QR" opens the camera (hidden when there is no camera). "Pick a device on the network". "Skip for now" | Camera scanner, or "Show this device's QR". "Pick a device on the network". "Skip for now" |
| 5 | Devices on the network | List of nearby devices. Selecting one shows the 6-digit code under it with "Codes match" and "Cancel" | Shows the pairing request with the same code, "Codes match" and "Cancel" |
| 6 | Done | Paired: both devices shown as connected. Skipped: "Ready", with "Pair now" | Same |

In step 5, pairing finishes only after both screens press "Codes match". Try pressing it on one side first: that side waits for the other.

## Decisions

- No account and no login.
- The device key is created automatically on first start (R1.1). Step 2 only explains it and shows the Device ID (R1.4); there is no "Create key" button.
- Step 3 is where the permission settings get their first values. It replaces the earlier first-use prompt. The settings apply to every paired device and can be changed later on "This device" (R5.1, R5.2).
- QR pairing needs no code to confirm (design §7.1). Desktops with a camera can scan too (R3.10).
- Nearby devices: pairing starts from a list, not by typing a code. The 6-digit code is compared by eye on both screens (R3.5).
- Pairing can be skipped. The app still opens.
- Camera access (QR) and Nearby devices access (Android) are asked only on the step that needs them (R15.2).
- The shorter onboarding in `urdevice-layout.html` follows the same order (welcome, safe and local, permissions, pair). This file shows the pairing steps in full.

## Spec links

R1 device identity, R2 discovery, R3 pairing (QR, codes, desktop scanning), R5.1–R5.2 permission settings and defaults, R15.2 Android permissions. Tasks T2.1, T2.3, T2.4, and T2.6.

## Mockup limits

- After "Cancel" in step 5, a "Codes match" press stays remembered. Reload the page to start over.
- Only Pixel has an example phone screen. The other devices in the list show their code only.
- The QR code is a fixed sample pattern, not a real pairing invite.

## Not decided yet

- Final fonts, icons, and illustrations.
- The Android permission explanation screens (camera, notifications, nearby devices) as separate screens.
- Empty state when no device is nearby.
