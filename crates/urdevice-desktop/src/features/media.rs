//! Media control (#20): the desktop side.
//!
//! The view is `ui/features/media.slint` (global `MediaFeature`). The core
//! service (`urdevice_core::services::media`) does the protocol; this file
//! gives it this computer's media adapter (when someone controls this
//! computer) and draws the state a host sends (when this computer controls
//! another device). See `docs/dev/adding-a-feature.md`.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode,
    VecModel,
};
use urdevice_core::identity::DeviceId;
use urdevice_core::node::{PeerStatus, PeerView};
use urdevice_core::services::media::{
    CmdKind, Media as MediaService, MediaEvents, MediaSession, MediaState, PlayState, VolumeChange,
};
use urdevice_core::{Capability, Mode, Node};

use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::{MainWindow, MediaFeature};

pub(crate) struct Media;

/// The message kinds `ui/features/media.slint` knows.
mod kind {
    pub const NONE: i32 = 0;
    pub const OFFLINE: i32 = 1;
    pub const DENIED: i32 = 2;
    pub const OFF: i32 = 3;
    pub const WAITING: i32 = 4;
    pub const LISTENER: i32 = 5;
    pub const UNSUPPORTED: i32 = 6;
    pub const EMPTY: i32 = 7;
    pub const REFUSED: i32 = 8;
}

/// How often the view redraws the playback position while it is open.
const TICK: Duration = Duration::from_millis(500);

/// Least time between two volume messages while the slider is dragged.
const VOLUME_EVERY: Duration = Duration::from_millis(150);

/// Everything the UI thread keeps for this view.
struct Ui {
    node: Option<Arc<Node>>,
    svc: Option<Arc<MediaService>>,
    /// The session the user picked, by id.
    selected: String,
    /// Hosts the user turned the player off for, until the app closes.
    off: HashSet<String>,
    /// The last refusal from the host, until the next state arrives.
    refused: bool,
    tabs: Vec<String>,
    art: Vec<u8>,
    art_image: Image,
    last_volume_sent: Option<Instant>,
    timer: Timer,
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui {
        node: None,
        svc: None,
        selected: String::new(),
        off: HashSet::new(),
        refused: false,
        tabs: Vec::new(),
        art: Vec::new(),
        art_image: Image::default(),
        last_volume_sent: None,
        timer: Timer::default(),
    });
}

/// Tells the view to redraw when the core has news. Called on core threads.
struct Events {
    window: slint::Weak<MainWindow>,
}

impl MediaEvents for Events {
    fn updated(&self, _peer: DeviceId) {
        let _ = self.window.upgrade_in_event_loop(|w| {
            UI.with(|u| u.borrow_mut().refused = false);
            sync(&w);
        });
    }

    fn refused(&self, _peer: DeviceId, _reason: &str) {
        let _ = self.window.upgrade_in_event_loop(|w| {
            UI.with(|u| u.borrow_mut().refused = true);
            sync(&w);
        });
    }
}

impl Feature for Media {
    fn id(&self) -> &'static str {
        "media"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Media,
            key: "media.v1",
        }]
    }

    fn has_view(&self) -> bool {
        true
    }

    fn start(&self, ctx: &FeatureCtx) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        let Some(svc) = ctx.node.service::<MediaService>() else {
            return;
        };
        // This computer's own sessions and volume, for controllers.
        svc.set_host(urdevice_platform::features::media::host());
        svc.set_events(Arc::new(Events {
            window: ctx.window.clone(),
        }));
        UI.with(|u| {
            let mut u = u.borrow_mut();
            u.node = Some(ctx.node.clone());
            u.svc = Some(svc);
            let weak = ctx.window.clone();
            u.timer.start(TimerMode::Repeated, TICK, move || {
                if let Some(w) = weak.upgrade() {
                    sync(&w);
                }
            });
        });
        wire(&w);
    }

    fn refresh(&self, ctx: &FeatureCtx, _snap: &urdevice_core::node::Snapshot) {
        if let Some(w) = ctx.window.upgrade() {
            sync(&w);
        }
    }

    fn open(&self, ctx: &FeatureCtx, peer: &PeerView, _tile: &str) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        let g = w.global::<MediaFeature>();
        let previous = g.get_peer_id().to_string();
        if previous != peer.id {
            // Leave the other host's updates off while this one is shown.
            if let (Some(old), Some(svc)) = (DeviceId::from_hex(&previous), service()) {
                let _ = svc.subscribe(old, false);
            }
            g.set_peer_id(peer.id.clone().into());
            UI.with(|u| {
                let mut u = u.borrow_mut();
                u.selected.clear();
                u.refused = false;
            });
        }
        let on = UI.with(|u| !u.borrow().off.contains(&peer.id));
        g.set_on(on);
        if on && let (Some(id), Some(svc)) = (DeviceId::from_hex(&peer.id), service()) {
            let _ = svc.subscribe(id, true);
        }
        sync(&w);
    }
}

fn service() -> Option<Arc<MediaService>> {
    UI.with(|u| u.borrow().svc.clone())
}

fn peer_of(w: &MainWindow) -> Option<(String, DeviceId)> {
    let hex = w.global::<MediaFeature>().get_peer_id().to_string();
    let id = DeviceId::from_hex(&hex)?;
    Some((hex, id))
}

/// Hook the Slint callbacks to the core service.
fn wire(w: &MainWindow) {
    let g = w.global::<MediaFeature>();
    let weak = w.as_weak();

    g.on_set_on({
        let weak = weak.clone();
        move |on| {
            let Some(w) = weak.upgrade() else { return };
            let (Some((hex, id)), Some(svc)) = (peer_of(&w), service()) else {
                return;
            };
            UI.with(|u| {
                let mut u = u.borrow_mut();
                if on {
                    u.off.remove(&hex);
                } else {
                    u.off.insert(hex.clone());
                }
            });
            w.global::<MediaFeature>().set_on(on);
            let _ = svc.subscribe(id, on);
            sync(&w);
        }
    });

    g.on_pick({
        let weak = weak.clone();
        move |i| {
            let Some(w) = weak.upgrade() else { return };
            let Some((_, id)) = peer_of(&w) else { return };
            let Some(st) = service().and_then(|s| s.state_of(id)) else {
                return;
            };
            if let Some(s) = usize::try_from(i).ok().and_then(|i| st.sessions.get(i)) {
                UI.with(|u| u.borrow_mut().selected = s.id.clone());
            }
            sync(&w);
        }
    });

    // A command for the selected session.
    let cmd = |kind: CmdKind| {
        let weak = weak.clone();
        move || {
            let Some(w) = weak.upgrade() else { return };
            if let Some((id, session)) = selected_session(&w) {
                let _ = service().map(|s| s.command(id, &session.id, kind, 0));
            }
        }
    };
    g.on_play_pause(cmd(CmdKind::PlayPause));
    g.on_prev(cmd(CmdKind::Prev));
    g.on_next(cmd(CmdKind::Next));

    g.on_seek({
        let weak = weak.clone();
        move |fraction| {
            let Some(w) = weak.upgrade() else { return };
            let Some((id, session)) = selected_session(&w) else {
                return;
            };
            let Some(duration) = session.duration_ms else {
                return;
            };
            let ms = (f64::from(fraction.clamp(0.0, 1.0)) * duration as f64) as u64;
            let _ = service().map(|s| s.command(id, &session.id, CmdKind::Seek, ms));
            w.global::<MediaFeature>()
                .set_progress(fraction.clamp(0.0, 1.0));
        }
    });

    g.on_set_volume({
        let weak = weak.clone();
        move |v, last| {
            let Some(w) = weak.upgrade() else { return };
            let Some((_, id)) = peer_of(&w) else { return };
            let throttled = UI.with(|u| {
                let mut u = u.borrow_mut();
                let now = Instant::now();
                let skip = !last
                    && u.last_volume_sent
                        .is_some_and(|t| now.duration_since(t) < VOLUME_EVERY);
                if !skip {
                    u.last_volume_sent = Some(now);
                }
                skip
            });
            if throttled {
                return;
            }
            let v = v.clamp(0.0, 1.0);
            let _ = service().map(|s| s.volume(id, VolumeChange::Set(v)));
            if last {
                w.global::<MediaFeature>().set_volume(v);
                w.global::<MediaFeature>().set_muted(false);
            }
        }
    });

    g.on_set_muted({
        move |m| {
            let Some(w) = weak.upgrade() else { return };
            let Some((_, id)) = peer_of(&w) else { return };
            let _ = service().map(|s| s.volume(id, VolumeChange::Mute(m)));
            w.global::<MediaFeature>().set_muted(m);
        }
    });
}

/// The host and the session the user is looking at.
fn selected_session(w: &MainWindow) -> Option<(DeviceId, MediaSession)> {
    let (_, id) = peer_of(w)?;
    let st = service()?.state_of(id)?;
    let selected = UI.with(|u| u.borrow().selected.clone());
    let s = st
        .sessions
        .iter()
        .find(|s| s.id == selected)
        .or_else(|| st.sessions.first())?
        .clone();
    Some((id, s))
}

/// Redraw the view from the core's current state.
fn sync(w: &MainWindow) {
    let g = w.global::<MediaFeature>();
    let Some((hex, id)) = peer_of(w) else { return };
    let (Some(node), Some(svc)) = UI.with(|u| {
        let u = u.borrow();
        (u.node.clone(), u.svc.clone())
    }) else {
        return;
    };
    let on = g.get_on();
    let peer = node.snapshot().peers.into_iter().find(|p| p.id == hex);
    let connected = peer
        .as_ref()
        .is_some_and(|p| p.status == PeerStatus::Connected);
    let denied = peer
        .as_ref()
        .and_then(|p| {
            p.remote_permissions
                .iter()
                .find(|r| r.capability == Capability::Media)
        })
        .is_some_and(|r| r.mode == Mode::Deny);
    let state = if on && connected && !denied {
        svc.state_of(id)
    } else {
        None
    };
    let refused = UI.with(|u| u.borrow().refused);

    let message = if !connected {
        kind::OFFLINE
    } else if denied {
        kind::DENIED
    } else if !on {
        kind::OFF
    } else if refused && state.is_none() {
        kind::REFUSED
    } else {
        match &state {
            None => kind::WAITING,
            Some(st) if st.unavailable == "listener_permission" => kind::LISTENER,
            Some(st) if st.sessions.is_empty() && !st.unavailable.is_empty() => kind::UNSUPPORTED,
            Some(st) if st.sessions.is_empty() => kind::EMPTY,
            Some(_) => kind::NONE,
        }
    };
    g.set_message_kind(message);
    draw(w, state.as_ref());
}

fn draw(w: &MainWindow, state: Option<&MediaState>) {
    let g = w.global::<MediaFeature>();
    let Some(st) = state else {
        set_tabs(&g, &[]);
        g.set_can_volume(false);
        g.set_can_seek(false);
        g.set_can_prev(false);
        g.set_can_next(false);
        g.set_can_play_pause(false);
        return;
    };
    g.set_can_volume(st.can_volume);
    g.set_volume(st.volume);
    g.set_muted(st.muted);

    let names: Vec<String> = st.sessions.iter().map(|s| s.app_name.clone()).collect();
    set_tabs(&g, &names);
    let selected = UI.with(|u| u.borrow().selected.clone());
    let idx = st
        .sessions
        .iter()
        .position(|s| s.id == selected)
        .unwrap_or(0);
    g.set_selected(i32::try_from(idx).unwrap_or(0));
    let Some(s) = st.sessions.get(idx) else {
        return;
    };
    UI.with(|u| u.borrow_mut().selected = s.id.clone());

    g.set_app_name(s.app_name.clone().into());
    g.set_title(s.title.clone().into());
    g.set_artist(s.artist.clone().into());
    g.set_playing(s.play_state() == PlayState::Playing);
    g.set_can_seek(s.can_seek && s.duration_ms.is_some());
    g.set_can_prev(s.can_prev);
    g.set_can_next(s.can_next);
    g.set_can_play_pause(s.can_play_pause);
    // Do not fight the user's finger while the seek bar is dragged: the
    // slider itself shows the drag position.
    let pos = s.position_ms.unwrap_or(0);
    let dur = s.duration_ms.unwrap_or(0);
    g.set_progress(if dur > 0 {
        (pos as f64 / dur as f64).clamp(0.0, 1.0) as f32
    } else {
        0.0
    });
    g.set_position_text(fmt_ms(pos).into());
    g.set_duration_text(fmt_ms(dur).into());

    let image = UI.with(|u| {
        let mut u = u.borrow_mut();
        if u.art != s.artwork_png {
            u.art = s.artwork_png.clone();
            u.art_image = decode_art(&s.artwork_png).unwrap_or_default();
        }
        (!u.art.is_empty() && u.art_image.size().width > 0).then(|| u.art_image.clone())
    });
    match image {
        Some(img) => {
            g.set_artwork(img);
            g.set_has_artwork(true);
        }
        None => g.set_has_artwork(false),
    }
}

fn set_tabs(g: &MediaFeature<'_>, names: &[String]) {
    let changed = UI.with(|u| {
        let mut u = u.borrow_mut();
        if u.tabs == names {
            false
        } else {
            u.tabs = names.to_vec();
            true
        }
    });
    if changed {
        let model: Vec<SharedString> = names.iter().map(|n| n.as_str().into()).collect();
        g.set_tabs(ModelRc::new(VecModel::from(model)));
    }
}

fn fmt_ms(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A PNG from a host, as a Slint image. The core already checked the size.
fn decode_art(bytes: &[u8]) -> Option<Image> {
    if bytes.is_empty() {
        return None;
    }
    let mut dec = png::Decoder::new(Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > 512 || h > 512 {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    let f = reader.next_frame(&mut buf).ok()?;
    let rgba: Vec<u8> = match f.color_type {
        png::ColorType::Rgba => buf[..f.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..f.buffer_size()]
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf[..f.buffer_size()]
            .iter()
            .flat_map(|g| [*g, *g, *g, 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf[..f.buffer_size()]
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Indexed => return None,
    };
    if rgba.len() != (w as usize) * (h as usize) * 4 {
        return None;
    }
    let mut px = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    px.make_mut_bytes().copy_from_slice(&rgba);
    Some(Image::from_rgba8(px))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_naturally() {
        assert_eq!(fmt_ms(0), "0:00");
        assert_eq!(fmt_ms(65_000), "1:05");
        assert_eq!(fmt_ms(3_725_000), "1:02:05");
    }

    #[test]
    fn rubbish_artwork_gives_no_image() {
        assert!(decode_art(b"").is_none());
        assert!(decode_art(b"not a png").is_none());
    }
}
