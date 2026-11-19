//! Media control (#20): bindings for the Android app.
//!
//! Kotlin gives the core this phone's media sessions and volume
//! ([`MediaHostFfi`], used when another device controls the phone) and
//! listens for state from a host ([`MediaListener`], used when the phone
//! controls another device).

use std::sync::Arc;

use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::media::{
    CmdKind, Media, MediaEvents, MediaHost, MediaSession, MediaState, PlayState, VolumeChange,
};
use urdevice_core::{CoreError, Result};

use crate::{FfiError, UrDeviceNode, device_id};

/// Playback state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiPlayState {
    Stopped,
    Playing,
    Paused,
}

/// One media app on a host.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiMediaSession {
    /// Echo this id in commands.
    pub id: String,
    pub app_name: String,
    /// Empty when unknown.
    pub title: String,
    /// Empty when unknown.
    pub artist: String,
    /// A PNG of at most 256 px, or empty.
    pub artwork_png: Vec<u8>,
    pub state: FfiPlayState,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub can_seek: bool,
    pub can_next: bool,
    pub can_prev: bool,
    pub can_play_pause: bool,
}

/// Everything a media player screen draws.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiMediaState {
    pub sessions: Vec<FfiMediaSession>,
    /// 0.0 to 1.0.
    pub volume: f32,
    pub muted: bool,
    pub can_volume: bool,
    /// Empty, `listener_permission`, or `unsupported`.
    pub unavailable: String,
}

/// What to do to a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiMediaCmd {
    PlayPause,
    Next,
    Prev,
    Seek,
}

/// A change of the system output.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum FfiVolumeChange {
    /// 0.0 to 1.0.
    Set {
        value: f32,
    },
    /// Steps of 2 %; negative is down.
    Step {
        steps: i32,
    },
    Mute {
        muted: bool,
    },
}

/// The phone's media sessions and volume, implemented in Kotlin. Called
/// from core worker threads.
#[uniffi::export(with_foreign)]
pub trait MediaHostFfi: Send + Sync {
    fn state(&self) -> FfiMediaState;
    fn command(&self, session_id: String, cmd: FfiMediaCmd, seek_ms: u64) -> bool;
    fn volume(&self, change: FfiVolumeChange) -> bool;
}

/// News from the core, on core threads: post to the main thread.
#[uniffi::export(with_foreign)]
pub trait MediaListener: Send + Sync {
    /// New state from `device_id`, or it was cleared.
    fn on_updated(&self, device_id: String);
    /// The host refused: `denied`, `declined`, `timeout`, `unsupported`, `offline`.
    fn on_refused(&self, device_id: String, reason: String);
}

impl From<MediaSession> for FfiMediaSession {
    fn from(s: MediaSession) -> Self {
        let state = match s.play_state() {
            PlayState::Playing => FfiPlayState::Playing,
            PlayState::Paused => FfiPlayState::Paused,
            PlayState::Stopped => FfiPlayState::Stopped,
        };
        Self {
            id: s.id,
            app_name: s.app_name,
            title: s.title,
            artist: s.artist,
            artwork_png: s.artwork_png,
            state,
            position_ms: s.position_ms,
            duration_ms: s.duration_ms,
            can_seek: s.can_seek,
            can_next: s.can_next,
            can_prev: s.can_prev,
            can_play_pause: s.can_play_pause,
        }
    }
}

impl From<FfiMediaSession> for MediaSession {
    fn from(s: FfiMediaSession) -> Self {
        let state = match s.state {
            FfiPlayState::Playing => PlayState::Playing,
            FfiPlayState::Paused => PlayState::Paused,
            FfiPlayState::Stopped => PlayState::Stopped,
        };
        Self {
            id: s.id,
            app_name: s.app_name,
            title: s.title,
            artist: s.artist,
            artwork_png: s.artwork_png,
            state: state as i32,
            position_ms: s.position_ms,
            duration_ms: s.duration_ms,
            can_seek: s.can_seek,
            can_next: s.can_next,
            can_prev: s.can_prev,
            can_play_pause: s.can_play_pause,
        }
    }
}

impl From<MediaState> for FfiMediaState {
    fn from(s: MediaState) -> Self {
        Self {
            sessions: s.sessions.into_iter().map(Into::into).collect(),
            volume: s.volume,
            muted: s.muted,
            can_volume: s.can_volume,
            unavailable: s.unavailable,
        }
    }
}

impl From<FfiMediaState> for MediaState {
    fn from(s: FfiMediaState) -> Self {
        Self {
            sessions: s.sessions.into_iter().map(Into::into).collect(),
            volume: s.volume,
            muted: s.muted,
            can_volume: s.can_volume,
            unavailable: s.unavailable,
        }
    }
}

impl From<FfiMediaCmd> for CmdKind {
    fn from(c: FfiMediaCmd) -> Self {
        match c {
            FfiMediaCmd::PlayPause => Self::PlayPause,
            FfiMediaCmd::Next => Self::Next,
            FfiMediaCmd::Prev => Self::Prev,
            FfiMediaCmd::Seek => Self::Seek,
        }
    }
}

impl From<CmdKind> for FfiMediaCmd {
    fn from(c: CmdKind) -> Self {
        match c {
            CmdKind::PlayPause => Self::PlayPause,
            CmdKind::Next => Self::Next,
            CmdKind::Prev => Self::Prev,
            CmdKind::Seek => Self::Seek,
        }
    }
}

impl From<FfiVolumeChange> for VolumeChange {
    fn from(c: FfiVolumeChange) -> Self {
        match c {
            FfiVolumeChange::Set { value } => Self::Set(value),
            FfiVolumeChange::Step { steps } => Self::Step(steps),
            FfiVolumeChange::Mute { muted } => Self::Mute(muted),
        }
    }
}

impl From<&VolumeChange> for FfiVolumeChange {
    fn from(c: &VolumeChange) -> Self {
        match c {
            VolumeChange::Set(v) => Self::Set { value: *v },
            VolumeChange::Step(s) => Self::Step { steps: *s },
            VolumeChange::Mute(m) => Self::Mute { muted: *m },
        }
    }
}

struct HostAdapter(Arc<dyn MediaHostFfi>);

impl MediaHost for HostAdapter {
    fn state(&self) -> MediaState {
        self.0.state().into()
    }

    fn command(&self, _grant: &Grant, session_id: &str, kind: CmdKind, seek_ms: u64) -> Result<()> {
        if self.0.command(session_id.to_owned(), kind.into(), seek_ms) {
            Ok(())
        } else {
            Err(CoreError::Io(std::io::Error::other("media command failed")))
        }
    }

    fn volume(&self, _grant: &Grant, change: &VolumeChange) -> Result<()> {
        if self.0.volume(change.into()) {
            Ok(())
        } else {
            Err(CoreError::Io(std::io::Error::other("volume change failed")))
        }
    }
}

struct EventsAdapter(Arc<dyn MediaListener>);

impl MediaEvents for EventsAdapter {
    fn updated(&self, peer: DeviceId) {
        self.0.on_updated(peer.hex());
    }

    fn refused(&self, peer: DeviceId, reason: &str) {
        self.0.on_refused(peer.hex(), reason.to_owned());
    }
}

impl UrDeviceNode {
    fn media_service(&self) -> Result<Arc<Media>, FfiError> {
        self.node.service::<Media>().ok_or(FfiError::NotFound)
    }
}

#[uniffi::export]
impl UrDeviceNode {
    /// Give the core this phone's media sessions and volume.
    pub fn media_set_host(&self, host: Arc<dyn MediaHostFfi>) {
        if let Some(s) = self.node.service::<Media>() {
            s.set_host(Arc::new(HostAdapter(host)));
        }
    }

    /// Listen for state pushed by a host this phone controls.
    pub fn media_set_listener(&self, listener: Arc<dyn MediaListener>) {
        if let Some(s) = self.node.service::<Media>() {
            s.set_events(Arc::new(EventsAdapter(listener)));
        }
    }

    /// Turn the player for `device` on or off (R11.6).
    pub fn media_subscribe(&self, device: String, on: bool) -> Result<(), FfiError> {
        let peer = device_id(&device)?;
        self.media_service()?.subscribe(peer, on)?;
        Ok(())
    }

    /// True while the player for `device` is on.
    pub fn media_is_on(&self, device: String) -> bool {
        device_id(&device)
            .ok()
            .zip(self.node.service::<Media>())
            .is_some_and(|(peer, s)| s.is_on(peer))
    }

    /// The newest state from `device`, with playing positions moved forward.
    pub fn media_state(&self, device: String) -> Option<FfiMediaState> {
        let peer = device_id(&device).ok()?;
        self.node.service::<Media>()?.state_of(peer).map(Into::into)
    }

    /// Control a session on `device`.
    pub fn media_command(
        &self,
        device: String,
        session_id: String,
        cmd: FfiMediaCmd,
        seek_ms: u64,
    ) -> Result<(), FfiError> {
        let peer = device_id(&device)?;
        self.media_service()?
            .command(peer, &session_id, cmd.into(), seek_ms)?;
        Ok(())
    }

    /// Change the volume or mute of `device`'s system output.
    pub fn media_volume(&self, device: String, change: FfiVolumeChange) -> Result<(), FfiError> {
        let peer = device_id(&device)?;
        self.media_service()?.volume(peer, change.into())?;
        Ok(())
    }
}
