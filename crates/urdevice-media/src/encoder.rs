//! The H.264 encoder abstraction (design §10, R12.3).
//!
//! Every back-end is set up for low latency: no B-frames, one reference
//! frame chain, constant bitrate, a keyframe every two seconds or when asked.
//! [`open`] tries the hardware back-end for this OS first and falls back to
//! `openh264`. [`AutoEncoder`] also falls back at run time: a hardware
//! encoder that fails mid-stream is replaced by the software one, and the
//! next frame is a keyframe.

use crate::MediaError;
use crate::convert::BgraFrame;

/// What to encode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderSettings {
    /// Encoded width (even).
    pub width: u32,
    /// Encoded height (even).
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
    /// Constant bitrate, bits per second.
    pub bitrate_bps: u32,
}

impl EncoderSettings {
    /// Frames between two keyframes: two seconds (design §10).
    pub fn keyframe_interval(&self) -> u32 {
        self.fps.max(1) * 2
    }

    /// Refuse odd or empty sizes and a zero rate.
    pub fn validate(&self) -> Result<(), MediaError> {
        if self.width < 16
            || self.height < 16
            || !self.width.is_multiple_of(2)
            || !self.height.is_multiple_of(2)
            || self.width > 8192
            || self.height > 8192
        {
            return Err(MediaError::BadSize(self.width, self.height));
        }
        if self.fps == 0 || self.bitrate_bps == 0 {
            return Err(MediaError::Encoder("zero frame rate or bitrate".into()));
        }
        Ok(())
    }
}

/// One encoded access unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedUnit {
    /// H.264 Annex B NAL units with start codes.
    pub data: Vec<u8>,
    /// True for an IDR frame.
    pub keyframe: bool,
    /// Presentation time in microseconds, as passed to `encode`.
    pub pts_us: u64,
}

/// An H.264 encoder.
pub trait VideoEncoder: Send {
    /// Back-end name for logs and the UI, for example `openh264`.
    fn name(&self) -> &'static str;

    /// True for a hardware encoder.
    fn hardware(&self) -> bool;

    /// The settings it was opened with.
    fn settings(&self) -> EncoderSettings;

    /// Encode one frame, scaling it to the encoder's size. `keyframe`
    /// forces an IDR. Returns what the encoder produced, which for a
    /// hardware encoder may lag a frame behind, and may be empty when the
    /// rate control skipped the frame.
    fn encode(
        &mut self,
        frame: &BgraFrame,
        pts_us: u64,
        keyframe: bool,
    ) -> Result<Vec<EncodedUnit>, MediaError>;
}

/// Which back-ends [`open`] may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Hardware where this OS has one, software otherwise.
    Auto,
    /// Only `openh264`.
    Software,
}

/// Open an encoder: the OS hardware encoder when `backend` allows it and one
/// is present (Windows: Media Foundation), otherwise `openh264`.
pub fn open(
    settings: EncoderSettings,
    backend: Backend,
) -> Result<Box<dyn VideoEncoder>, MediaError> {
    settings.validate()?;
    if backend == Backend::Auto {
        #[cfg(target_os = "windows")]
        match crate::mf::MfEncoder::open(settings) {
            Ok(e) => return Ok(Box::new(e)),
            Err(e) => tracing::info!(error = %e, "no hardware H.264 encoder; using openh264"),
        }
    }
    Ok(Box::new(crate::soft::SoftEncoder::open(settings)?))
}

/// An encoder that replaces a failing hardware back-end with `openh264`.
pub struct AutoEncoder {
    inner: Box<dyn VideoEncoder>,
    backend: Backend,
    force_key: bool,
}

impl AutoEncoder {
    /// Open with [`open`].
    pub fn open(settings: EncoderSettings, backend: Backend) -> Result<Self, MediaError> {
        Ok(Self {
            inner: open(settings, backend)?,
            backend,
            force_key: false,
        })
    }

    /// Re-open with new settings (the ladder moved). The next frame is a
    /// keyframe.
    pub fn reconfigure(&mut self, settings: EncoderSettings) -> Result<(), MediaError> {
        if settings == self.inner.settings() {
            return Ok(());
        }
        self.inner = open(settings, self.backend)?;
        self.force_key = true;
        Ok(())
    }

    /// The back-end in use.
    pub fn name(&self) -> &'static str {
        self.inner.name()
    }

    /// The current settings.
    pub fn settings(&self) -> EncoderSettings {
        self.inner.settings()
    }

    /// Encode, falling back to software on a hardware error.
    pub fn encode(
        &mut self,
        frame: &BgraFrame,
        pts_us: u64,
        keyframe: bool,
    ) -> Result<Vec<EncodedUnit>, MediaError> {
        let key = keyframe || std::mem::take(&mut self.force_key);
        match self.inner.encode(frame, pts_us, key) {
            Ok(u) => Ok(u),
            Err(MediaError::BadFrame) => Err(MediaError::BadFrame),
            Err(e) if self.inner.hardware() => {
                tracing::warn!(error = %e, encoder = self.inner.name(), "hardware encoder failed; using openh264");
                let settings = self.inner.settings();
                self.backend = Backend::Software;
                self.inner = open(settings, Backend::Software)?;
                self.inner.encode(frame, pts_us, true)
            }
            Err(e) => Err(e),
        }
    }
}
