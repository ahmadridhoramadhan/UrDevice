//! Shared mouse and keyboard across desktops (#30): desktop OS adapters.
//!
//! Slot for the kvm feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`kvm/mod.rs`, `kvm/linux.rs`, ...).
