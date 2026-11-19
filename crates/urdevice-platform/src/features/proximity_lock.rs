//! Lock the desktop with the phone (#38): desktop OS adapters.
//!
//! Slot for the proximity_lock feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`proximity_lock/mod.rs`, `proximity_lock/linux.rs`, ...).
