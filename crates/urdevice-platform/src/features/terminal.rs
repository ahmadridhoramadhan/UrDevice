//! Remote terminal (#32): desktop OS adapters.
//!
//! Slot for the terminal feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`terminal/mod.rs`, `terminal/linux.rs`, ...).
