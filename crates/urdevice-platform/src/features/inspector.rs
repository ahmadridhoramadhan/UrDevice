//! HTTP request inspector (#34): desktop OS adapters.
//!
//! Slot for the inspector feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`inspector/mod.rs`, `inspector/linux.rs`, ...).
