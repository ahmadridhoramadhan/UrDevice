//! Bluetooth fallback (#23): desktop OS adapters.
//!
//! Slot for the bluetooth feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`bluetooth/mod.rs`, `bluetooth/linux.rs`, ...).
