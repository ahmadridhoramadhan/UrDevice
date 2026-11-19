//! Remote DevTools and ADB (#35): desktop OS adapters.
//!
//! Slot for the adb feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`adb/mod.rs`, `adb/linux.rs`, ...).
