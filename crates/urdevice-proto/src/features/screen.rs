//! Screen sharing (#21, design §10): wire messages for `ServiceId::SCREEN`.
//!
//! Control messages travel as the body of a [`crate::Feature`] envelope,
//! wrapped in [`Screen`]. Field numbers are local to this service and are the
//! wire contract: never reuse or renumber one. `proto/features/screen.proto`
//! lists the same schema.
//!
//! The viewer (controller) asks for an activity first: screen is a watched
//! capability, so every message it sends to the host names that activity.
//! It sends [`ListDisplays`], then [`Start`] for one display, and
//! [`RequestKeyframe`] when its decoder lost sync. The host answers with
//! [`Displays`] and opens one unidirectional stream of kind [`STREAM_VIDEO`]
//! per [`Start`] (`Direction::ToController`), whose header `meta` is a
//! [`StreamInfo`]. The stream carries [`VideoFrame`]s, each prefixed with its
//! length as a big-endian `u32` ([`encode_frame`], [`frame_len`]).
//!
//! The phone-as-webcam service (`ServiceId::CAMERA`, #28) speaks the same
//! messages: its "displays" are the phone's cameras (`primary` is the back
//! camera), and a viewer adds [`Control`] to switch the flash and the zoom.
//!
//! Everything here is untrusted input. Lengths are checked against the limits
//! below before anything is allocated, and [`Display::sanitize`] clamps what a
//! hostile host can make a viewer draw.

use prost::Message;

/// Stream kind for the video stream (host to viewer).
pub const STREAM_VIDEO: u32 = 1;

/// Most displays a [`Displays`] may carry. More are dropped.
pub const MAX_DISPLAYS: usize = 16;

/// Longest display name, in bytes.
pub const MAX_NAME: usize = 128;

/// Longest error reason, in bytes.
pub const MAX_REASON: usize = 64;

/// Largest encoded [`VideoFrame`], in bytes. A 1080p keyframe at 8 Mbps is
/// far below this.
pub const MAX_FRAME: usize = 4 * 1024 * 1024;

/// Largest width or height a host may announce or a viewer may ask for.
pub const MAX_DIMENSION: u32 = 8192;

/// Highest frame rate a viewer may ask for.
pub const MAX_FPS: u32 = 60;

/// Zoom is sent as a ratio times 100: 100 is no zoom.
pub const ZOOM_MIN: u32 = 100;

/// Largest zoom ratio a viewer may ask for (10x). The host clamps it to what the camera does.
pub const ZOOM_MAX: u32 = 1000;

/// Size of the length prefix in front of every frame on the video stream.
pub const FRAME_PREFIX: usize = 4;

/// A screen message.
#[derive(Clone, PartialEq, Message)]
pub struct Screen {
    /// The message.
    #[prost(oneof = "message::Msg", tags = "1, 2, 3, 4, 5, 6")]
    pub msg: Option<message::Msg>,
}

/// Nested types for [`Screen`]: `message::Msg` is the oneof.
pub mod message {
    /// One screen message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Msg {
        /// Viewer to host: which displays can be shared (R12.6).
        #[prost(message, tag = "1")]
        ListDisplays(super::ListDisplays),
        /// Host to viewer: the display list.
        #[prost(message, tag = "2")]
        Displays(super::Displays),
        /// Viewer to host: start (or switch) the video stream.
        #[prost(message, tag = "3")]
        Start(super::Start),
        /// Viewer to host: the next frame should be a keyframe.
        #[prost(message, tag = "4")]
        RequestKeyframe(super::RequestKeyframe),
        /// Host to viewer: capture could not start or stopped.
        #[prost(message, tag = "5")]
        Error(super::ScreenError),
        /// Viewer to host (camera only): flash and zoom.
        #[prost(message, tag = "6")]
        Control(super::Control),
    }
}

impl From<message::Msg> for Screen {
    fn from(msg: message::Msg) -> Self {
        Self { msg: Some(msg) }
    }
}

/// Ask the host for its displays.
#[derive(Clone, PartialEq, Message)]
pub struct ListDisplays {}

/// One display on the host.
#[derive(Clone, PartialEq, Message)]
pub struct Display {
    /// Host-chosen id; the viewer echoes it in [`Start`].
    #[prost(uint32, tag = "1")]
    pub id: u32,
    /// Name to show in the picker, for example "DELL U2720Q".
    #[prost(string, tag = "2")]
    pub name: String,
    /// Width in pixels; 0 when the host only learns it once capture starts
    /// (the Wayland portal, Android).
    #[prost(uint32, tag = "3")]
    pub width: u32,
    /// Height in pixels; 0 when unknown.
    #[prost(uint32, tag = "4")]
    pub height: u32,
    /// The primary display.
    #[prost(bool, tag = "5")]
    pub primary: bool,
}

impl Display {
    /// Clamp a display a peer sent.
    pub fn sanitize(&mut self) {
        cut(&mut self.name, MAX_NAME);
        self.width = self.width.min(MAX_DIMENSION);
        self.height = self.height.min(MAX_DIMENSION);
    }
}

/// The host's displays.
#[derive(Clone, PartialEq, Message)]
pub struct Displays {
    /// At most [`MAX_DISPLAYS`].
    #[prost(message, repeated, tag = "1")]
    pub displays: Vec<Display>,
}

impl Displays {
    /// Clamp a list a peer sent.
    pub fn sanitize(&mut self) {
        self.displays.truncate(MAX_DISPLAYS);
        for d in &mut self.displays {
            d.sanitize();
        }
    }
}

/// Start the video stream for one display. A second `Start` in the same
/// activity switches the display: the host ends the old stream and opens a
/// new one.
#[derive(Clone, PartialEq, Message)]
pub struct Start {
    /// A [`Display::id`].
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    /// Largest width the viewer wants, 0 for no limit.
    #[prost(uint32, tag = "2")]
    pub max_width: u32,
    /// Largest height the viewer wants, 0 for no limit.
    #[prost(uint32, tag = "3")]
    pub max_height: u32,
    /// Highest frame rate the viewer wants, 0 for the host's default.
    #[prost(uint32, tag = "4")]
    pub max_fps: u32,
}

impl Start {
    /// Clamp a request a peer sent.
    pub fn sanitize(&mut self) {
        self.max_width = self.max_width.min(MAX_DIMENSION);
        self.max_height = self.max_height.min(MAX_DIMENSION);
        self.max_fps = self.max_fps.min(MAX_FPS);
    }
}

/// Camera controls during a stream (`ServiceId::CAMERA` only): the host
/// applies them to the running camera and ignores what the camera cannot do.
#[derive(Clone, PartialEq, Message)]
pub struct Control {
    /// The torch (flash light) is on.
    #[prost(bool, tag = "1")]
    pub flash: bool,
    /// Zoom ratio times 100, [`ZOOM_MIN`] to [`ZOOM_MAX`].
    #[prost(uint32, tag = "2")]
    pub zoom_x100: u32,
}

impl Control {
    /// Clamp a request a peer sent.
    pub fn sanitize(&mut self) {
        self.zoom_x100 = self.zoom_x100.clamp(ZOOM_MIN, ZOOM_MAX);
    }
}

/// Ask for a keyframe: the viewer's decoder lost sync or fell behind.
#[derive(Clone, PartialEq, Message)]
pub struct RequestKeyframe {}

/// Capture failed or ended on the host.
#[derive(Clone, PartialEq, Message)]
pub struct ScreenError {
    /// `no_host` (nothing can capture here), `no_display`, `capture_failed`,
    /// `consent_declined` (the OS dialog was refused), or `stopped`.
    #[prost(string, tag = "1")]
    pub reason: String,
}

impl ScreenError {
    /// Clamp a reason a peer sent.
    pub fn sanitize(&mut self) {
        cut(&mut self.reason, MAX_REASON);
    }
}

/// The `meta` of a [`STREAM_VIDEO`] stream header.
#[derive(Clone, PartialEq, Message)]
pub struct StreamInfo {
    /// The display being captured.
    #[prost(uint32, tag = "1")]
    pub display_id: u32,
    /// Always `h264` (Annex B byte stream, no B-frames).
    #[prost(string, tag = "2")]
    pub codec: String,
}

/// One encoded access unit on the video stream (design §10).
#[derive(Clone, PartialEq, Message)]
pub struct VideoFrame {
    /// Capture time on the host, in microseconds from the stream start.
    #[prost(uint64, tag = "1")]
    pub pts_us: u64,
    /// True for an IDR frame (with SPS and PPS in front).
    #[prost(bool, tag = "2")]
    pub keyframe: bool,
    /// H.264 Annex B NAL units.
    #[prost(bytes = "vec", tag = "3")]
    pub data: Vec<u8>,
    /// Encoded width, set on keyframes; 0 otherwise.
    #[prost(uint32, tag = "4")]
    pub width: u32,
    /// Encoded height, set on keyframes; 0 otherwise.
    #[prost(uint32, tag = "5")]
    pub height: u32,
}

/// Errors from reading the video stream framing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// The length prefix is over [`MAX_FRAME`] or zero.
    #[error("frame length {0} is out of range")]
    Length(usize),
    /// The frame did not decode.
    #[error("bad video frame")]
    Decode,
}

/// Encode one frame with its length prefix.
pub fn encode_frame(frame: &VideoFrame) -> Vec<u8> {
    let len = frame.encoded_len();
    let mut out = Vec::with_capacity(FRAME_PREFIX + len);
    out.extend_from_slice(&u32::try_from(len).unwrap_or(u32::MAX).to_be_bytes());
    frame.encode(&mut out).unwrap_or_default();
    out
}

/// Read a length prefix. Refuses zero and anything over [`MAX_FRAME`].
pub fn frame_len(prefix: [u8; FRAME_PREFIX]) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(FrameError::Length(len));
    }
    Ok(len)
}

/// Decode the body of one frame (after its prefix) and clamp it.
pub fn decode_frame(body: &[u8]) -> Result<VideoFrame, FrameError> {
    if body.len() > MAX_FRAME {
        return Err(FrameError::Length(body.len()));
    }
    let mut f = VideoFrame::decode(body).map_err(|_| FrameError::Decode)?;
    f.width = f.width.min(MAX_DIMENSION);
    f.height = f.height.min(MAX_DIMENSION);
    Ok(f)
}

fn cut(s: &mut String, max: usize) {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let f = VideoFrame {
            pts_us: 33_333,
            keyframe: true,
            data: vec![0, 0, 0, 1, 0x67, 1, 2, 3],
            width: 1920,
            height: 1080,
        };
        let bytes = encode_frame(&f);
        let len = frame_len(bytes[..4].try_into().unwrap()).unwrap();
        assert_eq!(len, bytes.len() - 4);
        assert_eq!(decode_frame(&bytes[4..]).unwrap(), f);
    }

    #[test]
    fn frame_len_limits() {
        assert!(frame_len([0; 4]).is_err());
        assert!(frame_len(((MAX_FRAME + 1) as u32).to_be_bytes()).is_err());
        assert_eq!(frame_len((MAX_FRAME as u32).to_be_bytes()), Ok(MAX_FRAME));
    }

    #[test]
    fn sanitize_clamps() {
        let mut d = Displays {
            displays: (0..40)
                .map(|i| Display {
                    id: i,
                    name: "é".repeat(200),
                    width: u32::MAX,
                    height: 5,
                    primary: false,
                })
                .collect(),
        };
        d.sanitize();
        assert_eq!(d.displays.len(), MAX_DISPLAYS);
        assert!(d.displays[0].name.len() <= MAX_NAME);
        assert_eq!(d.displays[0].width, MAX_DIMENSION);
        let mut s = Start {
            display_id: 0,
            max_width: 99_999,
            max_height: 1,
            max_fps: 1000,
        };
        s.sanitize();
        assert_eq!((s.max_width, s.max_fps), (MAX_DIMENSION, MAX_FPS));
    }

    #[test]
    fn control_zoom_is_clamped() {
        let mut c = Control {
            flash: true,
            zoom_x100: 0,
        };
        c.sanitize();
        assert_eq!(c.zoom_x100, ZOOM_MIN);
        c.zoom_x100 = u32::MAX;
        c.sanitize();
        assert_eq!(c.zoom_x100, ZOOM_MAX);
    }

    #[test]
    fn garbage_does_not_panic() {
        for n in 0..64u8 {
            let body: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            let _ = decode_frame(&body);
            let _ = Screen::decode(body.as_slice());
        }
    }
}
