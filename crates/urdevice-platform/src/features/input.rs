//! Remote control: pointer, keyboard, text, and presenter (#18, #19): desktop OS adapters.
//!
//! Slot for the input feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`input/mod.rs`, `input/linux.rs`, ...).
