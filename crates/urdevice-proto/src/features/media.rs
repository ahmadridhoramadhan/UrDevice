//! Media control (#20, design §9.4): wire messages for `ServiceId::MEDIA`.
//!
//! Every message travels as the body of a [`crate::Feature`] envelope, wrapped
//! in [`Media`]. Field numbers are local to this service and are the wire
//! contract: never reuse or renumber one. `proto/features/media.proto` lists
//! the same schema.
//!
//! The controller sends [`Subscribe`], [`Cmd`], and [`VolumeCmd`] to the host
//! (`Direction::ToHost`). The host answers with [`State`]
//! (`Direction::ToController`) while the controller is subscribed. There is no
//! shuffle and no repeat (R11.5).
//!
//! Everything here is untrusted input. [`Session::sanitize`] and
//! [`State::sanitize`] clamp what a peer sends so a hostile host cannot make
//! the controller hold or draw unbounded data.

use prost::Message;

/// Most sessions a [`State`] may carry. More are dropped.
pub const MAX_SESSIONS: usize = 16;

/// Longest session id, in bytes.
pub const MAX_ID: usize = 256;

/// Longest app name, title, or artist, in bytes.
pub const MAX_TEXT: usize = 512;

/// Largest artwork per session, in bytes of PNG.
pub const MAX_ARTWORK: usize = 96 * 1024;

/// Largest artwork per [`State`] in total, in bytes of PNG. Keeps the whole
/// message far below the 1 MiB control frame.
pub const MAX_ARTWORK_TOTAL: usize = 512 * 1024;

/// Longest `unavailable` reason, in bytes.
pub const MAX_REASON: usize = 64;

/// Largest `VolStep` the host applies, in steps of 2 %.
pub const MAX_VOLUME_STEP: i32 = 25;

/// A media message.
#[derive(Clone, PartialEq, Message)]
pub struct Media {
    /// The message.
    #[prost(oneof = "media::Msg", tags = "1, 2, 3, 4")]
    pub msg: Option<media::Msg>,
}

/// Nested types for [`Media`].
pub mod media {
    /// One media message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Msg {
        /// Controller to host: start or stop updates (R11.6).
        #[prost(message, tag = "1")]
        Subscribe(super::Subscribe),
        /// Host to controller: sessions and volume.
        #[prost(message, tag = "2")]
        State(super::State),
        /// Controller to host: control one session (R11.1, R11.2).
        #[prost(message, tag = "3")]
        Cmd(super::Cmd),
        /// Controller to host: system volume or mute (R11.1).
        #[prost(message, tag = "4")]
        Volume(super::VolumeCmd),
    }
}

impl From<media::Msg> for Media {
    fn from(msg: media::Msg) -> Self {
        Self { msg: Some(msg) }
    }
}

/// Turn updates on or off for this controller (R11.6).
#[derive(Clone, PartialEq, Message)]
pub struct Subscribe {
    /// True to receive [`State`] now and on every change; false to stop.
    #[prost(bool, tag = "1")]
    pub on: bool,
}

/// Playback state of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum PlayState {
    /// Not playing anything.
    Stopped = 0,
    /// Playing.
    Playing = 1,
    /// Paused.
    Paused = 2,
}

/// One media app on the host (SMTC session, MPRIS player, Android
/// MediaSession).
#[derive(Clone, PartialEq, Message)]
pub struct Session {
    /// Stable id on the host; the controller echoes it in [`Cmd`].
    #[prost(string, tag = "1")]
    pub id: String,
    /// App name, for the source tab.
    #[prost(string, tag = "2")]
    pub app_name: String,
    /// Track title; empty when unknown.
    #[prost(string, tag = "3")]
    pub title: String,
    /// Artist; empty when unknown.
    #[prost(string, tag = "4")]
    pub artist: String,
    /// Artwork as PNG, at most 256 px a side and [`MAX_ARTWORK`] bytes; empty
    /// when unknown.
    #[prost(bytes = "vec", tag = "5")]
    pub artwork_png: Vec<u8>,
    /// A [`PlayState`].
    #[prost(enumeration = "PlayState", tag = "6")]
    pub state: i32,
    /// Position in milliseconds when the host built this message.
    #[prost(uint64, optional, tag = "7")]
    pub position_ms: Option<u64>,
    /// Track length in milliseconds.
    #[prost(uint64, optional, tag = "8")]
    pub duration_ms: Option<u64>,
    /// The session can seek (R11.5).
    #[prost(bool, tag = "9")]
    pub can_seek: bool,
    /// The session can skip to the next track (R11.5).
    #[prost(bool, tag = "10")]
    pub can_next: bool,
    /// The session can go to the previous track (R11.5).
    #[prost(bool, tag = "11")]
    pub can_prev: bool,
    /// The session can play and pause (R11.5).
    #[prost(bool, tag = "12")]
    pub can_play_pause: bool,
}

impl Session {
    /// The playback state, `Stopped` for a value this build does not know.
    pub fn play_state(&self) -> PlayState {
        PlayState::try_from(self.state).unwrap_or(PlayState::Stopped)
    }

    /// Clamp a session a peer sent: cut text at a character boundary, drop
    /// artwork that is too large or is not a PNG, and keep the position
    /// inside the duration.
    pub fn sanitize(&mut self) {
        cut(&mut self.id, MAX_ID);
        cut(&mut self.app_name, MAX_TEXT);
        cut(&mut self.title, MAX_TEXT);
        cut(&mut self.artist, MAX_TEXT);
        if self.artwork_png.len() > MAX_ARTWORK || !self.artwork_png.starts_with(PNG_MAGIC) {
            self.artwork_png.clear();
        }
        if PlayState::try_from(self.state).is_err() {
            self.state = PlayState::Stopped as i32;
        }
        if let (Some(p), Some(d)) = (self.position_ms, self.duration_ms)
            && d > 0
            && p > d
        {
            self.position_ms = Some(d);
        }
    }
}

/// The first bytes of every PNG file.
pub const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Cut `s` to at most `max` bytes without splitting a character.
fn cut(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

/// Host to controller: everything the controller draws (design §9.4).
#[derive(Clone, PartialEq, Message)]
pub struct State {
    /// One entry per media app.
    #[prost(message, repeated, tag = "1")]
    pub sessions: Vec<Session>,
    /// System output volume, 0.0 to 1.0.
    #[prost(float, tag = "2")]
    pub volume: f32,
    /// System output is muted.
    #[prost(bool, tag = "3")]
    pub muted: bool,
    /// The host can read and set the system volume and mute.
    #[prost(bool, tag = "4")]
    pub can_volume: bool,
    /// Why the host cannot list sessions, as a short code the controller
    /// translates: `listener_permission` (Android notification access is
    /// off), `unsupported`, or empty when it can.
    #[prost(string, tag = "5")]
    pub unavailable: String,
}

impl State {
    /// Clamp a state a peer sent: at most [`MAX_SESSIONS`] sessions, total
    /// artwork within [`MAX_ARTWORK_TOTAL`], volume in 0..=1, and no
    /// duplicate session ids (the first wins).
    pub fn sanitize(&mut self) {
        self.sessions.truncate(MAX_SESSIONS);
        let mut total = 0usize;
        let mut seen = std::collections::HashSet::new();
        self.sessions.retain_mut(|s| {
            s.sanitize();
            if s.id.is_empty() || !seen.insert(s.id.clone()) {
                return false;
            }
            if total + s.artwork_png.len() > MAX_ARTWORK_TOTAL {
                s.artwork_png.clear();
            }
            total += s.artwork_png.len();
            true
        });
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0.0, 1.0)
        } else {
            0.0
        };
        cut(&mut self.unavailable, MAX_REASON);
    }
}

/// What to do to a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, prost::Enumeration)]
#[repr(i32)]
pub enum CmdKind {
    /// Toggle play and pause.
    PlayPause = 0,
    /// Next track.
    Next = 1,
    /// Previous track.
    Prev = 2,
    /// Jump to `seek_ms`.
    Seek = 3,
}

/// Controller to host: control one session (R11.1, R11.2).
#[derive(Clone, PartialEq, Message)]
pub struct Cmd {
    /// A [`Session::id`] from the last [`State`].
    #[prost(string, tag = "1")]
    pub session_id: String,
    /// A [`CmdKind`].
    #[prost(enumeration = "CmdKind", tag = "2")]
    pub kind: i32,
    /// Target position for [`CmdKind::Seek`], in milliseconds.
    #[prost(uint64, tag = "3")]
    pub seek_ms: u64,
}

/// Controller to host: the system output (R11.1).
#[derive(Clone, PartialEq, Message)]
pub struct VolumeCmd {
    /// What to do.
    #[prost(oneof = "volume_cmd::Cmd", tags = "1, 2, 3")]
    pub cmd: Option<volume_cmd::Cmd>,
}

/// Nested types for [`VolumeCmd`].
pub mod volume_cmd {
    /// One volume command.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Cmd {
        /// Set the volume, 0.0 to 1.0.
        #[prost(float, tag = "1")]
        Set(f32),
        /// Change the volume by this many 2 % steps; negative is down.
        #[prost(sint32, tag = "2")]
        Step(i32),
        /// Mute (true) or unmute (false).
        #[prost(bool, tag = "3")]
        Mute(bool),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str) -> Session {
        Session {
            id: id.into(),
            app_name: "Spotify".into(),
            title: "Song".into(),
            artist: "Band".into(),
            artwork_png: Vec::new(),
            state: PlayState::Playing as i32,
            position_ms: Some(1000),
            duration_ms: Some(200_000),
            can_seek: true,
            can_next: true,
            can_prev: false,
            can_play_pause: true,
        }
    }

    #[test]
    fn round_trip() {
        let m = Media::from(media::Msg::State(State {
            sessions: vec![session("a")],
            volume: 0.5,
            muted: true,
            can_volume: true,
            unavailable: String::new(),
        }));
        let bytes = m.encode_to_vec();
        assert_eq!(Media::decode(bytes.as_slice()).unwrap(), m);
        let c = Media::from(media::Msg::Volume(VolumeCmd {
            cmd: Some(volume_cmd::Cmd::Step(-3)),
        }));
        assert_eq!(Media::decode(c.encode_to_vec().as_slice()).unwrap(), c);
    }

    #[test]
    fn garbage_does_not_panic() {
        for len in 0..64u8 {
            let junk: Vec<u8> = (0..len)
                .map(|i| i.wrapping_mul(37).wrapping_add(11))
                .collect();
            let _ = Media::decode(junk.as_slice());
        }
    }

    #[test]
    fn sanitize_clamps_hostile_state() {
        let mut png = PNG_MAGIC.to_vec();
        png.resize(MAX_ARTWORK, 0);
        let mut big = session("big");
        big.title = "é".repeat(MAX_TEXT); // 2 bytes each: must cut on a boundary
        big.artwork_png = png.clone();
        let mut st = State {
            sessions: (0..40).map(|i| session(&format!("s{i}"))).collect(),
            volume: f32::NAN,
            muted: false,
            can_volume: true,
            unavailable: "x".repeat(500),
        };
        st.sessions[0] = big;
        for s in st.sessions.iter_mut().skip(1).take(10) {
            s.artwork_png = png.clone();
        }
        st.sessions.push(session("s1")); // duplicate of a kept id
        st.sanitize();
        assert!(st.sessions.len() <= MAX_SESSIONS);
        assert_eq!(st.volume, 0.0);
        assert!(st.unavailable.len() <= MAX_REASON);
        assert!(st.sessions[0].title.len() <= MAX_TEXT);
        let total: usize = st.sessions.iter().map(|s| s.artwork_png.len()).sum();
        assert!(total <= MAX_ARTWORK_TOTAL);
        let mut ids: Vec<_> = st.sessions.iter().map(|s| s.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), st.sessions.len());
    }

    #[test]
    fn sanitize_drops_bad_artwork_and_clamps_position() {
        let mut s = session("a");
        s.artwork_png = b"not a png".to_vec();
        s.position_ms = Some(999_999);
        s.state = 77;
        s.sanitize();
        assert!(s.artwork_png.is_empty());
        assert_eq!(s.position_ms, Some(200_000));
        assert_eq!(s.play_state(), PlayState::Stopped);
    }
}
