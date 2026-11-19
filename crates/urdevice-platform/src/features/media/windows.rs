//! Windows media host: the System Media Transport Controls (SMTC) sessions
//! that Windows lists for every media app, and the default output volume
//! through Core Audio (R11.2, R11.5, design §9.4).
//!
//! The session id is the app's AppUserModelId; a second session of the same
//! app gets `#2`, `#3`, ... in list order.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use urdevice_core::services::Grant;
use urdevice_core::services::media::{
    CmdKind, MAX_ARTWORK, MediaHost, MediaSession, MediaState, PlayState, VolumeChange,
};
use urdevice_core::{CoreError, Result};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};

use super::artwork::{self, MAX_INPUT_BYTES};
use super::parse::app_name_from_aumid;

/// 100 ns ticks per millisecond.
const TICKS_PER_MS: i64 = 10_000;

/// Seconds from 1601-01-01 (Windows `DateTime`) to 1970-01-01.
const EPOCH_DIFF_SECS: i64 = 11_644_473_600;

/// Most thumbnails kept after decoding.
const ART_CACHE: usize = 12;

pub(crate) struct Smtc {
    /// Decoded thumbnails by session id, title, and artist.
    art: Mutex<HashMap<String, Option<Vec<u8>>>>,
}

fn io(msg: impl Into<String>) -> CoreError {
    CoreError::Io(std::io::Error::other(msg.into()))
}

/// WinRT and Core Audio need COM in the multithreaded apartment on the
/// calling thread. A second call on the same thread is harmless.
fn init_com() {
    // SAFETY: plain COM initialization with no reserved argument.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

impl Smtc {
    pub(crate) fn new() -> Self {
        Self {
            art: Mutex::new(HashMap::new()),
        }
    }

    /// Every session with its id, in list order.
    fn sessions() -> Option<Vec<(String, Session)>> {
        init_com();
        let manager = Manager::RequestAsync().ok()?.join().ok()?;
        let list = manager.GetSessions().ok()?;
        let mut seen: HashMap<String, u32> = HashMap::new();
        let mut out = Vec::new();
        for i in 0..list.Size().ok()? {
            let Ok(s) = list.GetAt(i) else { continue };
            let aumid = s
                .SourceAppUserModelId()
                .map(|h| h.to_string())
                .unwrap_or_default();
            if aumid.is_empty() {
                continue;
            }
            let n = seen.entry(aumid.clone()).or_insert(0);
            *n += 1;
            let id = if *n == 1 {
                aumid
            } else {
                format!("{aumid}#{n}")
            };
            out.push((id, s));
        }
        Some(out)
    }

    fn read(&self, id: &str, s: &Session) -> Option<MediaSession> {
        let aumid = id.split('#').next().unwrap_or(id);
        let props = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
        let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
        let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
        let info = s.GetPlaybackInfo().ok()?;
        let controls = info.Controls().ok()?;
        let state = match info.PlaybackStatus() {
            Ok(Status::Playing) => PlayState::Playing,
            Ok(Status::Paused) => PlayState::Paused,
            _ => PlayState::Stopped,
        };
        let flag = |r: windows::core::Result<bool>| r.unwrap_or(false);
        let (position_ms, duration_ms) = match s.GetTimelineProperties() {
            Ok(t) => {
                let start = t.StartTime().map(|x| x.Duration).unwrap_or(0);
                let end = t.EndTime().map(|x| x.Duration).unwrap_or(0);
                let pos = t.Position().map(|x| x.Duration).unwrap_or(0);
                let mut pos = pos - start;
                if state == PlayState::Playing
                    && let Ok(upd) = t.LastUpdatedTime()
                {
                    // The position is as of `LastUpdatedTime`; a playing
                    // session has moved on since.
                    pos += (now_ticks() - upd.UniversalTime).max(0);
                }
                let dur = end - start;
                (
                    u64::try_from(pos / TICKS_PER_MS).ok(),
                    u64::try_from(dur / TICKS_PER_MS).ok().filter(|d| *d > 0),
                )
            }
            Err(_) => (None, None),
        };
        let can_pp = flag(controls.IsPlayPauseToggleEnabled())
            || flag(controls.IsPlayEnabled())
            || flag(controls.IsPauseEnabled());
        let artwork_png = self.thumbnail(id, &title, &artist, &props);
        Some(MediaSession {
            id: id.to_owned(),
            app_name: app_name_from_aumid(aumid),
            title,
            artist,
            artwork_png,
            state: state as i32,
            position_ms,
            duration_ms,
            can_seek: flag(controls.IsPlaybackPositionEnabled()),
            can_next: flag(controls.IsNextEnabled()),
            can_prev: flag(controls.IsPreviousEnabled()),
            can_play_pause: can_pp,
        })
    }

    fn thumbnail(
        &self,
        id: &str,
        title: &str,
        artist: &str,
        props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
    ) -> Vec<u8> {
        let key = format!("{id}\u{1}{title}\u{1}{artist}");
        if let Some(hit) = self.art.lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
            return hit.clone().unwrap_or_default();
        }
        let png = read_thumbnail(props);
        let mut g = self.art.lock().unwrap_or_else(|p| p.into_inner());
        if g.len() >= ART_CACHE {
            g.clear();
        }
        g.insert(key, png.clone());
        png.unwrap_or_default()
    }
}

fn read_thumbnail(
    props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> Option<Vec<u8>> {
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = usize::try_from(stream.Size().ok()?).ok()?;
    if size == 0 || size > MAX_INPUT_BYTES {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).ok()?).ok()?;
    reader
        .LoadAsync(u32::try_from(size).ok()?)
        .ok()?
        .join()
        .ok()?;
    let mut buf = vec![0u8; size];
    reader.ReadBytes(&mut buf).ok()?;
    let png = artwork::to_small_png(&buf)?;
    (png.len() <= MAX_ARTWORK).then_some(png)
}

fn now_ticks() -> i64 {
    let since_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() / 100)
        .unwrap_or(0);
    i64::try_from(since_unix)
        .unwrap_or(0)
        .saturating_add(EPOCH_DIFF_SECS * 10_000_000)
}

/// The default render endpoint's volume interface.
fn endpoint() -> windows::core::Result<IAudioEndpointVolume> {
    init_com();
    // SAFETY: COM calls with valid arguments; the interfaces are released
    // when they drop.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
    }
}

fn read_volume() -> Option<(f32, bool)> {
    let ep = endpoint().ok()?;
    // SAFETY: `ep` is a live interface.
    unsafe {
        let level = ep.GetMasterVolumeLevelScalar().ok()?;
        let muted = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        Some((level.clamp(0.0, 1.0), muted))
    }
}

fn set_volume(change: &VolumeChange) -> Result<()> {
    let ep = endpoint().map_err(|e| io(format!("no audio output: {e}")))?;
    // SAFETY: `ep` is a live interface and the event context may be null.
    let r = unsafe {
        match change {
            VolumeChange::Set(v) => {
                let v = if v.is_finite() {
                    v.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ep.SetMasterVolumeLevelScalar(v, std::ptr::null())
            }
            VolumeChange::Step(n) => {
                let now = ep.GetMasterVolumeLevelScalar().unwrap_or(0.0);
                let v = (now + *n as f32 * 0.02).clamp(0.0, 1.0);
                ep.SetMasterVolumeLevelScalar(v, std::ptr::null())
            }
            VolumeChange::Mute(m) => ep.SetMute(*m, std::ptr::null()),
        }
    };
    r.map_err(|e| io(format!("volume change failed: {e}")))
}

impl MediaHost for Smtc {
    fn state(&self) -> MediaState {
        let vol = read_volume();
        let mut st = MediaState {
            volume: vol.map_or(0.0, |v| v.0),
            muted: vol.is_some_and(|v| v.1),
            can_volume: vol.is_some(),
            ..MediaState::default()
        };
        match Self::sessions() {
            Some(list) => {
                st.sessions = list.iter().filter_map(|(id, s)| self.read(id, s)).collect();
            }
            None => st.unavailable = "unsupported".into(),
        }
        st
    }

    fn command(&self, _grant: &Grant, session_id: &str, kind: CmdKind, seek_ms: u64) -> Result<()> {
        let list = Self::sessions().ok_or_else(|| io("media sessions not available"))?;
        let (_, s) = list
            .iter()
            .find(|(id, _)| id == session_id)
            .ok_or(CoreError::NotFound)?;
        let ok = match kind {
            CmdKind::PlayPause => s.TryTogglePlayPauseAsync(),
            CmdKind::Next => s.TrySkipNextAsync(),
            CmdKind::Prev => s.TrySkipPreviousAsync(),
            CmdKind::Seek => {
                let ms = i64::try_from(seek_ms).unwrap_or(i64::MAX / TICKS_PER_MS);
                // The position is counted from the track's start time.
                let start = s
                    .GetTimelineProperties()
                    .and_then(|t| t.StartTime())
                    .map(|t| t.Duration)
                    .unwrap_or(0);
                s.TryChangePlaybackPositionAsync(
                    ms.saturating_mul(TICKS_PER_MS).saturating_add(start),
                )
            }
        }
        .and_then(|op| op.join())
        .map_err(|e| io(format!("media command failed: {e}")))?;
        if ok {
            Ok(())
        } else {
            Err(io("the app refused the command"))
        }
    }

    fn volume(&self, _grant: &Grant, change: &VolumeChange) -> Result<()> {
        set_volume(change)
    }
}
