# Troubleshooting

## Pairing says the device is unreachable, or a device never appears

Most often a firewall is blocking inbound UDP on the receiving device.

1. Put both devices on the same Wi-Fi or LAN. A guest network, a different subnet or a VPN usually blocks local discovery. Some routers have "AP/client isolation", which must be off.
2. **Windows:** the network must be **Private**, and `urdevice.exe` needs an inbound UDP allow rule. The `.msi` adds it. Otherwise follow [Windows permissions](permissions.md#windows). This was the cause of the "paired phone cannot reach the PC" report (issue #59).
3. **Linux:** allow UDP 47810 and 47811 from your LAN in `ufw`, `firewalld` or `nftables`.
4. **macOS:** allow **Local Network** for UrDevice, and accept the incoming-connections prompt.
5. Restart UrDevice on both devices and try again. Use **Scan QR** if the list does not show the device.

## Wayland: portal prompts keep appearing or never appear

- The prompt belongs to your desktop, not UrDevice. Look for a screen-share or remote-control dialog and for a screen-sharing indicator in the top bar.
- Make sure `xdg-desktop-portal` and your desktop's backend (`xdg-desktop-portal-gnome`, `-kde`, `-wlr`) are installed and running. After installing, log out and in.
- The saved choice can be cleared: on KDE in System Settings > Permissions (Plasma 6.5 and later), on GNOME by revoking screen sharing in Settings > Privacy & Security.
- On Sway, Hyprland and other wlroots compositors there is no RemoteDesktop portal, so remote input needs `/dev/uinput` access. See [Linux permissions](permissions.md#linux).

## Bluetooth (BLE) banner shown instead of a Wi-Fi connection

The banner means the local network is not reachable and UrDevice fell back to Bluetooth, which is slower and supports fewer features. To go back to the normal connection:

- Join the same Wi-Fi on both devices and check the firewall steps above.
- Turn Bluetooth on and allow the Nearby devices permission if you want the fallback to work.
- Bluetooth fallback between Android and a Mac has not been tested on real hardware yet.

## Android: remote control or typing does nothing

Check that the Accessibility service and the UrDevice keyboard are enabled and selected. See [Android permissions](permissions.md#android). Android may switch the accessibility service off after an app update. Turn it on again.

## Still stuck

Open an issue at <https://github.com/ahmadridhoramadhan/UrDevice/issues> with your OS and version, how both devices are connected, and the steps you tried. Do not include pairing codes or screenshots of QR codes.
