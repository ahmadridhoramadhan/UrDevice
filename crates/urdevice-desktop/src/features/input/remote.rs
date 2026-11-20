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

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;

use slint::ComponentHandle as _;
use urdevice_core::identity::DeviceId;
use urdevice_core::services::input::{
    HostStatus, Input as Service, KeyOut, KeyboardCapture, Mods, PermStatus, RelativePointer,
    button, host_moves_by_distance, normalize, scroll_units,
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
    let same = REMOTE.with(|r| r.borrow().as_ref().is_some_and(|x| x.peer == peer));
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
            });
        });
    }
    with(|r| {
        r.win.set_device_name(r.name.as_str().into());
        let _ = r.win.show();
        r.start();
        r.refresh();
    });
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
            }
            r.refresh();
        }
    });
}

/// `peer` reported what it can do.
pub(super) fn host_status(peer: DeviceId, status: HostStatus) {
    with(|r| {
        if r.peer == peer {
            r.by_distance = host_moves_by_distance(&status.backend);
            r.rel.leave();
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
        let status = if let Some(why) = &self.refused {
            match why.as_str() {
                "denied" => format!("Remote control is turned off on {name}."),
                "declined" => format!("{name} did not allow remote control."),
                "timeout" => format!("{name} did not answer in time."),
                "unsupported" => format!("{name} cannot be controlled from here."),
                _ => format!("{name} is offline."),
            }
        } else if self.svc.is_connecting(self.peer) {
            format!("Waiting for {name} to allow remote control…")
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
                    PermStatus::Missing(_) => format!(
                        "{name} needs a permission first. Follow the steps on {name}, then start again."
                    ),
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
        let android = self.host.as_ref().is_some_and(|h| h.backend == "android");
        self.win.set_show_nav(android);
        if let Some((w, h)) = self.host.as_ref().and_then(|h| h.display)
            && h > 0
        {
            self.win.set_aspect(w as f32 / h as f32);
        }
    }

    fn set_captured(&mut self, on: bool) {
        if on && !self.active() {
            return;
        }
        if !on {
            self.release_keys();
        }
        self.captured = on;
        self.rel.leave();
        self.win.set_captured(on);
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

    fn pointer(&mut self, x: f32, y: f32, w: f32, h: f32) {
        if !self.captured {
            return;
        }
        if self.by_distance {
            if let Some((dx, dy)) = self.rel.moved(x, y) {
                let _ = self.svc.pointer_move(self.peer, dx, dy);
            }
        } else {
            let _ = self
                .svc
                .pointer_abs(self.peer, normalize(x, w), normalize(y, h), 0);
        }
    }
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
        }
        slint::CloseRequestResponse::HideWindow
    });
}
