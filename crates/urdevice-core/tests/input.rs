//! Remote control end to end between two in-process nodes (design §9.3, R9, R10).
//!
//! Device `a` is the host with a fake input sink; device `b` is the phone.

#![allow(clippy::unwrap_used)]

mod harness;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness::{Pair, WAIT, holds_for, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::input::{
    HostStatus, Input, InputEvents, InputSink, KeyEvent, Missing, Overlay, PermStatus,
    PointerEvent, button, modifier, usage,
};
use urdevice_core::{Capability, CoreError, Mode};

/// What the host was asked to do.
#[derive(Clone, Debug, PartialEq)]
enum Did {
    Pointer(PointerEvent),
    Key(u32, bool),
    Text(String),
    Laser(Option<(u32, u32)>),
}

struct FakeSink {
    log: Mutex<Vec<Did>>,
    status: Mutex<PermStatus>,
    grants: Mutex<Vec<Grant>>,
}

impl FakeSink {
    fn new(status: PermStatus) -> Arc<Self> {
        Arc::new(Self {
            log: Mutex::default(),
            status: Mutex::new(status),
            grants: Mutex::default(),
        })
    }

    fn log(&self) -> Vec<Did> {
        self.log.lock().unwrap().clone()
    }

    fn has(&self, d: &Did) -> bool {
        self.log.lock().unwrap().contains(d)
    }

    /// The sum of all relative moves so far.
    fn moved(&self) -> (i64, i64) {
        self.log().iter().fold((0, 0), |(x, y), d| match d {
            Did::Pointer(PointerEvent::Move { dx, dy }) => (x + i64::from(*dx), y + i64::from(*dy)),
            _ => (x, y),
        })
    }
}

impl InputSink for FakeSink {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn permission_status(&self) -> PermStatus {
        *self.status.lock().unwrap()
    }

    fn pointer(&self, grant: &Grant, e: &PointerEvent) -> urdevice_core::Result<()> {
        self.grants.lock().unwrap().push(grant.clone());
        self.log.lock().unwrap().push(Did::Pointer(*e));
        Ok(())
    }

    fn key(&self, grant: &Grant, e: &KeyEvent) -> urdevice_core::Result<()> {
        self.grants.lock().unwrap().push(grant.clone());
        self.log.lock().unwrap().push(Did::Key(e.usage, e.down));
        Ok(())
    }

    fn text(&self, grant: &Grant, t: &str) -> urdevice_core::Result<()> {
        self.grants.lock().unwrap().push(grant.clone());
        self.log.lock().unwrap().push(Did::Text(t.into()));
        Ok(())
    }
}

struct FakeOverlay(Arc<FakeSink>);

impl Overlay for FakeOverlay {
    fn laser(&self, _grant: &Grant, pos: Option<(f32, f32)>) -> urdevice_core::Result<()> {
        let p = pos.map(|(x, y)| ((x * 100.0).round() as u32, (y * 100.0).round() as u32));
        self.0.log.lock().unwrap().push(Did::Laser(p));
        Ok(())
    }
}

#[derive(Default)]
struct Listener {
    sessions: Mutex<Vec<bool>>,
    statuses: Mutex<Vec<HostStatus>>,
    refused: Mutex<Vec<String>>,
    missing: Mutex<Vec<Missing>>,
}

impl InputEvents for Listener {
    fn session(&self, _peer: DeviceId, active: bool) {
        self.sessions.lock().unwrap().push(active);
    }
    fn host_status(&self, _peer: DeviceId, status: &HostStatus) {
        self.statuses.lock().unwrap().push(status.clone());
    }
    fn refused(&self, _peer: DeviceId, reason: &str) {
        self.refused.lock().unwrap().push(reason.into());
    }
    fn missing_permission(&self, _peer: DeviceId, missing: Missing) {
        self.missing.lock().unwrap().push(missing);
    }
}

struct Rig {
    p: Pair,
    host: Arc<Input>,
    ctl: Arc<Input>,
    sink: Arc<FakeSink>,
    host_events: Arc<Listener>,
    ctl_events: Arc<Listener>,
}

fn rig(status: PermStatus) -> Rig {
    let p = Pair::new();
    let host = p.a.node.service::<Input>().unwrap();
    let ctl = p.b.node.service::<Input>().unwrap();
    let sink = FakeSink::new(status);
    host.set_sink(sink.clone());
    host.set_overlay(Arc::new(FakeOverlay(sink.clone())));
    let host_events = Arc::new(Listener::default());
    let ctl_events = Arc::new(Listener::default());
    host.set_events(host_events.clone());
    ctl.set_events(ctl_events.clone());
    Rig {
        p,
        host,
        ctl,
        sink,
        host_events,
        ctl_events,
    }
}

impl Rig {
    fn host_id(&self) -> DeviceId {
        self.p.a.id()
    }

    /// Connect from the phone and approve on the laptop.
    fn start(&self) {
        self.p.a.set(Capability::Input, Mode::Ask);
        self.ctl.connect(self.host_id()).unwrap();
        self.p.a.answer_prompt(true);
        wait_until("session", WAIT, || self.ctl.is_active(self.host_id()));
        wait_until("status", WAIT, || {
            !self.ctl_events.statuses.lock().unwrap().is_empty()
        });
    }
}

#[test]
fn remote_control_is_always_ask_and_shows_the_indicator() {
    let r = rig(PermStatus::Ready);
    assert_eq!(
        r.p.b.remote_mode(&r.p.a, Capability::Input),
        Some(Mode::Ask)
    );
    r.start();
    // The host shows who controls it, with Stop (R5.4); the phone shows it too.
    wait_until("indicator", WAIT, || {
        r.p.a
            .snap()
            .activities
            .iter()
            .any(|a| a.incoming && a.capability == "input.v1")
            && r.p.b.snap().activities.iter().any(|a| !a.incoming)
    });
    assert_eq!(
        r.ctl_events.statuses.lock().unwrap()[0],
        HostStatus {
            status: PermStatus::Ready,
            backend: "fake".into()
        }
    );
}

#[test]
fn pointer_clicks_scroll_keys_and_text_reach_the_host() {
    let r = rig(PermStatus::Ready);
    r.start();
    let host = r.host_id();

    // R9.1: movement, with the many small moves of one 8 ms window merged.
    for _ in 0..50 {
        r.ctl.pointer_move(host, 3, -2).unwrap();
    }
    wait_until("moves", WAIT, || r.sink.moved() == (150, -100));

    // A click goes out after the moves before it.
    r.ctl.pointer_move(host, 1, 1).unwrap();
    r.ctl.button(host, button::LEFT, true).unwrap();
    r.ctl.button(host, button::LEFT, false).unwrap();
    r.ctl.button(host, button::RIGHT, true).unwrap();
    r.ctl.button(host, button::RIGHT, false).unwrap();
    wait_until("clicks", WAIT, || {
        r.sink.has(&Did::Pointer(PointerEvent::Button {
            button: button::RIGHT,
            down: false,
        }))
    });
    let log = r.sink.log();
    let at = |d: &Did| log.iter().position(|x| x == d).unwrap();
    assert!(
        at(&Did::Pointer(PointerEvent::Move { dx: 1, dy: 1 }))
            < at(&Did::Pointer(PointerEvent::Button {
                button: button::LEFT,
                down: true
            }))
    );

    r.ctl.scroll(host, 0, 240).unwrap();
    wait_until("scroll", WAIT, || {
        r.sink
            .has(&Did::Pointer(PointerEvent::Scroll { dx: 0, dy: 240 }))
    });

    // R10: Unicode text arrives as text, special keys and shortcuts as keys.
    r.ctl.text(host, "héllo 世界 😀").unwrap();
    r.ctl.tap_key(host, usage::ENTER, 0).unwrap();
    r.ctl
        .tap_key(host, usage::A + 2, modifier::CTRL | modifier::SHIFT)
        .unwrap();
    wait_until("keys", WAIT, || {
        r.sink.has(&Did::Key(usage::LEFT_CTRL, false))
    });
    let log = r.sink.log();
    assert!(log.contains(&Did::Text("héllo 世界 😀".into())));
    let keys: Vec<_> = log
        .iter()
        .filter_map(|d| match d {
            Did::Key(k, down) => Some((*k, *down)),
            _ => None,
        })
        .collect();
    assert_eq!(
        keys,
        vec![
            (usage::ENTER, true),
            (usage::ENTER, false),
            (usage::LEFT_CTRL, true),
            (usage::LEFT_SHIFT, true),
            (usage::A + 2, true),
            (usage::A + 2, false),
            (usage::LEFT_CTRL, false),
            (usage::LEFT_SHIFT, false),
        ]
    );
    // Every call the adapter got carried a grant for this activity (R5).
    let grants = r.sink.grants.lock().unwrap();
    assert!(
        grants
            .iter()
            .all(|g| g.capability() == Capability::Input && g.activity() != 0)
    );
}

#[test]
fn long_text_is_split_and_arrives_whole() {
    let r = rig(PermStatus::Ready);
    r.start();
    let text = "世".repeat(5000); // 15,000 bytes
    r.ctl.text(r.host_id(), &text).unwrap();
    wait_until("text", WAIT, || {
        let got: String = r
            .sink
            .log()
            .into_iter()
            .filter_map(|d| match d {
                Did::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        got == text
    });
}

#[test]
fn the_laser_shows_moves_and_hides() {
    let r = rig(PermStatus::Ready);
    r.start();
    let host = r.host_id();
    r.ctl.laser(host, 0.25, 0.5, true).unwrap();
    wait_until("laser on", WAIT, || r.sink.has(&Did::Laser(Some((25, 50)))));
    r.ctl.laser(host, 1.5, -1.0, true).unwrap();
    wait_until("laser clamped", WAIT, || {
        r.sink.has(&Did::Laser(Some((100, 0))))
    });
    r.ctl.laser(host, 0.0, 0.0, false).unwrap();
    wait_until("laser off", WAIT, || r.sink.has(&Did::Laser(None)));
}

#[test]
fn stop_on_the_host_lets_go_of_held_keys_and_buttons() {
    let r = rig(PermStatus::Ready);
    r.start();
    let host = r.host_id();
    r.ctl.button(host, button::LEFT, true).unwrap();
    r.ctl.key(host, usage::A, true, modifier::ALT).unwrap();
    r.ctl.laser(host, 0.5, 0.5, true).unwrap();
    wait_until("held", WAIT, || {
        r.sink.has(&Did::Key(usage::A, true)) && r.sink.has(&Did::Laser(Some((50, 50))))
    });

    // Stop on the controlled desktop (R5.4, R5.5).
    let id =
        r.p.a
            .snap()
            .activities
            .iter()
            .find(|a| a.incoming)
            .unwrap()
            .id;
    r.p.a.node.stop_activity(id);
    wait_until("released", WAIT, || {
        r.sink.has(&Did::Key(usage::A, false))
            && r.sink.has(&Did::Key(usage::LEFT_ALT, false))
            && r.sink.has(&Did::Pointer(PointerEvent::Button {
                button: button::LEFT,
                down: false,
            }))
            && r.sink.has(&Did::Laser(None))
    });
    wait_until("controller told", WAIT, || !r.ctl.is_active(host));
    assert_eq!(
        r.ctl_events.sessions.lock().unwrap().as_slice(),
        [true, false]
    );

    // Nothing more is accepted for the ended activity, and the phone cannot queue.
    assert!(matches!(
        r.ctl.pointer_move(host, 5, 5),
        Err(CoreError::InvalidInput(_))
    ));
    let before = r.sink.log().len();
    holds_for("no late events", 300, || r.sink.log().len() == before);
}

#[test]
fn the_phone_can_end_the_session() {
    let r = rig(PermStatus::Ready);
    r.start();
    let host = r.host_id();
    r.ctl.key(host, usage::LEFT_SHIFT, true, 0).unwrap();
    wait_until("shift down", WAIT, || {
        r.sink.has(&Did::Key(usage::LEFT_SHIFT, true))
    });
    r.ctl.end(host).unwrap();
    wait_until("shift released", WAIT, || {
        r.sink.has(&Did::Key(usage::LEFT_SHIFT, false))
    });
    wait_until("indicator gone", WAIT, || {
        r.p.a.snap().activities.is_empty() && r.p.b.snap().activities.is_empty()
    });
    assert!(!r.ctl.is_active(host));
}

#[test]
fn deny_and_decline_are_reported_and_nothing_is_injected() {
    let r = rig(PermStatus::Ready);
    r.p.a.set(Capability::Input, Mode::Deny);
    wait_until("deny known", WAIT, || {
        r.p.b.remote_mode(&r.p.a, Capability::Input) == Some(Mode::Deny)
    });
    r.ctl.connect(r.host_id()).unwrap();
    wait_until("denied", WAIT, || {
        r.ctl_events
            .refused
            .lock()
            .unwrap()
            .iter()
            .any(|x| x == "denied")
    });
    assert!(!r.ctl.is_active(r.host_id()));

    r.p.a.set(Capability::Input, Mode::Ask);
    wait_until("ask known", WAIT, || {
        r.p.b.remote_mode(&r.p.a, Capability::Input) == Some(Mode::Ask)
    });
    r.ctl_events.refused.lock().unwrap().clear();
    r.ctl.connect(r.host_id()).unwrap();
    r.p.a.answer_prompt(false);
    wait_until("declined", WAIT, || {
        r.ctl_events
            .refused
            .lock()
            .unwrap()
            .iter()
            .any(|x| x == "declined")
    });
    assert!(r.sink.log().is_empty());
}

#[test]
fn allow_still_needs_an_activity_for_remote_control() {
    // Watched capability: the setting Allow does not skip the indicator.
    let r = rig(PermStatus::Ready);
    r.p.a.set(Capability::Input, Mode::Allow);
    wait_until("allow known", WAIT, || {
        r.p.b.remote_mode(&r.p.a, Capability::Input) == Some(Mode::Allow)
    });
    r.ctl.connect(r.host_id()).unwrap();
    wait_until("session", WAIT, || r.ctl.is_active(r.host_id()));
    wait_until("indicator", WAIT, || {
        r.p.a.snap().activities.iter().any(|a| a.incoming)
    });
}

#[test]
fn a_missing_os_permission_is_reported_to_both_sides() {
    let r = rig(PermStatus::Missing(Missing::Accessibility));
    r.start();
    assert_eq!(
        r.ctl_events.statuses.lock().unwrap()[0].status,
        PermStatus::Missing(Missing::Accessibility)
    );
    wait_until("host told", WAIT, || {
        r.host_events.missing.lock().unwrap().as_slice() == [Missing::Accessibility]
    });
}

#[test]
fn a_host_without_an_adapter_says_unavailable() {
    let r = rig(PermStatus::Ready);
    r.host.set_sink(Arc::new(NoSink));
    r.start();
    assert_eq!(
        r.ctl_events.statuses.lock().unwrap()[0].status,
        PermStatus::Unavailable
    );
}

struct NoSink;

impl InputSink for NoSink {
    fn name(&self) -> &'static str {
        "none"
    }
    fn permission_status(&self) -> PermStatus {
        PermStatus::Unavailable
    }
    fn pointer(&self, _: &Grant, _: &PointerEvent) -> urdevice_core::Result<()> {
        Err(CoreError::unsupported("input", "test"))
    }
    fn key(&self, _: &Grant, _: &KeyEvent) -> urdevice_core::Result<()> {
        Err(CoreError::unsupported("input", "test"))
    }
    fn text(&self, _: &Grant, _: &str) -> urdevice_core::Result<()> {
        Err(CoreError::unsupported("input", "test"))
    }
}

#[test]
fn bad_calls_are_rejected_on_the_phone() {
    let r = rig(PermStatus::Ready);
    // No session yet.
    assert!(r.ctl.pointer_move(r.host_id(), 1, 1).is_err());
    r.start();
    assert!(r.ctl.key(r.host_id(), 0x03, true, 0).is_err());
    assert!(r.ctl.key(r.host_id(), 0x1234, true, 0).is_err());
    // Control characters are dropped from text, not sent.
    r.ctl.text(r.host_id(), "a\u{1b}b\0").unwrap();
    wait_until("text", WAIT, || r.sink.has(&Did::Text("ab".into())));
    std::thread::sleep(Duration::from_millis(50));
}
