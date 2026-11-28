//! Phone as webcam and microphone (#28, design §18.1): wire messages for `ServiceId::CAMERA` and `ServiceId::MIC`.
//!
//! Owned by the webcam feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/webcam.proto`.
//!
//! **Camera.** `ServiceId::CAMERA` speaks the screen messages (`features::screen`): its "displays"
//! are the phone's cameras and a viewer adds `Control`. Nothing else is needed here for it.
//!
//! **Microphone (`ServiceId::MIC`, T11.5).** The viewer (a desktop) asks for an activity first:
//! the microphone is a watched capability, so every message it sends names that activity. It sends
//! [`Start`]; the host answers with one unidirectional stream of kind [`STREAM_AUDIO`]
//! (`Direction::ToController`) whose header `meta` is an [`AudioInfo`]. The stream carries raw
//! PCM chunks, each prefixed with its length as a big-endian `u32` ([`encode_chunk`],
//! [`chunk_len`]): 16-bit little-endian samples, [`CHANNELS`] channel at [`SAMPLE_RATE`] Hz. A
//! compressed codec (Opus) can replace this later without a new service: [`AudioInfo::codec`]
//! names it.
//!
//! Everything here is untrusted input. The length prefix is checked against [`MAX_CHUNK`] before
//! anything is allocated, and the viewer accepts only the one format it can play.

use prost::Message;

/// Stream kind for the audio stream (host to viewer).
pub const STREAM_AUDIO: u32 = 1;

/// Samples per second.
pub const SAMPLE_RATE: u32 = 48_000;

/// Channels. The phone's microphone is mono.
pub const CHANNELS: u32 = 1;

/// Size of the length prefix in front of every chunk on the audio stream.
pub const CHUNK_PREFIX: usize = 4;

/// Largest chunk, in bytes: 8192 samples, about 170 ms.
pub const MAX_CHUNK: usize = 16 * 1024;

/// Longest error reason, in bytes.
pub const MAX_REASON: usize = 64;

/// The codec name of raw 16-bit little-endian samples.
pub const CODEC_PCM_S16LE: &str = "pcm_s16le";

/// A microphone message.
#[derive(Clone, PartialEq, Message)]
pub struct Mic {
    /// The message.
    #[prost(oneof = "mic::Msg", tags = "1, 2")]
    pub msg: Option<mic::Msg>,
}

/// Nested types for [`Mic`]: `mic::Msg` is the oneof.
pub mod mic {
    /// One microphone message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Msg {
        /// Viewer to host: start the audio stream.
        #[prost(message, tag = "1")]
        Start(super::Start),
        /// Host to viewer: the microphone could not start or stopped.
        #[prost(message, tag = "2")]
        Error(super::MicError),
    }
}

impl From<mic::Msg> for Mic {
    fn from(msg: mic::Msg) -> Self {
        Self { msg: Some(msg) }
    }
}

/// Start the audio stream. A second `Start` in the same activity replaces the first stream.
#[derive(Clone, PartialEq, Message)]
pub struct Start {}

/// The microphone failed or ended on the host.
#[derive(Clone, PartialEq, Message)]
pub struct MicError {
    /// `no_host` (nothing can record here), `consent_declined` (the RECORD_AUDIO permission was
    /// refused), `capture_failed`, or `stopped`.
    #[prost(string, tag = "1")]
    pub reason: String,
}

impl MicError {
    /// Clamp a reason a peer sent.
    pub fn sanitize(&mut self) {
        if self.reason.len() > MAX_REASON {
            let mut end = MAX_REASON;
            while !self.reason.is_char_boundary(end) {
                end -= 1;
            }
            self.reason.truncate(end);
        }
    }
}

/// The `meta` of a [`STREAM_AUDIO`] stream header.
#[derive(Clone, PartialEq, Message)]
pub struct AudioInfo {
    /// Samples per second.
    #[prost(uint32, tag = "1")]
    pub sample_rate: u32,
    /// Number of channels.
    #[prost(uint32, tag = "2")]
    pub channels: u32,
    /// [`CODEC_PCM_S16LE`].
    #[prost(string, tag = "3")]
    pub codec: String,
}

impl AudioInfo {
    /// The one format the host sends and the viewer plays.
    pub fn standard() -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            codec: CODEC_PCM_S16LE.into(),
        }
    }

    /// True for the format a viewer can play.
    pub fn is_standard(&self) -> bool {
        self.sample_rate == SAMPLE_RATE
            && self.channels == CHANNELS
            && self.codec == CODEC_PCM_S16LE
    }
}

/// Errors from reading the audio stream framing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChunkError {
    /// The length prefix is zero, odd, or over [`MAX_CHUNK`].
    #[error("chunk length {0} is out of range")]
    Length(usize),
}

/// Encode one chunk with its length prefix. Returns `None` for a chunk [`chunk_len`] would refuse.
pub fn encode_chunk(pcm: &[u8]) -> Option<Vec<u8>> {
    chunk_len(u32::try_from(pcm.len()).ok()?.to_be_bytes()).ok()?;
    let mut out = Vec::with_capacity(CHUNK_PREFIX + pcm.len());
    out.extend_from_slice(&u32::try_from(pcm.len()).ok()?.to_be_bytes());
    out.extend_from_slice(pcm);
    Some(out)
}

/// Read a length prefix. Refuses zero, an odd length (a sample is two bytes), and anything over
/// [`MAX_CHUNK`].
pub fn chunk_len(prefix: [u8; CHUNK_PREFIX]) -> Result<usize, ChunkError> {
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 || len > MAX_CHUNK || !len.is_multiple_of(2) {
        return Err(ChunkError::Length(len));
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_round_trip() {
        let pcm = vec![1u8, 2, 3, 4];
        let bytes = encode_chunk(&pcm).unwrap();
        let len = chunk_len(bytes[..4].try_into().unwrap()).unwrap();
        assert_eq!(len, 4);
        assert_eq!(&bytes[4..], pcm.as_slice());
    }

    #[test]
    fn chunk_len_limits() {
        assert!(chunk_len([0; 4]).is_err());
        assert!(chunk_len(3u32.to_be_bytes()).is_err());
        assert!(chunk_len(((MAX_CHUNK + 2) as u32).to_be_bytes()).is_err());
        assert_eq!(chunk_len((MAX_CHUNK as u32).to_be_bytes()), Ok(MAX_CHUNK));
        assert!(encode_chunk(&[]).is_none());
        assert!(encode_chunk(&vec![0; MAX_CHUNK + 2]).is_none());
    }

    #[test]
    fn only_the_standard_format_is_played() {
        assert!(AudioInfo::standard().is_standard());
        let mut a = AudioInfo::standard();
        a.sample_rate = 44_100;
        assert!(!a.is_standard());
    }

    #[test]
    fn reason_is_clamped_on_a_char_boundary() {
        let mut e = MicError {
            reason: "é".repeat(100),
        };
        e.sanitize();
        assert!(e.reason.len() <= MAX_REASON);
    }

    #[test]
    fn garbage_does_not_panic() {
        for n in 0..64u8 {
            let body: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            let _ = Mic::decode(body.as_slice());
            let _ = AudioInfo::decode(body.as_slice());
        }
    }
}
