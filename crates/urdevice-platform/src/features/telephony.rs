//! SMS and calls from the desktop (#29): desktop OS adapters.
//!
//! Slot for the telephony feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`telephony/mod.rs`, `telephony/linux.rs`, ...).
