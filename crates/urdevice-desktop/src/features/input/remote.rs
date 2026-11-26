//! The remote-control window (#19, T4.8, R9.1, R10.2): control another desktop
//! or an Android phone from this desktop.
//!
//! The window is a pad shaped like the other device's display. Clicking it
//! **captures** the pointer and keyboard: pointer positions, buttons, wheel,
//! and keys go to the other device through the core's `Input` service. Pressing
//! `RELEASE_HOTKEY` (Ctrl+Alt+G) (or Stop, or closing the window) releases them and lets
//! go of every key and button that is down. Nothing typed or pointed is
//! logged.
//!
//! The rules that turn key events into text, keys, and shortcuts live in
//! `urdevice_core::services::input` (`KeyboardCapture`), where they are tested.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use slint::ComponentHandle as _;
use urdevice_core::identity::DeviceId;
use urdevice_core::services::input::{
    HostStatus, Input as Service, KeyOut, KeyboardCapture, Missing, Mods, PermStatus,
    RelativePointer, button, host_moves_by_distance, normalize, scroll_units,
};

use crate::ui::RemoteWindow;

/// Everything about the one open remote-control window.
struct Remote {
    win: RemoteWindow,
    peer: DeviceId,
    name: String,
    svc: Arc<Service>,
    keys: KeyboardCapture,
    rel: RelativePointer,
    /// The host moves by distance only (Wayland portal, `uinput`).
    by_distance: bool,
    captured: bool,
    buttons: BTreeSet<u32>,
    /// What the host last reported, for the bar.
    host: Option<HostStatus>,
    refused: Option<String>,
    /// The window shows the other device's screen (#21) and control starts with a click on it.
    screen: bool,
    /// A click asked for control before the session was active: capture as soon as it is.
    grab_when_active: bool,
    /// The size of the picture the window was last fitted to.
    fitted: (u32, u32),
    /// A line about the screen session (asking, starting, ended) that replaces the control line.
    note: Option<String>,
    /// The real pointer is grabbed (confined or locked) right now.
    grabbed: Cell<bool>,
    /// The pointer is locked: its raw movement, not window positions, moves the host.
    locked: Cell<bool>,
    /// Movement not yet sent because it was less than one pixel.
    remainder: (f64, f64),
}

thread_local! {
    /// Slint windows live on the UI thread.
    static REMOTE: RefCell<Option<Remote>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Remote) -> R) -> Option<R> {
    REMOTE.with(|r| r.borrow_mut().as_mut().map(f))
}

/// Open the window for `peer` (or bring the open one forward) and start a
/// session. Runs on the UI thread.
pub(super) fn open(svc: Arc<Service>, peer: DeviceId, name: &str) {
    open_window(svc, peer, name, false);
}

/// Open the window that shows `peer`'s screen (#21). It starts no control session: the first
/// click on the picture does.
pub(crate) fn open_screen(svc: Arc<Service>, peer: DeviceId, name: &str) {
    open_window(svc, peer, name, true);
}

fn open_window(svc: Arc<Service>, peer: DeviceId, name: &str, screen: bool) {
    let same = REMOTE.with(|r| {
        r.borrow()
            .as_ref()
            .is_some_and(|x| x.peer == peer && x.screen == screen)
    });
    if !same {
        // Another device's window ends its session first.
        close();
        let Ok(win) = RemoteWindow::new() else {
            tracing::warn!("the remote-control window could not be created");
            return;
        };
        wire(&win);
        REMOTE.with(|r| {
            *r.borrow_mut() = Some(Remote {
                win,
                peer,
                name: name.to_owned(),
                svc,
                keys: KeyboardCapture::default(),
                rel: RelativePointer::default(),
                by_distance: false,
                captured: false,
                buttons: BTreeSet::new(),
                host: None,
                refused: None,
                screen,
                grab_when_active: false,
                fitted: (0, 0),
                note: None,
                grabbed: Cell::new(false),
                locked: Cell::new(false),
                remainder: (0.0, 0.0),
            });
        });
    }
    with(|r| {
        r.win.set_device_name(r.name.as_str().into());
        r.win.set_screen(r.screen);
        let _ = r.win.show();
        if !r.screen {
            r.start();
        }
        r.refresh();
    });
}

/// The newest picture of the shared screen, `width` by `height` pixels. The first picture (and a
/// change of size) fits the window to it, never larger than this computer's screen.
pub(crate) fn screen_frame(frame: slint::Image, width: u32, height: u32) {
    with(|r| {
        if !r.screen || width == 0 || height == 0 {
            return;
        }
        r.win.set_frame(frame);
        r.win.set_has_frame(true);
        r.win.set_aspect(width as f32 / height as f32);
        if r.fitted != (width, height) {
            r.fitted = (width, height);
            fit_window(&r.win, width, height);
        }
    });
}

/// The picture is gone (the session ended or restarted).
pub(crate) fn screen_clear() {
    with(|r| {
        if r.screen {
            r.win.set_has_frame(false);
            r.win.set_frame(slint::Image::default());
            r.fitted = (0, 0);
        }
    });
}

/// A line about the screen session, or `None` to show the control line again.
pub(crate) fn screen_note(note: Option<String>) {
    with(|r| {
        if r.screen {
            r.note = note;
            r.refresh();
        }
    });
}

/// The displays of the other device when it has more than one, and the one shown.
pub(crate) fn screen_displays(names: &[String], index: i32) {
    with(|r| {
        if r.screen {
            let model: Vec<slint::SharedString> = names.iter().map(|n| n.as_str().into()).collect();
            r.win
                .set_displays(slint::ModelRc::new(slint::VecModel::from(model)));
            r.win.set_display_index(index);
        }
    });
}

/// Room around the picture: the padding of the window, and the bar above the pad.
const FRAME_W: f64 = 24.0;
const FRAME_H: f64 = 68.0;

/// Size the window to the picture at 1:1, shrunk (same shape) until it fits this computer's
/// screen, so it is never larger than that screen.
fn fit_window(win: &RemoteWindow, width: u32, height: u32) {
    use slint::winit_030::WinitWindowAccessor;
    let mut limit = (1600.0_f64, 900.0_f64);
    win.window().with_winit_window(|w| {
        if let Some(m) = w.current_monitor().or_else(|| w.primary_monitor()) {
            let scale = m.scale_factor().max(0.5);
            limit = (
                f64::from(m.size().width) / scale,
                f64::from(m.size().height) / scale,
            );
        }
    });
    let (fw, fh) = (f64::from(width), f64::from(height));
    // Leave room for the title bar and the panel of the desktop.
    let room_w = limit.0 * 0.96 - FRAME_W;
    let room_h = limit.1 * 0.88 - FRAME_H;
    let k = (room_w / fw).min(room_h / fh).clamp(0.1, 1.0);
    win.window().set_size(slint::LogicalSize::new(
        (fw * k + FRAME_W) as f32,
        (fh * k + FRAME_H) as f32,
    ));
}

/// True while the remote-control window (not a screen window) for `peer` is open.
pub(super) fn is_open_for(peer: DeviceId) -> bool {
    REMOTE.with(|r| {
        r.try_borrow()
            .ok()
            .is_some_and(|r| r.as_ref().is_some_and(|r| !r.screen && r.peer == peer))
    })
}

/// End the session and close the window. Runs on the UI thread.
pub(super) fn close() {
    let old = REMOTE.with(|r| r.borrow_mut().take());
    if let Some(mut r) = old {
        r.release();
        let _ = r.svc.end(r.peer);
        let _ = r.win.hide();
    }
}

/// The session with `peer` started or ended (any reason).
pub(super) fn session(peer: DeviceId, active: bool) {
    with(|r| {
        if r.peer == peer {
            if !active {
                r.release();
                r.host = None;
            } else if r.grab_when_active {
                r.grab_when_active = false;
                r.set_captured(true);
            }
            r.refresh();
        }
    });
}

/// `peer` reported what it can do.
pub(super) fn host_status(peer: DeviceId, status: HostStatus) {
    with(|r| {
        if r.peer == peer {
            // A phone shows its own pointer, so its window moves by distance too (like a
            // touchpad). A screen window shows the picture and keeps absolute positions.
            r.by_distance = host_moves_by_distance(&status.backend)
                || (status.backend == "android" && !r.screen);
            r.rel.leave();
            if r.grabbed.get() {
                // The way the host moves may have changed: grab again to match.
                r.grabbed.set(false);
            }
            r.host = Some(status);
            r.refused = None;
            r.refresh();
        }
    });
}

/// `peer` refused the session.
pub(super) fn refused(peer: DeviceId, reason: &str) {
    with(|r| {
        if r.peer == peer {
            r.refused = Some(reason.to_owned());
            r.release();
            r.refresh();
        }
    });
}

impl Remote {
    fn active(&self) -> bool {
        self.svc.is_active(self.peer)
    }

    fn start(&mut self) {
        if !self.active() && !self.svc.is_connecting(self.peer) {
            self.refused = None;
            if self.svc.connect(self.peer).is_err() {
                self.refused = Some("unsupported".into());
            }
        }
    }

    /// Redraw the bar and the pad from what is known.
    fn refresh(&self) {
        let active = self.active();
        let name = &self.name;
        let status = if let Some(note) = &self.note {
            note.clone()
        } else if let Some(why) = &self.refused {
            match why.as_str() {
                "denied" => format!("Remote control is turned off on {name}."),
                "declined" => format!("{name} did not allow remote control."),
                "timeout" => format!("{name} did not answer in time."),
                "unsupported" => format!("{name} cannot be controlled from here."),
                _ => format!("{name} is offline."),
            }
        } else if self.svc.is_connecting(self.peer) {
            format!("Waiting for {name} to allow remote control…")
        } else if !active && self.screen {
            format!("Viewing {name}. Click the screen to control it.")
        } else if !active {
            "Remote control is off.".to_owned()
        } else {
            match &self.host {
                Some(h) => match h.status {
                    PermStatus::Ready => match h.text_missing {
                        Some(_) => format!(
                            "Controlling {name}. Typing needs the UrDevice keyboard turned on and in use on it."
                        ),
                        None => format!("Controlling {name}."),
                    },
                    PermStatus::Missing(m) => match m {
                        Missing::AccessibilityService => format!(
                            "Turn on the UrDevice accessibility service on {name} (Settings > Accessibility). Taps, Back, Home and Recents need it."
                        ),
                        Missing::InputMethod => format!(
                            "{name} needs the UrDevice keyboard turned on and in use before it can type."
                        ),
                        Missing::Portal => format!(
                            "Approve the remote-control request on the screen of {name}, then try again."
                        ),
                        Missing::Uinput => {
                            format!(
                                "{name} needs a one-time setup for remote control. It asks on that computer."
                            )
                        }
                        Missing::Accessibility => format!(
                            "Turn on UrDevice in System Settings > Privacy & Security > Accessibility on {name}."
                        ),
                    },
                    PermStatus::Unavailable => {
                        format!("{name} cannot take remote control on this system.")
                    }
                },
                None => format!("Controlling {name}."),
            }
        };
        self.win.set_status(status.into());
        self.win.set_active(active);
        self.win.set_captured(self.captured && active);
        self.apply_grab();
        let android = self.host.as_ref().is_some_and(|h| h.backend == "android");
        self.win.set_show_nav(android);
        // In a screen window the picture itself sets the shape (it changes when the phone turns);
        // the size the host reported once would squash it.
        let picture_sets_shape = self.screen && self.fitted != (0, 0);
        if !picture_sets_shape
            && let Some((w, h)) = self.host.as_ref().and_then(|h| h.display)
            && h > 0
        {
            self.win.set_aspect(w as f32 / h as f32);
        }
    }

    fn set_captured(&mut self, on: bool) {
        if on && !self.active() {
            if self.screen {
                // The click on the picture asks for control; capture starts once it is allowed.
                self.grab_when_active = true;
                self.start();
                self.refresh();
            }
            return;
        }
        if !on {
            self.grab_when_active = false;
            self.release_keys();
        }
        self.captured = on;
        self.rel.leave();
        self.win.set_captured(on);
        self.apply_grab();
    }

    /// Let go of the pointer and keyboard: every key and button that is down
    /// goes up on the other device first.
    fn release(&mut self) {
        self.set_captured(false);
    }

    fn release_keys(&mut self) {
        for b in std::mem::take(&mut self.buttons) {
            let _ = self.svc.button(self.peer, b, false);
        }
        let outs = self.keys.release_all();
        self.send(outs);
    }

    fn send(&mut self, outs: Vec<KeyOut>) {
        for o in outs {
            let r = match o {
                KeyOut::Text(t) => self.svc.text(self.peer, &t),
                KeyOut::Key {
                    usage,
                    down,
                    modifiers,
                } => self.svc.key(self.peer, usage, down, modifiers),
                KeyOut::Release => {
                    self.set_captured(false);
                    Ok(())
                }
            };
            if r.is_err() {
                // The session is gone; the bar says so after the next refresh.
                break;
            }
        }
    }

    /// Grab the real pointer while it controls the other device, and let go otherwise.
    /// A host that moves by distance gets a locked pointer: it never stops at the window edge.
    fn apply_grab(&self) {
        // A screen window shows the picture and keeps the mouse free; only a pad grabs it.
        let want = self.captured && self.active() && !self.screen;
        if want == self.grabbed.get() {
            return;
        }
        self.grabbed.set(want);
        self.locked
            .set(confine_pointer(&self.win, want, self.by_distance));
    }

    /// How many pixels on the host one pixel of the pad is worth. A phone: the pad is as wide
    /// as the phone's screen, so crossing the window crosses the screen.
    fn gain(&self, pad_w: f32) -> f32 {
        match &self.host {
            Some(h) if h.backend == "android" && pad_w > 0.0 => h
                .display
                .map_or(1.0, |(dw, _)| (dw as f32 / pad_w).clamp(0.5, 8.0)),
            _ => 1.0,
        }
    }

    /// Raw mouse movement while the pointer is locked.
    fn raw_move(&mut self, dx: f64, dy: f64) {
        if !self.captured || !self.locked.get() || !self.by_distance {
            return;
        }
        let size = self.win.window().size();
        let pad_w = size.width as f32 / self.win.window().scale_factor();
        let k = f64::from(self.gain(pad_w)) * RAW_GAIN;
        let x = dx * k + self.remainder.0;
        let y = dy * k + self.remainder.1;
        let (sx, sy) = (x.round(), y.round());
        self.remainder = (x - sx, y - sy);
        if sx != 0.0 || sy != 0.0 {
            let _ = self.svc.pointer_move(self.peer, sx as i32, sy as i32);
        }
    }

    fn pointer(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if !self.captured || self.locked.get() {
            return;
        }
        if self.by_distance {
            if let Some((dx, dy)) = self.rel.moved(x, y) {
                let k = self.gain(w);
                let scaled = |v: i32| (v as f32 * k).round() as i32;
                let _ = self.svc.pointer_move(self.peer, scaled(dx), scaled(dy));
            }
        } else {
            let _ = self
                .svc
                .pointer_abs(self.peer, normalize(x, w), normalize(y, h), 0);
        }
    }
}

/// Keep the real mouse inside the window while it controls the other device, so it cannot
/// wander off and click something else. Released with Ctrl+Alt+G. Not every system can
/// confine the pointer; then it is simply not confined.
///
/// With `lock` the pointer is locked instead of confined, where the system can: it stops
/// at no edge, and its movement arrives as raw distances ([`RawMotion`]). Returns true when
/// the pointer is locked.
fn confine_pointer(win: &RemoteWindow, on: bool, lock: bool) -> bool {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::window::CursorGrabMode;
    let locked = win
        .window()
        .with_winit_window(|w| {
            if !on {
                let _ = w.set_cursor_grab(CursorGrabMode::None);
                return false;
            }
            if lock && w.set_cursor_grab(CursorGrabMode::Locked).is_ok() {
                tracing::info!("pointer locked to the window");
                return true;
            }
            match w.set_cursor_grab(CursorGrabMode::Confined) {
                Ok(()) => tracing::info!("pointer confined to the window"),
                Err(e) => tracing::warn!(error = %e, "could not confine the pointer"),
            }
            false
        })
        .unwrap_or(false);
    LOCKED.store(locked, Ordering::Relaxed);
    locked
}

/// Raw mouse counts are small and unaccelerated, so a locked pointer would feel slow. This
/// multiplies them. Raise it for a faster pointer, lower it for a slower one.
const RAW_GAIN: f64 = 3.0;

/// True while the real pointer is locked and its raw movement drives the other device.
static LOCKED: AtomicBool = AtomicBool::new(false);

/// Raw mouse movement is not tied to a window position, so a locked pointer that sits at
/// the edge of the window still reports how far the hand moved.
struct RawMotion;

impl slint::winit_030::CustomApplicationHandler for RawMotion {
    fn device_event(
        &mut self,
        _event_loop: &slint::winit_030::winit::event_loop::ActiveEventLoop,
        _device_id: slint::winit_030::winit::event::DeviceId,
        event: slint::winit_030::winit::event::DeviceEvent,
    ) -> slint::winit_030::EventResult {
        if let slint::winit_030::winit::event::DeviceEvent::MouseMotion { delta } = event
            && LOCKED.load(Ordering::Relaxed)
        {
            REMOTE.with(|r| {
                if let Ok(mut r) = r.try_borrow_mut()
                    && let Some(r) = r.as_mut()
                {
                    r.raw_move(delta.0, delta.1);
                }
            });
        }
        slint::winit_030::EventResult::Propagate
    }
}

/// Choose the window backend, with [`RawMotion`] attached. Call once, before any window.
pub(crate) fn select_backend() -> Result<(), slint::PlatformError> {
    slint::BackendSelector::new()
        .with_winit_custom_application_handler(RawMotion)
        .select()
}

fn mods(ctrl: bool, alt: bool, shift: bool, meta: bool) -> Mods {
    Mods {
        ctrl,
        alt,
        shift,
        meta,
    }
}

/// Connect the window's callbacks. Each looks the window up again, so none
/// holds the window and there is no reference cycle.
fn wire(win: &RemoteWindow) {
    win.on_pointer_move(|x, y, w, h| {
        with(|r| r.pointer(x, y, w, h));
    });
    win.on_pointer_leave(|| {
        with(|r| r.rel.leave());
    });
    win.on_pointer_button(|b, down, x, y, w, h| {
        with(|r| {
            if !r.captured {
                return;
            }
            r.pointer(x, y, w, h);
            let b = match b {
                1 => button::RIGHT,
                2 => button::MIDDLE,
                _ => button::LEFT,
            };
            if down {
                r.buttons.insert(b);
            } else {
                r.buttons.remove(&b);
            }
            let _ = r.svc.button(r.peer, b, down);
        });
    });
    win.on_pointer_scroll(|dx, dy| {
        with(|r| {
            if r.captured {
                let (sx, sy) = (scroll_units(dx), scroll_units(dy));
                if sx != 0 || sy != 0 {
                    let _ = r.svc.scroll(r.peer, sx, sy);
                }
            }
        });
    });
    win.on_key_down(|text, ctrl, alt, shift, meta| {
        with(|r| {
            if r.captured {
                let outs = r.keys.press(&text, mods(ctrl, alt, shift, meta));
                r.send(outs);
                r.refresh();
            }
        });
    });
    win.on_key_up(|text, ctrl, alt, shift, meta| {
        with(|r| {
            if r.captured {
                let outs = r.keys.release(&text, mods(ctrl, alt, shift, meta));
                r.send(outs);
            }
        });
    });
    win.on_pick_display(super::super::screen::pick_display_from_window);
    win.on_capture(|on| {
        with(|r| r.set_captured(on));
    });
    win.on_nav(|a| {
        with(|r| {
            if let Ok(a) = u32::try_from(a) {
                let _ = r.svc.nav(r.peer, a);
            }
        });
    });
    win.on_start(|| {
        with(|r| {
            r.start();
            r.refresh();
        });
    });
    win.on_stop(|| {
        with(|r| {
            r.release();
            let _ = r.svc.end(r.peer);
            r.refresh();
        });
    });
    win.window().on_close_requested(|| {
        // Take the window out of the slot first: closing runs callbacks.
        let old = REMOTE.with(|r| r.borrow_mut().take());
        if let Some(mut r) = old {
            r.release();
            let _ = r.svc.end(r.peer);
            if r.screen {
                // Closing the window of a shared screen ends the screen session.
                super::super::screen::window_closed();
            } else {
                // The remote-control tile is a switch: closing the window turns it off.
                let _ = slint::invoke_from_event_loop(crate::app::refresh_now);
            }
        }
        slint::CloseRequestResponse::HideWindow
    });
}
