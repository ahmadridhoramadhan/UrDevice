//! Localhost sharing and Expose on LAN (#22): bindings for the Android app.
//!
//! Slot for the tunnel feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `tunnel_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
