//! Linux media host: MPRIS players over the session D-Bus, and the system
//! volume through `wpctl` (PipeWire) or `pactl` (PulseAudio).
//!
//! One session per `org.mpris.MediaPlayer2.*` bus name (R11.2). The bus name
//! is the session id. Artwork comes only from `file://` URLs: the host never
//! fetches a remote URL for a player.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use urdevice_core::services::Grant;
use urdevice_core::services::media::{
    CmdKind, MediaHost, MediaSession, MediaState, PlayState, VolumeChange,
};
use urdevice_core::{CoreError, Result};
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use super::artwork::{self, MAX_INPUT_BYTES};
use super::parse::{
    Volume, file_url_path, is_mpris_bus_name, pactl_muted, pactl_volume, wpctl_volume,
};

const PLAYER_PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";

/// A slow player must not stall the others for long.
const CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// Cover files kept after decoding, so a track is decoded once.
const ART_CACHE: usize = 12;

type Props = HashMap<String, OwnedValue>;

pub(crate) struct Mpris {
    conn: Mutex<Option<Connection>>,
    names: Mutex<HashMap<String, String>>,
    art: Mutex<HashMap<String, Option<Vec<u8>>>>,
}

impl Mpris {
    pub(crate) fn new() -> Self {
        Self {
            conn: Mutex::new(None),
            names: Mutex::new(HashMap::new()),
            art: Mutex::new(HashMap::new()),
        }
    }

    /// The session bus connection, made on first use and again after an error.
    fn conn(&self) -> Option<Connection> {
        let mut g = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        if g.is_none() {
            *g = zbus::blocking::connection::Builder::session()
                .and_then(|b| b.method_timeout(CALL_TIMEOUT).build())
                .ok();
        }
        g.clone()
    }

    fn reset_conn(&self) {
        *self.conn.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    fn player_names(&self, conn: &Connection) -> Option<Vec<String>> {
        let bus = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
        let mut names: Vec<String> = bus
            .list_names()
            .ok()?
            .into_iter()
            .map(|n| n.to_string())
            // playerctld mirrors another player; listing it would double a tab.
            .filter(|n| is_mpris_bus_name(n) && !n.ends_with(".playerctld"))
            .collect();
        names.sort();
        Some(names)
    }

    fn proxy<'a>(
        conn: &Connection,
        name: &'a str,
        iface: &'a str,
    ) -> Option<zbus::blocking::Proxy<'a>> {
        zbus::blocking::Proxy::new(conn, name, PLAYER_PATH, iface).ok()
    }

    fn identity(&self, conn: &Connection, name: &str) -> String {
        if let Some(n) = self
            .names
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(name)
        {
            return n.clone();
        }
        let ident = Self::proxy(conn, name, PROPS_IFACE)
            .and_then(|p| {
                p.call::<_, _, OwnedValue>("Get", &(ROOT_IFACE, "Identity"))
                    .ok()
            })
            .and_then(|v| str_of(&v))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                name.trim_start_matches("org.mpris.MediaPlayer2.")
                    .to_owned()
            });
        let mut g = self.names.lock().unwrap_or_else(|p| p.into_inner());
        if g.len() > 64 {
            g.clear();
        }
        g.insert(name.to_owned(), ident.clone());
        ident
    }

    fn read_player(&self, conn: &Connection, name: &str) -> Option<MediaSession> {
        let props: Props = Self::proxy(conn, name, PROPS_IFACE)?
            .call("GetAll", &(PLAYER_IFACE,))
            .ok()?;
        let flag = |k: &str| props.get(k).and_then(bool_of).unwrap_or(false);
        let control = flag("CanControl");
        let state = match props.get("PlaybackStatus").and_then(str_of).as_deref() {
            Some("Playing") => PlayState::Playing,
            Some("Paused") => PlayState::Paused,
            _ => PlayState::Stopped,
        };
        let md: Props = props
            .get("Metadata")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| Props::try_from(v).ok())
            .unwrap_or_default();
        let title = md.get("xesam:title").and_then(str_of).unwrap_or_default();
        let artist = md
            .get("xesam:artist")
            .and_then(|v| strs_of(v).into_iter().next().or_else(|| str_of(v)))
            .unwrap_or_default();
        let duration_ms = md
            .get("mpris:length")
            .and_then(int_of)
            .and_then(|us| u64::try_from(us / 1000).ok())
            .filter(|d| *d > 0);
        let position_ms = props
            .get("Position")
            .and_then(int_of)
            .or_else(|| {
                Self::proxy(conn, name, PROPS_IFACE)?
                    .call::<_, _, OwnedValue>("Get", &(PLAYER_IFACE, "Position"))
                    .ok()
                    .as_ref()
                    .and_then(int_of)
            })
            .and_then(|us| u64::try_from(us / 1000).ok());
        let artwork_png = md
            .get("mpris:artUrl")
            .and_then(str_of)
            .and_then(|u| self.artwork(&u))
            .unwrap_or_default();
        Some(MediaSession {
            id: name.to_owned(),
            app_name: self.identity(conn, name),
            title,
            artist,
            artwork_png,
            state: state as i32,
            position_ms,
            duration_ms,
            can_seek: control && flag("CanSeek"),
            can_next: control && flag("CanGoNext"),
            can_prev: control && flag("CanGoPrevious"),
            can_play_pause: control && (flag("CanPlay") || flag("CanPause")),
        })
    }

    /// The cover for `url`, as a small PNG. Only local files are read.
    fn artwork(&self, url: &str) -> Option<Vec<u8>> {
        if let Some(hit) = self.art.lock().unwrap_or_else(|p| p.into_inner()).get(url) {
            return hit.clone();
        }
        let png = file_url_path(url).and_then(|p| read_image(&p));
        let mut g = self.art.lock().unwrap_or_else(|p| p.into_inner());
        if g.len() >= ART_CACHE {
            g.clear();
        }
        g.insert(url.to_owned(), png.clone());
        png
    }

    /// Position and track id of a player, for a seek.
    fn seek_info(conn: &Connection, name: &str) -> Option<(Option<String>, i64)> {
        let props = Self::proxy(conn, name, PROPS_IFACE)?;
        let all: Props = props.call("GetAll", &(PLAYER_IFACE,)).ok()?;
        let pos = all.get("Position").and_then(int_of).unwrap_or(0);
        let track = all
            .get("Metadata")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| Props::try_from(v).ok())
            .and_then(|m| m.get("mpris:trackid").and_then(str_of));
        Some((track, pos))
    }

    fn do_command(&self, session_id: &str, kind: CmdKind, seek_ms: u64) -> Result<()> {
        if !is_mpris_bus_name(session_id) {
            return Err(CoreError::InvalidInput("bad media session".into()));
        }
        let conn = self.conn().ok_or_else(no_bus)?;
        let player = Self::proxy(&conn, session_id, PLAYER_IFACE).ok_or_else(no_bus)?;
        let r = match kind {
            CmdKind::PlayPause => player.call::<_, _, ()>("PlayPause", &()),
            CmdKind::Next => player.call::<_, _, ()>("Next", &()),
            CmdKind::Prev => player.call::<_, _, ()>("Previous", &()),
            CmdKind::Seek => {
                let target = i64::try_from(seek_ms.min(i64::MAX as u64 / 1000))
                    .unwrap_or(0)
                    .saturating_mul(1000);
                match Self::seek_info(&conn, session_id) {
                    Some((Some(track), _)) if ObjectPath::try_from(track.as_str()).is_ok() => {
                        match ObjectPath::try_from(track.as_str()) {
                            Ok(path) => player.call::<_, _, ()>("SetPosition", &(path, target)),
                            Err(e) => Err(e.into()),
                        }
                    }
                    Some((_, now)) => player.call::<_, _, ()>("Seek", &(target - now)),
                    None => return Err(no_bus()),
                }
            }
        };
        r.map_err(|e| {
            self.reset_conn();
            CoreError::Io(std::io::Error::other(format!("media command failed: {e}")))
        })
    }
}

fn no_bus() -> CoreError {
    CoreError::Io(std::io::Error::other("session bus not available"))
}

/// The default output through PipeWire's `wpctl`, else PulseAudio's `pactl`.
mod audio {
    use super::*;

    const WPCTL_SINK: &str = "@DEFAULT_AUDIO_SINK@";
    const PACTL_SINK: &str = "@DEFAULT_SINK@";

    fn run(program: &str, args: &[&str]) -> Option<String> {
        let out = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub(super) fn read() -> Option<Volume> {
        if let Some(v) = run("wpctl", &["get-volume", WPCTL_SINK])
            .as_deref()
            .and_then(wpctl_volume)
        {
            return Some(v);
        }
        let level = pactl_volume(&run("pactl", &["get-sink-volume", PACTL_SINK])?)?;
        let muted = run("pactl", &["get-sink-mute", PACTL_SINK])
            .as_deref()
            .and_then(pactl_muted)
            .unwrap_or(false);
        Some(Volume { level, muted })
    }

    pub(super) fn apply(change: &VolumeChange) -> Result<()> {
        let ok = match change {
            VolumeChange::Set(v) => {
                let v = if v.is_finite() {
                    v.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let w = format!("{v:.2}");
                let p = format!("{}%", (v * 100.0).round() as u32);
                run("wpctl", &["set-volume", "-l", "1.0", WPCTL_SINK, &w]).is_some()
                    || run("pactl", &["set-sink-volume", PACTL_SINK, &p]).is_some()
            }
            VolumeChange::Step(n) => {
                let pct = n.unsigned_abs().min(25) * 2;
                let sign = if *n < 0 { '-' } else { '+' };
                let w = format!("{pct}%{sign}");
                let p = format!("{sign}{pct}%");
                *n == 0
                    || run("wpctl", &["set-volume", "-l", "1.0", WPCTL_SINK, &w]).is_some()
                    || run("pactl", &["set-sink-volume", PACTL_SINK, &p]).is_some()
            }
            VolumeChange::Mute(m) => {
                let f = if *m { "1" } else { "0" };
                run("wpctl", &["set-mute", WPCTL_SINK, f]).is_some()
                    || run("pactl", &["set-sink-mute", PACTL_SINK, f]).is_some()
            }
        };
        if ok {
            Ok(())
        } else {
            Err(CoreError::Io(std::io::Error::other(
                "no audio control available",
            )))
        }
    }
}

impl MediaHost for Mpris {
    fn state(&self) -> MediaState {
        let volume = audio::read();
        let mut st = MediaState {
            volume: volume.map_or(0.0, |v| v.level),
            muted: volume.is_some_and(|v| v.muted),
            can_volume: volume.is_some(),
            ..MediaState::default()
        };
        let Some(conn) = self.conn() else {
            st.unavailable = "unsupported".into();
            return st;
        };
        let Some(names) = self.player_names(&conn) else {
            self.reset_conn();
            st.unavailable = "unsupported".into();
            return st;
        };
        self.names
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|k, _| names.contains(k));
        let sessions: Vec<Option<MediaSession>> = std::thread::scope(|s| {
            let jobs: Vec<_> = names
                .iter()
                .map(|n| {
                    let conn = &conn;
                    s.spawn(move || self.read_player(conn, n))
                })
                .collect();
            jobs.into_iter().map(|j| j.join().ok().flatten()).collect()
        });
        st.sessions = sessions.into_iter().flatten().collect();
        st
    }

    fn command(&self, _grant: &Grant, session_id: &str, kind: CmdKind, seek_ms: u64) -> Result<()> {
        self.do_command(session_id, kind, seek_ms)
    }

    fn volume(&self, _grant: &Grant, change: &VolumeChange) -> Result<()> {
        audio::apply(change)
    }
}

/// Read a cover file: a regular file of a sane size, decoded and shrunk.
fn read_image(path: &str) -> Option<Vec<u8>> {
    let f = std::fs::File::open(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    f.take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .ok()?;
    artwork::to_small_png(&buf)
}

fn unwrap_value<'a>(mut v: &'a Value<'a>) -> &'a Value<'a> {
    while let Value::Value(inner) = v {
        v = inner;
    }
    v
}

fn str_of(v: &OwnedValue) -> Option<String> {
    match unwrap_value(v) {
        Value::Str(s) => Some(s.as_str().to_owned()),
        Value::ObjectPath(p) => Some(p.as_str().to_owned()),
        _ => None,
    }
}

fn strs_of(v: &OwnedValue) -> Vec<String> {
    match unwrap_value(v) {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| match unwrap_value(x) {
                Value::Str(s) => Some(s.as_str().to_owned()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn bool_of(v: &OwnedValue) -> Option<bool> {
    match unwrap_value(v) {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

fn int_of(v: &OwnedValue) -> Option<i64> {
    match unwrap_value(v) {
        Value::I64(n) => Some(*n),
        Value::U64(n) => i64::try_from(*n).ok(),
        Value::I32(n) => Some(i64::from(*n)),
        Value::U32(n) => Some(i64::from(*n)),
        Value::F64(n) if n.is_finite() => Some(*n as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads whatever players run on this machine. With no session bus the
    /// state says so; either way nothing panics and ids stay valid.
    #[test]
    fn live_state_is_well_formed() {
        let host = Mpris::new();
        let mut st = host.state();
        st.sanitize();
        for s in &st.sessions {
            assert!(is_mpris_bus_name(&s.id));
            assert!(!s.app_name.is_empty());
        }
        if std::env::var_os("URDEVICE_PRINT_MEDIA").is_some() {
            for s in &st.sessions {
                eprintln!(
                    "{} | {} | {} - {} | {:?} {:?}/{:?} seek={} next={} prev={} art={}",
                    s.id,
                    s.app_name,
                    s.artist,
                    s.title,
                    s.play_state(),
                    s.position_ms,
                    s.duration_ms,
                    s.can_seek,
                    s.can_next,
                    s.can_prev,
                    s.artwork_png.len()
                );
            }
            eprintln!(
                "volume {} muted {} can_volume {} unavailable '{}'",
                st.volume, st.muted, st.can_volume, st.unavailable
            );
        }
    }

    #[test]
    fn refuses_a_session_that_is_not_an_mpris_name() {
        let host = Mpris::new();
        assert!(
            host.do_command("org.freedesktop.Notifications", CmdKind::Next, 0)
                .is_err()
        );
        assert!(host.do_command("", CmdKind::Next, 0).is_err());
    }
}
