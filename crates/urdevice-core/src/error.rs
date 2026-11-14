//! Error type shared by the core and the platform adapters.

/// Result alias used across the core.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;

/// Errors returned by the core and by platform adapters.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// The feature is not available on this platform or back-end yet.
    #[error("{feature} is not supported on {platform}")]
    Unsupported {
        /// The feature that was requested, for example `clipboard`.
        feature: &'static str,
        /// The platform or back-end that refused it, for example `linux-wayland`.
        platform: &'static str,
    },
    /// The operating system refused the request because a permission is missing.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// An I/O error from the operating system.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl CoreError {
    /// Shorthand for [`CoreError::Unsupported`].
    pub fn unsupported(feature: &'static str, platform: &'static str) -> Self {
        Self::Unsupported { feature, platform }
    }
}
