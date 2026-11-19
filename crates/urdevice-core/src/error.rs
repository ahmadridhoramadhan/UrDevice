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
    /// The trust store could not be read or written.
    #[error("trust store: {0}")]
    Store(String),
    /// Secure storage (keychain, Keystore, Secret Service) failed.
    #[error("secure storage: {0}")]
    Secret(String),
    /// Key or certificate handling failed.
    #[error("crypto: {0}")]
    Crypto(String),
    /// A network or QUIC error.
    #[error("network: {0}")]
    Network(String),
    /// The peer broke the protocol or sent a malformed message.
    #[error("protocol: {0}")]
    Protocol(String),
    /// A pairing attempt failed. The string is a short reason such as `expired`.
    #[error("pairing failed: {0}")]
    Pairing(String),
    /// Pairing is locked after too many failures (R3.7).
    #[error("pairing is locked for {secs} s")]
    Locked {
        /// Seconds until pairing opens again.
        secs: u64,
    },
    /// The caller passed something invalid, for example a bad QR code.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The device, prompt, or activity does not exist.
    #[error("not found")]
    NotFound,
    /// The other device refused a request: `denied`, `declined`, `timeout`,
    /// `unsupported`, or `offline`.
    #[error("refused: {0}")]
    Refused(String),
    /// The node has stopped.
    #[error("the node has stopped")]
    Stopped,
}

impl CoreError {
    /// Shorthand for [`CoreError::Unsupported`].
    pub fn unsupported(feature: &'static str, platform: &'static str) -> Self {
        Self::Unsupported { feature, platform }
    }
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Store(e.to_string())
    }
}

impl From<urdevice_proto::ProtoError> for CoreError {
    fn from(e: urdevice_proto::ProtoError) -> Self {
        Self::Protocol(e.to_string())
    }
}

impl From<rcgen::Error> for CoreError {
    fn from(e: rcgen::Error) -> Self {
        Self::Crypto(e.to_string())
    }
}

impl From<rustls::Error> for CoreError {
    fn from(e: rustls::Error) -> Self {
        Self::Crypto(e.to_string())
    }
}

impl From<quinn::ConnectionError> for CoreError {
    fn from(e: quinn::ConnectionError) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<quinn::ConnectError> for CoreError {
    fn from(e: quinn::ConnectError) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<quinn::WriteError> for CoreError {
    fn from(e: quinn::WriteError) -> Self {
        Self::Network(e.to_string())
    }
}

impl From<quinn::ReadExactError> for CoreError {
    fn from(e: quinn::ReadExactError) -> Self {
        Self::Network(e.to_string())
    }
}
