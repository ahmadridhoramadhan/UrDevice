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

## Webcam: other apps do not list "UrDevice Camera"

The phone camera always shows in its UrDevice window. To use it in a call app, the desktop needs a virtual camera.

- **Linux:** needs the `v4l2loopback` kernel module, set up once with root. On Fedora it comes from RPM Fusion (`akmod-v4l2loopback`); on Ubuntu from `v4l2loopback-dkms`. Then load it:

  ```sh
  sudo modprobe v4l2loopback devices=1 video_nr=10 card_label="UrDevice Camera" exclusive_caps=1
  ```

  **The camera is listed as "OBS Virtual Camera"?** The Fedora package of `v4l2loopback` loads the module at boot with that card label (`/usr/lib/modprobe.d/98-v4l2loopback.conf`), and UrDevice uses the loopback device it finds. OBS does not have to be installed. The label is fixed when the module loads, so UrDevice cannot change it. To get your own name, reload the module with yours (close every app that uses the camera first), and keep it with a file that sorts after the package's:

  ```sh
  sudo modprobe -r v4l2loopback
  sudo modprobe v4l2loopback devices=1 video_nr=10 card_label="UrDevice Camera" exclusive_caps=1
  echo 'options v4l2loopback devices=1 video_nr=10 card_label="UrDevice Camera" exclusive_caps=1' | sudo tee /etc/modprobe.d/99-urdevice.conf
  ```

  To load it at every start, put `v4l2loopback` in `/etc/modules-load.d/urdevice.conf` and the options in `/etc/modprobe.d/urdevice.conf` (`options v4l2loopback devices=1 video_nr=10 card_label="UrDevice Camera" exclusive_caps=1`). With Secure Boot on, the module needs a signing key. The camera shows up in other apps only while a phone camera window is streaming.
- **Windows and macOS:** there is no virtual camera yet (Windows 11 needs a registered media source, macOS a signed Camera Extension). The camera shows in its UrDevice window only.
- **Picture turned the wrong way:** hold the phone sideways, or press **Rotate** in the camera window.

## Webcam: the microphone

Linux only. The phone microphone appears as "UrDevice Microphone" (PipeWire) while the microphone switch is on, and goes away when it is off. It needs `pw-cat` (package `pipewire-utils`) and `pactl` (package `pulseaudio-utils`). If you hear nothing, check that the phone allowed the microphone and that the microphone is not muted in the app. On Windows and macOS the microphone is not available yet, because a virtual audio driver is needed.

## Still stuck

Open an issue at <https://github.com/ahmadridhoramadhan/UrDevice/issues> with your OS and version, how both devices are connected, and the steps you tried. Do not include pairing codes or screenshots of QR codes.
