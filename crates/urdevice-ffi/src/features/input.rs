//! Remote control: pointer, keyboard, text, and presenter (#18, #19): bindings for the Android app.
//!
//! Slot for the input feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `input_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
