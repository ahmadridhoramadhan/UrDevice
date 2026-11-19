//! Port forwarding profiles (#33): bindings for the Android app.
//!
//! Slot for the port_profiles feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `port_profiles_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
