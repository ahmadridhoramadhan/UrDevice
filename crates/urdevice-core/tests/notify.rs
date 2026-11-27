//! Notification mirroring end to end between two in-process nodes (design §18.2, R18).

#![allow(clippy::unwrap_used)]

mod harness;

use std::sync::{Arc, Mutex};

use harness::{Dev, Pair, WAIT, holds_for, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::notify::{
    Notify, NotifyApp, NotifyEvents, NotifyHost, NotifyItem, NotifyState,
};
use urdevice_core::{Capability, Mode};

/// A phone with a few notifications.
#[derive(Default)]
struct FakePhone {
    state: Mutex<NotifyState>,
}

impl FakePhone {
    fn new() -> Arc<Self> {
        let p = Arc::new(Self::default());
        *p.state.lock().unwrap() = NotifyState {
            items: vec![
                item("1", "com.chat", "Chat", "Ana", "hello", 3000),
                item("2", "com.mail", "Mail", "Boss", "report", 2000),
                item("3", "com.chat", "Chat", "Ben", "lunch?", 1000),
            ],
            apps: vec![app("com.chat", "Chat"), app("com.mail", "Mail")],
            unavailable: String::new(),
        };
        p
    }

    fn edit(&self, f: impl FnOnce(&mut NotifyState)) {
        f(&mut self.state.lock().unwrap());
    }
}

fn item(key: &str, id: &str, name: &str, title: &str, text: &str, at: u64) -> NotifyItem {
    NotifyItem {
        key: key.into(),
        app_id: id.into(),
        app_name: name.into(),
        title: title.into(),
        text: text.into(),
        posted_ms: at,
        hidden: false,
    }
}

fn app(id: &str, name: &str) -> NotifyApp {
    NotifyApp {
        id: id.into(),
        name: name.into(),
    }
}

impl NotifyHost for FakePhone {
    fn state(&self) -> NotifyState {
        self.state.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct Events {
    refused: Mutex<Vec<String>>,
}

impl NotifyEvents for Events {
    fn updated(&self, _peer: DeviceId) {}

    fn refused(&self, _peer: DeviceId, reason: &str) {
        self.refused.lock().unwrap().push(reason.into());
    }
}

struct Rig {
    p: Pair,
    phone: Arc<FakePhone>,
    events: Arc<Events>,
    /// The service on the desktop (`a`).
    desk: Arc<Notify>,
}

impl Rig {
    fn new() -> Self {
        let p = Pair::with(
            Dev::new("Laptop"),
            Dev::with("Phone", "android", Vec::new()),
        );
        let phone = FakePhone::new();
        let events = Arc::new(Events::default());
        let desk = p.a.node.service::<Notify>().unwrap();
        p.b.node
            .service::<Notify>()
            .unwrap()
            .set_host(phone.clone());
        desk.set_events(events.clone());
        // The phone's user allows it, so no prompt is needed.
        p.b.set(Capability::Notify, Mode::Allow);
        wait_until("desktop sees allow", WAIT, || {
            p.a.remote_mode(&p.b, Capability::Notify) == Some(Mode::Allow)
        });
        Self {
            p,
            phone,
            events,
            desk,
        }
    }

    fn phone_id(&self) -> DeviceId {
        self.p.b.id()
    }

    fn state(&self) -> Option<NotifyState> {
        self.desk.state_of(self.phone_id())
    }

    fn keys(&self) -> Vec<String> {
        self.state()
            .map(|s| s.items.into_iter().map(|i| i.key).collect())
            .unwrap_or_default()
    }
}

#[test]
fn nothing_is_sent_until_mirroring_is_on_and_apps_are_picked() {
    let r = Rig::new();
    assert!(!r.desk.is_on(r.phone_id()), "off by default (R18.2)");
    assert!(r.state().is_none());
    // On, but no app picked: the list of apps arrives, no notification does.
    r.desk.subscribe(r.phone_id(), true).unwrap();
    wait_until("first state", WAIT, || r.state().is_some());
    let st = r.state().unwrap();
    assert!(st.items.is_empty(), "newly seen apps are not mirrored");
    assert_eq!(st.apps.len(), 2);
    assert_eq!(r.desk.known_apps(r.phone_id()).len(), 2);
}

#[test]
fn only_the_picked_apps_arrive_newest_first() {
    let r = Rig::new();
    r.desk
        .set_apps(r.phone_id(), &["com.chat".to_owned()])
        .unwrap();
    r.desk.subscribe(r.phone_id(), true).unwrap();
    wait_until("chat only", WAIT, || r.keys() == ["1", "3"]);
    // Picking the other app too sends its notifications.
    r.desk
        .set_apps(
            r.phone_id(),
            &["com.chat".to_owned(), "com.mail".to_owned()],
        )
        .unwrap();
    wait_until("both apps", WAIT, || r.keys() == ["1", "2", "3"]);
    assert_eq!(r.desk.picked_apps(r.phone_id()).len(), 2);
}

#[test]
fn a_notification_dismissed_on_the_phone_leaves_the_desktop() {
    let r = Rig::new();
    r.desk
        .set_apps(r.phone_id(), &["com.chat".to_owned()])
        .unwrap();
    r.desk.subscribe(r.phone_id(), true).unwrap();
    wait_until("first state", WAIT, || r.keys() == ["1", "3"]);
    r.phone.edit(|s| s.items.retain(|i| i.key != "1"));
    wait_until("dismissed", WAIT, || r.keys() == ["3"]);
    r.phone.edit(|s| {
        s.items
            .insert(0, item("4", "com.chat", "Chat", "Cy", "new", 4000));
    });
    wait_until("new one", WAIT, || r.keys() == ["4", "3"]);
}

#[test]
fn turning_mirroring_off_stops_and_drops_everything() {
    let r = Rig::new();
    r.desk
        .set_apps(r.phone_id(), &["com.chat".to_owned()])
        .unwrap();
    r.desk.subscribe(r.phone_id(), true).unwrap();
    wait_until("first state", WAIT, || !r.keys().is_empty());
    r.desk.subscribe(r.phone_id(), false).unwrap();
    assert!(r.state().is_none(), "content is dropped when it is off");
    assert!(!r.desk.is_on(r.phone_id()));
    holds_for("no state while off", 2500, || {
        r.phone.edit(|s| s.items[0].title = "While off".into());
        r.state().is_none()
    });
}

#[test]
fn deny_on_the_phone_stops_pushes() {
    let r = Rig::new();
    r.desk
        .set_apps(r.phone_id(), &["com.chat".to_owned()])
        .unwrap();
    r.desk.subscribe(r.phone_id(), true).unwrap();
    wait_until("first state", WAIT, || !r.keys().is_empty());
    r.p.b.set(Capability::Notify, Mode::Deny);
    wait_until("desktop sees deny", WAIT, || {
        r.p.a.remote_mode(&r.p.b, Capability::Notify) == Some(Mode::Deny)
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    r.phone.edit(|s| s.items[0].title = "After deny".into());
    holds_for("no push after deny", 2500, || {
        r.state()
            .is_none_or(|s| s.items.iter().all(|i| i.title != "After deny"))
    });
}

#[test]
fn always_ask_needs_one_approval_on_the_phone() {
    let r = Rig::new();
    r.p.b.set(Capability::Notify, Mode::Ask);
    wait_until("desktop sees ask", WAIT, || {
        r.p.a.remote_mode(&r.p.b, Capability::Notify) == Some(Mode::Ask)
    });
    r.desk
        .set_apps(r.phone_id(), &["com.chat".to_owned()])
        .unwrap();
    r.desk.subscribe(r.phone_id(), true).unwrap();
    r.p.b.answer_prompt(true);
    wait_until("state after approval", WAIT, || !r.keys().is_empty());
}

#[test]
fn a_declined_prompt_is_reported() {
    let r = Rig::new();
    r.p.b.set(Capability::Notify, Mode::Ask);
    wait_until("desktop sees ask", WAIT, || {
        r.p.a.remote_mode(&r.p.b, Capability::Notify) == Some(Mode::Ask)
    });
    r.desk.subscribe(r.phone_id(), true).unwrap();
    r.p.b.answer_prompt(false);
    wait_until("declined", WAIT, || {
        r.events
            .refused
            .lock()
            .unwrap()
            .iter()
            .any(|x| x == "declined")
    });
    assert!(r.state().is_none());
}

#[test]
fn a_phone_without_an_adapter_answers_with_an_empty_state() {
    let p = Pair::with(
        Dev::new("Laptop"),
        Dev::with("Phone", "android", Vec::new()),
    );
    p.b.set(Capability::Notify, Mode::Allow);
    wait_until("allow", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Notify) == Some(Mode::Allow)
    });
    let desk = p.a.node.service::<Notify>().unwrap();
    desk.set_apps(p.b.id(), &["com.chat".to_owned()]).unwrap();
    desk.subscribe(p.b.id(), true).unwrap();
    wait_until("state", WAIT, || desk.state_of(p.b.id()).is_some());
    let st = desk.state_of(p.b.id()).unwrap();
    assert!(st.items.is_empty());
    assert_eq!(st.unavailable, "unsupported");
}

#[test]
fn a_desktop_does_not_offer_notifications() {
    let p = Pair::new();
    assert!(
        !p.a.snap().peers[0]
            .remote_permissions
            .iter()
            .any(|r| r.capability == Capability::Notify)
    );
}
