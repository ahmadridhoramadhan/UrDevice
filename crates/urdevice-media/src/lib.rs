//! Video for UrDevice screen sharing (design §10, R12.3): pixel conversion,
//! the H.264 encoder abstraction with hardware back-ends and the `openh264`
//! fallback, and the desktop decoder.
//!
//! Frames pass through here and are never stored or logged.

pub mod convert;
pub mod encoder;
#[cfg(target_os = "windows")]
mod mf;
pub mod soft;

pub use convert::BgraFrame;
pub use encoder::{AutoEncoder, Backend, EncodedUnit, EncoderSettings, VideoEncoder};
pub use soft::{H264Decoder, RgbaFrame};

/// Errors from conversion, encoding, and decoding.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// The frame buffer is shorter than its size and stride say.
    #[error("frame buffer does not match its size")]
    BadFrame,
    /// A size that is odd, empty, or too large.
    #[error("unsupported size {0}x{1}")]
    BadSize(u32, u32),
    /// The encoder failed.
    #[error("encoder: {0}")]
    Encoder(String),
    /// The decoder failed.
    #[error("decoder: {0}")]
    Decoder(String),
}
