# Permissions on each OS

UrDevice asks for an OS permission only when you first use the feature that needs it. Denying one disables only that feature.

## Windows

- **Firewall (needed for pairing).** Windows blocks inbound UDP to unknown programs. UrDevice needs inbound UDP for QUIC (default port 47811, another port if that one is taken) and mDNS discovery.
  - The `.msi` installs a rule for `urdevice.exe`: inbound, UDP, any local port, **Private** networks only.
  - With the `.zip`, allow UrDevice on **Private networks** in the prompt on first launch, or add the rule yourself in an elevated terminal:
    ```
    netsh advfirewall firewall add rule name="UrDevice (UDP)" dir=in action=allow program="C:\path\to\urdevice.exe" protocol=UDP profile=private
    ```
  - If your Wi-Fi is classed as **Public**, no rule applies. Set it to Private in Settings > Network & internet > Wi-Fi > your network.
- **Camera** (optional): scanning a QR code with a webcam. Settings > Privacy & security > Camera.

## macOS

- **Local Network**: allow when asked, or discovery and pairing will not see other devices (System Settings > Privacy & Security > Local Network).
- **Accessibility**: needed to let another device control this Mac's keyboard and pointer (System Settings > Privacy & Security > Accessibility).
- **Screen Recording**: needed to share this Mac's screen (System Settings > Privacy & Security > Screen & System Audio Recording). macOS requires restarting the app after you grant it.
- **Camera** (optional): scanning a QR code.
- Unsigned build: right-click the app and choose **Open** the first time. macOS support has not been tested on real hardware yet.

## Linux

- **Wayland (GNOME, KDE Plasma):** screen sharing and remote input go through the XDG desktop portals (ScreenCast and RemoteDesktop). The desktop shows a prompt the first time. Pick the screen to share and allow remote control. The choice can be remembered for later sessions.
- **Sway, Hyprland and other wlroots compositors:** ScreenCast works, but there is no RemoteDesktop portal. Remote input then needs `/dev/uinput`. The app checks access and offers a udev rule when it is missing. Review the rule, install it as root, and log in again.
- **X11:** no portal prompts. Screen and input use X11 directly.
- Pairing needs UDP 47810 (discovery) and 47811 (QUIC) open if you run a firewall such as `ufw`: `sudo ufw allow from 192.168.0.0/16 to any port 47810,47811 proto udp` (adjust to your subnet).

## Android

Open the matching system screen from UrDevice's permission page. Grant only what you use.

- **Nearby devices / local network, notifications:** allow when asked. Android 17 gates LAN traffic behind a local-network permission, and the foreground notification keeps the connection alive.
- **Camera:** scan a QR code to pair.
- **Accessibility service ("UrDevice remote control"):** lets another device tap, swipe and type on this phone. Settings > Accessibility > Installed apps. Off by default. It can be turned off at any time.
- **Input method (keyboard):** needed for text entry from another device. Settings > System > Keyboard > On-screen keyboard, enable UrDevice, then pick it in the keyboard switcher.
- **Notification access:** lets UrDevice show this phone's media controls and notifications on your other devices. Settings > Notifications > Device & app notifications.
- **Screen capture (MediaProjection):** Android asks every time you start sharing the phone's screen. Choose **Start now** in the system dialog.
- **Photos and media:** only when you send files from the gallery.
- **Install unknown apps:** needed once to install the `.apk` outside a store. Settings > Apps > Special app access.
