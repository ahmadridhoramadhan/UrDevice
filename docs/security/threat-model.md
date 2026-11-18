# UrDevice threat model

This document expands design §15 for what Release 1 issue #2 builds: identity, discovery, pairing, the encrypted session, and permissions. Feature services (clipboard, files, input, screen, localhost) add their own rows when they land.

## Assets

| Asset | Where it lives |
|---|---|
| Identity private key (Ed25519) | OS keystore: Keychain, Credential Manager (DPAPI), Secret Service, or an Android Keystore–wrapped blob. Fallback: an encrypted file, see "Known limits" |
| Trust-store MAC key | Same place as the identity key |
| Trust store (paired devices, permissions, activity log) | SQLite file in the app data directory, 0600 on desktop, app-private on Android, excluded from Android backups |
| Pairing token (16 bytes) | Memory only, single use, at most 120 s |
| Permission decisions | Made on the host by `node::net::handle_request`; never taken from the peer |

## Trust boundaries

1. **The network.** Everything from the LAN is hostile: UDP announcements, mDNS records, QUIC packets, and control messages.
2. **The QR code.** Its text comes from a camera and may be crafted.
3. **Local files.** Another process of the same user can edit the trust store file.
4. **The peer.** A paired device may be lost, stolen, or running modified code.

## Threats and mitigations

| # | Threat | Mitigation | Where |
|---|---|---|---|
| T1 | A device on the same Wi-Fi connects without pairing | The server's client-certificate verifier accepts a key only if it is paired or the Pair screen is open; TLS fails otherwise. Hidden mode and lockout close the pairing window too | `transport::PinnedClientVerifier`, `Inner::update_policy` |
| T2 | MITM while pairing by QR | The scanner pins the Device ID from the QR code (`Expect::Device`). The proof is HMAC-SHA256(token, TLS exporter ‖ fp_host ‖ fp_scanner), so it is bound to this TLS session and both keys; the raw token never crosses the network | `pairing::pair_proof`, `net::qr_scan_inner` |
| T3 | Replay of a QR code or a captured proof | The token is taken out of the Pair screen on the first attempt, whether it succeeds or not, and a new QR is shown. A captured proof is useless in another TLS session because the exporter differs | `Inner::take_token`; test `qr_token_is_single_use_and_wrong_key_is_refused` |
| T4 | MITM while pairing by code | Both screens show `u32(exporter("EXPORTER-urdevice-sas", 4)) mod 10^6`; a MITM ends up with two TLS sessions and two different codes. Nothing is stored until both users press "Codes match". There is no way to type a code | `net::code_confirm`; test `code_pairing_needs_both_confirmations` |
| T5 | Guessing the code or brute-forcing proofs | 5 failures in 5 minutes lock pairing for 5 minutes; while locked, unknown keys fail in TLS | `pairing::Lockout`; test `lockout_after_five_failures` |
| T6 | Spoofed announcements | Announcements only add addresses to try. A matching short ID still needs the full pinned key in TLS. Packets are capped at 512 bytes, parsed strictly, and limited to 20 per second per source | `discovery::parse_announce`, `RateLimiter` |
| T7 | Malformed or oversized control messages | Frames over 1 MiB are refused before allocation; unknown or out-of-place messages close the session; 3 violations in 10 minutes pause reconnects to that peer. Names and platform strings are cleaned (no control or bidi characters, 64 chars) | `urdevice_proto::frame_len`, `net::handle_msg`, `pairing::clean_name` |
| T8 | A modified peer ignores the host's Deny | The host checks every `ActivityRequest` with its own settings. `PermissionState` only draws the peer's tiles | `net::handle_request`; test `qr_pairing_then_permissions` |
| T9 | Silent use of Always Ask features | The prompt runs on the host and denies after 30 s. Input and screen activities show a banner and a notification with Stop on the host; Stop and switching to Deny end them at once | `net::ask`, `Inner::stop_activities` |
| T10 | A remote request turns on terminal or file changes | There is no protocol message that changes a setting. Locally, `set_permission` refuses Allow for these two without the extra confirmation, and onboarding can't set them | `Node::set_permission`; test `terminal_allow_needs_confirmation` |
| T11 | Lost or stolen paired device | Forget on any other device deletes the pairing on both sides when connected; when the lost device is offline, the host refuses its key from then on. Remote control and screen sharing default to Always Ask | `Node::forget`; test `unpair_closes_and_blocks` |
| T12 | Local tampering with the trust store | Paired-device, permission, and shared-port rows carry an HMAC keyed from secure storage. A row with a bad MAC is ignored: the device is not trusted and the permission reads as Deny | `store::TrustStore`; test `tampered_rows_are_ignored` |
| T13 | Logs leak data | The log formatter prints only allow-listed field names; other fields become `[redacted]`. Logs carry the 8-byte short ID, never keys or tokens | `logging::RedactingFields` |
| T14 | Duplicate connections used to confuse state | When two sessions to the same peer exist, the one started by the lower Device ID wins; the other closes with `DUPLICATE` | `net::run_session` |
| T15 | Desktop camera misuse | The camera runs only while the scan window is open; frames stay in memory and are dropped after decoding | `urdevice-desktop/src/camera.rs` |

## Known limits

- **Encrypted-file fallback on desktop.** When no OS keystore is reachable (for example a Linux session without a Secret Service), secrets go to `secrets/*.bin`, sealed with ChaCha20-Poly1305 under a key derived from the machine ID and user name. This stops copying the file to another machine. It does not stop another process of the same user on the same machine. `URDEVICE_NO_KEYRING=1` forces this path for testing.
- **Discovery reveals presence.** The display name, platform, and short ID are visible on the LAN unless hidden mode is on (R2.3, R2.7).
- **Unpair while the other side is offline.** The other device keeps its record until it is forgotten there too. It can't connect, because the host refuses its key.
- **No external review yet.** T9.4 tracks the review before 1.0.
