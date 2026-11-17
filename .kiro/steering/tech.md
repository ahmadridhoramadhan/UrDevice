---
inclusion: fileMatch
fileMatchPattern: ["**/*.rs", "**/Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "deny.toml", "**/*.slint", "android/**", ".github/workflows/**"]
---

# UrDevice: technology and conventions

## Stack

- One Rust core shared by all platforms. Rust 1.95.0 (pinned in `rust-toolchain.toml`), edition 2024.
- Desktop app: Slint, in `crates/urdevice-desktop` (binary `urdevice`).
- Android app: native Kotlin with Jetpack Compose in `android/`, using the Rust core through UniFFI (`crates/urdevice-ffi`).
- Transport: QUIC (`quinn`). Discovery: mDNS plus UDP broadcast. Protocol messages: length-prefixed protobuf (1 MiB limit).

## Crates

| Crate | Purpose |
|---|---|
| `urdevice-proto` | protobuf schemas |
| `urdevice-core` | identity, trust store, session, permissions, feature services, platform traits |
| `urdevice-platform` | desktop adapters (Windows, macOS, Linux X11 and Wayland) |
| `urdevice-media` | capture and encode |
| `urdevice-ffi` | UniFFI bindings for Android |
| `urdevice-desktop` | Slint desktop app |

Platform-specific code lives behind traits defined in `urdevice-core`. Do not put OS calls in the core.

## Protocol notes

- Permission settings are looked up by capability only, not by device (design §7.4). The host sends `PermissionState` after the handshake and within 1 s of a change; the peer uses it only to draw the UI.
- New session messages from the UI review: `PermissionState`, `DeviceStatus` (battery), `Disconnect`, `MediaSubscribe`, `MediaState`, `NotifSubscribe` (design §6.3, §6.5, §9.4, §18.2).
- Desktop QR scanning uses `nokhwa` for the camera and `rqrr` for decoding (design §7.5).

## Before every commit

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI also runs `cargo build --workspace --locked`, `cargo deny check`, and `cargo audit`, and builds the Android debug APK. CI must be green on Linux, macOS, and Windows before a pull request is merged.

## Rust style

- `rustfmt.toml` sets max width 100. Use `Result` and typed errors, not `unwrap` or `expect` outside tests.
- Keep parsers for untrusted input (network messages, QR payloads, file names) small, size-limited, and covered by tests.
- Prefer property-based tests (`proptest`) for parsers and path handling. Examples: path cleaning for file transfer, and file-browser paths that must never leave a shared folder.

## Dependencies and licenses

- The project is MIT. New dependencies must pass `cargo deny` (`deny.toml`).
- Slint is used under its royalty-free license. Keep the attribution (AboutSlint or badge) in the app and the README.
