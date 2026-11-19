//! Localhost sharing and Expose on LAN (#22): desktop OS adapters.
//!
//! Slot for the tunnel feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`tunnel/mod.rs`, `tunnel/linux.rs`, ...).
