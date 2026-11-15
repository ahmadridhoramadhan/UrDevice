# Onboarding: layout

The first-launch flow, for desktop and Android. This is a layout mockup, not final art. Open [`onboarding.html`](onboarding.html) in a browser. It has no scripts, so it also works in file viewers. Click the step numbers. In step 6, click a device in the list.

The look is "Frankenstein": dark background, green glow, dashed "stitch" borders. Colors in the mockup are placeholders.

## Steps

| # | Screen | Desktop | Phone |
|---|---|---|---|
| 1 | Welcome | Name this device | Name this device |
| 2 | Device key | Create the key, show the fingerprint | Same |
| 3 | How it connects | Same Wi-Fi only, no server | Same |
| 4 | You decide | First-use prompt explained: Allow always, Ask every time, Deny | Same |
| 5 | Pair with a QR code | Shows a QR code. Skip for now. "Nearby devices instead" | Camera scans the QR code. Camera access is asked only here |
| 6 | Nearby devices | List of nearby devices. Click one and its 6-digit code opens under that row | Shows the pairing request and the same code. "It matches" |

## Decisions

- No account and no login.
- Pairing can be skipped. The app still opens.
- Nearby devices: pairing starts from a clickable list, not by typing a code. The 6-digit code is shown on both devices and the user confirms it matches (R3.5). This stops a stranger on the same Wi-Fi from posing as a known device.
- The list rows have no dropdown arrow. A click opens the code under the row. (The mockup cannot close it again. The real app should.)
- Camera access (QR) and Nearby devices access (Android) are asked only on the step that needs them (R15.2).

## Spec links

R1 device identity, R2 discovery, R3 pairing (QR and code), R5.2 first-use prompt, R15.2 Android permissions. Tasks T2.1 and T2.3.

## Not decided yet

- Final colors, fonts, icons, and the logo placement.
- The Android permission explanation screens (camera, notifications, nearby devices) as separate screens.
- Empty state when no device is nearby.
