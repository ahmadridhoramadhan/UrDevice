//! Remote control into Android and between desktops, end to end between two
//! in-process nodes (#19, design §9.3, R9.1, R9.3, R10.2, R10.3).
//!
//! Device `a` is the host. In the Android tests it runs the real
//! [`TouchSink`] over a fake accessibility service and keyboard; in the
//! desktop test it runs a fake desktop sink. Device `b` is the controller and
//! uses the same capture helpers as the remote-control window.

#![allow(clippy::unwrap_used)]

mod harness;

use std::sync::{Arc, Mutex};

use harness::{Pair, WAIT, holds_for, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::input::{
    Gesture, HostStatus, Input, InputEvents, InputSink, KeyEvent, KeyOut, KeyboardCapture, Missing,
    Mods, PermStatus, PointerEvent, TouchHost, TouchSink, button, modifier, nav, usage,
};
use urdevice_core::{Capability, Mode, Result};

const SCREEN: (u32, u32) = (1080, 2400);

/// Datagrams (positions) and the control stream (buttons) may overtake each
/// other on a real path, and timers are coarse on Windows: let positions land
/// before a button follows.
fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(80));
}

#[derive(Clone, Debug, PartialEq)]
enum Did {
    Gesture(Gesture),
    Nav(u32),
    Text(String),
    Key(u32, bool, u32),
}

/// The phone's accessibility service and keyboard.
struct Phone {
    gestures: Mutex<bool>,
    text: Mutex<bool>,
    log: Mutex<Vec<Did>>,
}

impl Phone {
    fn new(gestures: bool, text: bool) -> Arc<Self> {
        Arc::new(Self {
            gestures: Mutex::new(gestures),
            text: Mutex::new(text),
            log: Mutex::default(),
        })
    }
    fn log(&self) -> Vec<Did> {
        self.log.lock().unwrap().clone()
    }
    fn has(&self, d: &Did) -> bool {
        self.log().contains(d)
    }
}

impl TouchHost for Phone {
    fn gestures_ready(&self) -> bool {
        *self.gestures.lock().unwrap()
    }
    fn text_ready(&self) -> bool {
        *self.text.lock().unwrap()
    }
    fn display_size(&self) -> (u32, u32) {
        SCREEN
    }
    fn gesture(&self, _: &Grant, g: &Gesture) -> Result<()> {
        self.log.lock().unwrap().push(Did::Gesture(g.clone()));
        Ok(())
    }
    fn nav(&self, _: &Grant, a: u32) -> Result<()> {
        self.log.lock().unwrap().push(Did::Nav(a));
        Ok(())
    }
    fn text(&self, _: &Grant, t: &str) -> Result<()> {
        self.log.lock().unwrap().push(Did::Text(t.into()));
        Ok(())
    }
    fn key(&self, _: &Grant, c: u32, d: bool, m: u32) -> Result<()> {
        self.log.lock().unwrap().push(Did::Key(c, d, m));
        Ok(())
    }
}

#[derive(Default)]
struct Listener {
    statuses: Mutex<Vec<HostStatus>>,
    missing: Mutex<Vec<Missing>>,
}

impl InputEvents for Listener {
    fn host_status(&self, _peer: DeviceId, status: &HostStatus) {
        self.statuses.lock().unwrap().push(status.clone());
    }
    fn missing_permission(&self, _peer: DeviceId, missing: Missing) {
        self.missing.lock().unwrap().push(missing);
    }
}

struct Rig {
    p: Pair,
    ctl: Arc<Input>,
    host_events: Arc<Listener>,
    ctl_events: Arc<Listener>,
}

fn rig(sink: Arc<dyn InputSink>) -> Rig {
    let p = Pair::new();
    let host = p.a.node.service::<Input>().unwrap();
    let ctl = p.b.node.service::<Input>().unwrap();
    host.set_sink(sink);
    let host_events = Arc::new(Listener::default());
    let ctl_events = Arc::new(Listener::default());
    host.set_events(host_events.clone());
    ctl.set_events(ctl_events.clone());
    Rig {
        p,
        ctl,
        host_events,
        ctl_events,
    }
}

impl Rig {
    fn id(&self) -> DeviceId {
        self.p.a.id()
    }

    fn start(&self) {
        self.p.a.set(Capability::Input, Mode::Ask);
        self.ctl.connect(self.id()).unwrap();
        self.p.a.answer_prompt(true);
        wait_until("session", WAIT, || self.ctl.is_active(self.id()));
        wait_until("status", WAIT, || {
            !self.ctl_events.statuses.lock().unwrap().is_empty()
        });
    }
}

fn android(gestures: bool, text: bool) -> (Rig, Arc<Phone>) {
    let phone = Phone::new(gestures, text);
    let r = rig(Arc::new(TouchSink::new(phone.clone())));
    (r, phone)
}

#[test]
fn a_desktop_taps_swipes_and_uses_back_home_recents_on_android() {
    let (r, phone) = android(true, true);
    r.start();
    let id = r.id();

    // The controller learns the phone is ready and how big its screen is.
    let st = r.ctl_events.statuses.lock().unwrap()[0].clone();
    assert_eq!(st.status, PermStatus::Ready);
    assert_eq!(st.backend, "android");
    assert_eq!(st.display, Some(SCREEN));
    assert_eq!(st.text_missing, None);

    // A click is a tap at the place the pointer was.
    r.ctl.pointer_abs(id, 32768, 16384, 0).unwrap();
    settle();
    r.ctl.button(id, button::LEFT, true).unwrap();
    r.ctl.button(id, button::LEFT, false).unwrap();
    wait_until("tap", WAIT, || {
        phone
            .log()
            .iter()
            .any(|d| matches!(d, Did::Gesture(Gesture::Tap { .. })))
    });

    // A drag is one swipe from where it began to where it ended.
    r.ctl.pointer_abs(id, 32768, 49152, 0).unwrap();
    settle();
    r.ctl.button(id, button::LEFT, true).unwrap();
    for i in 1..=10u32 {
        r.ctl.pointer_abs(id, 32768, 49152 - i * 3000, 0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    settle();
    r.ctl.button(id, button::LEFT, false).unwrap();
    // Positions travel as datagrams and the button on the stream, so the path
    // may start a step or two after the first position: check the direction.
    let swiped = |d: &Did| match d {
        Did::Gesture(Gesture::Swipe { points, .. }) => {
            points.first().is_some_and(|p| p.1 > 1400) && points.last().is_some_and(|p| p.1 < 1100)
        }
        _ => false,
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(WAIT);
    while !phone.log().iter().any(swiped) {
        assert!(
            std::time::Instant::now() < deadline,
            "no swipe; the phone saw {:?}",
            phone.log()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // Back, Home, Recents, then a right click is Back as well.
    for a in [nav::BACK, nav::HOME, nav::RECENTS] {
        r.ctl.nav(id, a).unwrap();
    }
    wait_until("nav", WAIT, || phone.has(&Did::Nav(nav::RECENTS)));
    let navs: Vec<_> = phone
        .log()
        .into_iter()
        .filter_map(|d| match d {
            Did::Nav(a) => Some(a),
            _ => None,
        })
        .collect();
    assert_eq!(navs, [nav::BACK, nav::HOME, nav::RECENTS]);
    r.ctl.button(id, button::RIGHT, true).unwrap();
    r.ctl.button(id, button::RIGHT, false).unwrap();
    wait_until("right click is back", WAIT, || {
        phone
            .log()
            .iter()
            .filter(|d| **d == Did::Nav(nav::BACK))
            .count()
            == 2
    });

    // The phone shows the indicator with Stop while it is controlled (R5.4).
    assert!(
        r.p.a
            .snap()
            .activities
            .iter()
            .any(|a| a.incoming && a.capability == "input.v1")
    );
    // A bad action is refused on the controller.
    assert!(r.ctl.nav(id, 99).is_err());
}

#[test]
fn a_desktop_types_into_android_through_the_keyboard() {
    let (r, phone) = android(true, true);
    r.start();
    let id = r.id();

    // The window's capture turns what the user types into text and shortcuts.
    let mut cap = KeyboardCapture::default();
    let send = |outs: Vec<KeyOut>| {
        for o in outs {
            match o {
                KeyOut::Text(t) => r.ctl.text(id, &t).unwrap(),
                KeyOut::Key {
                    usage,
                    down,
                    modifiers,
                } => r.ctl.key(id, usage, down, modifiers).unwrap(),
                KeyOut::Release => unreachable!(),
            }
        }
    };
    send(cap.press("é", Mods::default()));
    send(cap.press("世", Mods::default()));
    send(cap.press("\n", Mods::default()));
    send(cap.release("\n", Mods::default()));
    let ctrl = Mods {
        ctrl: true,
        ..Mods::default()
    };
    send(cap.press("a", ctrl));
    send(cap.release("a", Mods::default()));

    wait_until("typed", WAIT, || phone.has(&Did::Key(29, false, 0x1000)));
    assert_eq!(
        phone.log(),
        vec![
            Did::Text("é".into()),
            Did::Text("世".into()),
            Did::Key(66, true, 0),
            Did::Key(66, false, 0),
            // Ctrl+A reaches the field as KEYCODE_A with Ctrl held.
            Did::Key(29, true, 0x1000),
            Did::Key(29, false, 0x1000),
        ]
    );
}

#[test]
fn a_phone_without_the_service_says_what_to_turn_on_and_does_nothing() {
    let (r, phone) = android(false, false);
    r.start();
    let st = r.ctl_events.statuses.lock().unwrap()[0].clone();
    assert_eq!(
        st.status,
        PermStatus::Missing(Missing::AccessibilityService)
    );
    // The phone's own user is told too, to open Android's settings.
    wait_until("host told", WAIT, || {
        r.host_events
            .missing
            .lock()
            .unwrap()
            .contains(&Missing::AccessibilityService)
    });
    let id = r.id();
    r.ctl.button(id, button::LEFT, true).unwrap();
    r.ctl.button(id, button::LEFT, false).unwrap();
    r.ctl.nav(id, nav::HOME).unwrap();
    r.ctl.text(id, "typed words").unwrap();
    holds_for("nothing runs", 300, || phone.log().is_empty());
}

#[test]
fn the_keyboard_missing_is_reported_while_taps_still_work() {
    let (r, phone) = android(true, false);
    r.start();
    let st = r.ctl_events.statuses.lock().unwrap()[0].clone();
    assert_eq!(st.status, PermStatus::Ready);
    assert_eq!(st.text_missing, Some(Missing::InputMethod));
    wait_until("host told", WAIT, || {
        r.host_events
            .missing
            .lock()
            .unwrap()
            .contains(&Missing::InputMethod)
    });
    let id = r.id();
    r.ctl.text(id, "dropped").unwrap();
    r.ctl.nav(id, nav::HOME).unwrap();
    wait_until("home", WAIT, || phone.has(&Did::Nav(nav::HOME)));
    assert!(!phone.log().iter().any(|d| matches!(d, Did::Text(_))));
}

#[test]
fn stop_on_the_phone_cancels_a_swipe_that_was_not_finished() {
    let (r, phone) = android(true, true);
    r.start();
    let id = r.id();
    r.ctl.pointer_abs(id, 1000, 1000, 0).unwrap();
    settle();
    r.ctl.button(id, button::LEFT, true).unwrap();
    r.ctl.pointer_move(id, 0, 400).unwrap();
    settle();
    r.ctl.key(id, usage::A, true, 0).unwrap();
    wait_until("key held", WAIT, || phone.has(&Did::Key(29, true, 0)));

    let act =
        r.p.a
            .snap()
            .activities
            .iter()
            .find(|a| a.incoming)
            .unwrap()
            .id;
    r.p.a.node.stop_activity(act);
    wait_until("key released", WAIT, || phone.has(&Did::Key(29, false, 0)));
    wait_until("controller told", WAIT, || !r.ctl.is_active(id));
    // The half-made swipe never ran.
    holds_for("no gesture after Stop", 300, || {
        !phone.log().iter().any(|d| matches!(d, Did::Gesture(_)))
    });
}

// --- desktop to desktop -----------------------------------------------------

#[derive(Default)]
struct DesktopSink {
    log: Mutex<Vec<String>>,
}

impl InputSink for DesktopSink {
    fn name(&self) -> &'static str {
        "x11"
    }
    fn permission_status(&self) -> PermStatus {
        PermStatus::Ready
    }
    fn pointer(&self, _: &Grant, e: &PointerEvent) -> Result<()> {
        self.log.lock().unwrap().push(format!("{e:?}"));
        Ok(())
    }
    fn key(&self, _: &Grant, e: &KeyEvent) -> Result<()> {
        self.log
            .lock()
            .unwrap()
            .push(format!("key {:#x} {}", e.usage, e.down));
        Ok(())
    }
    fn text(&self, _: &Grant, t: &str) -> Result<()> {
        self.log.lock().unwrap().push(format!("text {t}"));
        Ok(())
    }
}

#[test]
fn a_desktop_controls_another_desktop_with_pointer_and_keys() {
    let sink = Arc::new(DesktopSink::default());
    let r = rig(sink.clone());
    r.start();
    let id = r.id();
    assert_eq!(r.ctl_events.statuses.lock().unwrap()[0].backend, "x11");

    // The remote-control window sends where the pointer is in the window.
    r.ctl.pointer_abs(id, 65535, 0, 0).unwrap();
    r.ctl.button(id, button::LEFT, true).unwrap();
    r.ctl.button(id, button::LEFT, false).unwrap();
    r.ctl.scroll(id, 0, 120).unwrap();

    let mut cap = KeyboardCapture::default();
    for o in cap.press("h", Mods::default()) {
        if let KeyOut::Text(t) = o {
            r.ctl.text(id, &t).unwrap();
        }
    }
    let both = Mods {
        ctrl: true,
        alt: true,
        ..Mods::default()
    };
    // The release hotkey is recognised and never sent.
    assert_eq!(cap.press("g", both), vec![KeyOut::Release]);
    r.ctl.tap_key(id, usage::ENTER, modifier::SHIFT).unwrap();

    // Positions and the wheel travel as datagrams; wait for each.
    for what in [
        "key 0xe1 false",
        "Abs { x: 65535, y: 0",
        "Scroll { dx: 0, dy: 120 }",
        "text h",
    ] {
        wait_until(what, WAIT, || {
            sink.log.lock().unwrap().iter().any(|l| l.contains(what))
        });
    }
    let log = sink.log.lock().unwrap().clone();
    assert!(log.iter().any(|l| l.contains("Abs { x: 65535, y: 0")));
    assert!(log.iter().any(|l| l == "text h"));
    assert!(log.iter().any(|l| l.contains("Scroll { dx: 0, dy: 120 }")));
    // Nothing for the hotkey: no `g`, and no Ctrl or Alt press.
    assert!(!log.iter().any(|l| l.contains("key 0xa ")));
    assert!(!log.iter().any(|l| l.contains("key 0xe0 true")));
    // A desktop host ignores Back, Home and Recents instead of failing.
    r.ctl.nav(id, nav::HOME).unwrap();
    holds_for("still alive", 200, || r.ctl.is_active(id));
}
