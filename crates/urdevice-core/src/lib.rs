//! Shared UrDevice core.
//!
//! The core holds everything that is the same on every device. It talks to the
//! operating system only through the traits in [`platform`]. Desktop adapters
//! live in `urdevice-platform`; on Android the same traits become UniFFI
//! callback interfaces implemented in Kotlin (design §9).

pub mod error;
pub mod platform;

pub use error::{CoreError, Result};

/// The version of the UrDevice core, taken from the crate version.
///
/// Every app shows this value so the desktop and Android builds can be compared.
pub fn core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_version_matches_crate_version() {
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
        assert!(!core_version().is_empty());
    }
}
