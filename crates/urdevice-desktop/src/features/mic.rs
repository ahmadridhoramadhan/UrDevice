//! Phone as microphone (#28, T11.5): the desktop side.
//!
//! The microphone tile is a switch. Turning it on asks the phone for an activity (the phone's
//! user answers Always Ask), opens a virtual microphone ("UrDevice Microphone", a PipeWire source
//! on Linux, `urdevice_platform::features::webcam::mic`), and plays what the phone records into
//! it, so a call app can pick it. Turning it off, or any end of the activity, removes the
//! microphone. Where the OS has no virtual microphone yet (Windows, macOS) the switch says so and
//! stays off (R17.4).
//!
//! Audio is never stored or logged.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use slint::{ComponentHandle, SharedString, Timer, TimerMode};
use urdevice_core::Capability;
use urdevice_core::identity::DeviceId;
use urdevice_core::services::mic::{Mic as MicService, MicEvents};
use urdevice_platform::features::webcam::mic::{VirtualMic, VirtualMicError, open_virtual_mic};

use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::{Labels, MainWindow};

pub(crate) struct Mic;

/// The `Labels.mic-note` kinds (`ui/labels.slint`).
mod note {
    pub const FAILED: i32 = 0;
    pub const NOT_AVAILABLE: i32 = 20;
    pub const NOT_INSTALLED: i32 = 21;
    pub const VIRTUAL_FAILED: i32 = 22;
}

/// The note for a reason the core or the phone gave.
fn note_for(reason: &str) -> i32 {
    match reason {
        "declined" => 1,
        "denied" => 2,
        "offline" => 3,
        "unsupported" | "no_host" => 4,
        "consent_declined" => 5,
        "timeout" => 8,
        _ => note::FAILED,
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// What the UI thread knows about the one microphone session.
#[derive(Default)]
struct Session {
    peer: Option<DeviceId>,
    activity: Option<u64>,
    /// The switch is on: asked, or listening.
    on: bool,
}

thread_local! {
    static SESSION: RefCell<Session> = RefCell::new(Session::default());
    static SVC: RefCell<Option<Arc<MicService>>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<slint::Weak<MainWindow>>> = const { RefCell::new(None) };
    static TOAST: Timer = Timer::default();
}

fn service() -> Option<Arc<MicService>> {
    SVC.with(|s| s.borrow().clone())
}

/// Show a line about the microphone for a few seconds, in the toast of the main window.
fn say(kind: i32) {
    let Some(w) = WINDOW.with(|w| w.borrow().as_ref().and_then(slint::Weak::upgrade)) else {
        return;
    };
    w.set_toast(w.global::<Labels>().invoke_mic_note(kind));
    let weak = w.as_weak();
    TOAST.with(|t| {
        t.start(TimerMode::SingleShot, Duration::from_secs(6), move || {
            if let Some(w) = weak.upgrade() {
                w.set_toast(SharedString::new());
            }
        });
    });
}

/// The microphone this computer plays into, shared with the core thread that delivers chunks.
type Out = Arc<Mutex<Option<Box<dyn VirtualMic>>>>;

struct Events {
    out: Out,
}

fn on_ui(f: impl FnOnce() + Send + 'static) {
    let _ = slint::invoke_from_event_loop(f);
}

/// The session is over: the switch goes off and the microphone goes away.
fn finish(out: &Out, note: Option<i32>) {
    *lock(out) = None;
    SESSION.with(|s| *s.borrow_mut() = Session::default());
    if let Some(n) = note {
        say(n);
    }
}

impl MicEvents for Events {
    fn opened(&self, peer: DeviceId, activity: u64) {
        on_ui(move || {
            SESSION.with(|s| {
                let mut s = s.borrow_mut();
                if s.peer == Some(peer) {
                    s.activity = Some(activity);
                }
            });
        });
    }

    fn stream_started(&self, peer: DeviceId, activity: u64) {
        // A new stream: a fresh virtual microphone, so no old audio plays.
        match open_virtual_mic() {
            Ok(m) => *lock(&self.out) = Some(m),
            Err(e) => {
                let kind = match e {
                    VirtualMicError::NotAvailable => note::NOT_AVAILABLE,
                    VirtualMicError::NotInstalled => note::NOT_INSTALLED,
                    VirtualMicError::Failed(_) => note::VIRTUAL_FAILED,
                };
                tracing::info!(error = %e, "no virtual microphone");
                let out = self.out.clone();
                on_ui(move || {
                    if let Some(svc) = service() {
                        svc.stop(peer, activity);
                    }
                    finish(&out, Some(kind));
                });
            }
        }
    }

    fn chunk(&self, _peer: DeviceId, _activity: u64, pcm: Vec<u8>) {
        let mut out = lock(&self.out);
        if out.as_mut().is_some_and(|m| m.write(&pcm).is_err()) {
            // The player died: drop it; the next stream opens a new one.
            *out = None;
        }
    }

    fn error(&self, peer: DeviceId, _activity: u64, reason: &str) {
        let kind = note_for(reason);
        let out = self.out.clone();
        on_ui(move || {
            if SESSION.with(|s| s.borrow().peer) == Some(peer) {
                finish(&out, Some(kind));
            }
        });
    }

    fn ended(&self, peer: DeviceId, _activity: u64) {
        let out = self.out.clone();
        on_ui(move || {
            if SESSION.with(|s| s.borrow().peer) == Some(peer) {
                finish(&out, None);
            }
        });
    }
}

thread_local! {
    static OUT: RefCell<Option<Out>> = const { RefCell::new(None) };
}

impl Feature for Mic {
    fn id(&self) -> &'static str {
        "mic"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Mic,
            key: "mic.v1",
        }]
    }

    fn start(&self, ctx: &FeatureCtx) {
        let Some(svc) = ctx.node.service::<MicService>() else {
            return;
        };
        let out: Out = Arc::default();
        svc.events.set(Arc::new(Events { out: out.clone() }));
        SVC.with(|s| *s.borrow_mut() = Some(svc));
        OUT.with(|o| *o.borrow_mut() = Some(out));
        WINDOW.with(|w| *w.borrow_mut() = Some(ctx.window.clone()));
    }

    // The tile is a switch: on while this computer listens to the phone. There is no page.
    fn switch(&self, _ctx: &FeatureCtx, peer: &str) -> Option<bool> {
        let id = DeviceId::from_hex(peer);
        Some(SESSION.with(|s| {
            let s = s.borrow();
            s.on && s.peer == id
        }))
    }

    fn set_switch(&self, _ctx: &FeatureCtx, peer: &str, on: bool) {
        let (Some(id), Some(svc)) = (DeviceId::from_hex(peer), service()) else {
            return;
        };
        let out = OUT.with(|o| o.borrow().clone());
        if !on {
            let (p, a) = SESSION.with(|s| {
                let s = s.borrow();
                (s.peer, s.activity)
            });
            if let (Some(p), Some(a)) = (p, a) {
                svc.stop(p, a);
            }
            if let Some(out) = out {
                finish(&out, None);
            }
            return;
        }
        // One phone at a time: another phone's session ends first.
        let (p, a) = SESSION.with(|s| {
            let s = s.borrow();
            (s.peer, s.activity)
        });
        if let (Some(p), Some(a)) = (p, a)
            && p != id
        {
            svc.stop(p, a);
        }
        SESSION.with(|s| {
            *s.borrow_mut() = Session {
                peer: Some(id),
                activity: None,
                on: true,
            };
        });
        if svc.request_listen(id).is_err()
            && let Some(out) = out
        {
            finish(&out, Some(note::FAILED));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_map_to_notes() {
        assert_eq!(note_for("declined"), 1);
        assert_eq!(note_for("consent_declined"), 5);
        assert_eq!(note_for("what"), note::FAILED);
    }
}
