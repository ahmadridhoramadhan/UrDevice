//! Notification mirroring (#26): the desktop side.
//!
//! The notifications of a phone are a panel at the bottom of the device page
//! (`NotifyPanel` in `ui/features/notify.slint`, global `NotifyFeature`) with
//! an Active switch, not a tile. It is on from the start and shows every
//! notification of the phone (R18.2). The core service
//! (`urdevice_core::services::notify`) does the protocol and keeps the switch in
//! the device settings; this file draws what a host sends. See `docs/dev/adding-a-feature.md`.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
use urdevice_core::identity::DeviceId;
use urdevice_core::node::{PeerStatus, Snapshot};
use urdevice_core::services::notify::{Notify as NotifyService, NotifyEvents};
use urdevice_core::{Capability, Mode, Node};

use super::{Feature, FeatureCtx};
use crate::ui::{MainWindow, NotifyFeature, NotifyRow, Page};

pub(crate) struct Notify;

/// How often the panel follows the device page, so it shows as soon as the page opens.
const TICK: Duration = Duration::from_millis(500);

/// The message kinds `ui/features/notify.slint` knows.
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

/// Everything the UI thread keeps for this panel.
#[derive(Default)]
struct Ui {
    node: Option<Arc<Node>>,
    svc: Option<Arc<NotifyService>>,
    /// Hosts this computer asked for notifications on the current connection.
    asked: HashSet<String>,
    /// The host refused the last request, until the next state arrives.
    refused: bool,
    timer: Timer,
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui::default());
}

/// Tells the panel to redraw when the core has news. Called on core threads.
struct Events {
    window: slint::Weak<MainWindow>,
}

impl NotifyEvents for Events {
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

impl Feature for Notify {
    fn id(&self) -> &'static str {
        "notify"
    }

    fn start(&self, ctx: &FeatureCtx) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        let Some(svc) = ctx.node.service::<NotifyService>() else {
            return;
        };
        svc.set_events(Arc::new(Events {
            window: ctx.window.clone(),
        }));
        UI.with(|u| {
            let mut u = u.borrow_mut();
            u.node = Some(ctx.node.clone());
            u.svc = Some(svc);
            let weak = ctx.window.clone();
            u.timer.start(TimerMode::Repeated, TICK, move || {
                if let Some(w) = weak.upgrade()
                    && let Some(node) = UI.with(|u| u.borrow().node.clone())
                {
                    follow(&w, &node.snapshot());
                }
            });
        });
        wire(&w);
    }

    fn refresh(&self, ctx: &FeatureCtx, snap: &Snapshot) {
        if let Some(w) = ctx.window.upgrade() {
            follow(&w, snap);
        }
    }
}

fn service() -> Option<Arc<NotifyService>> {
    UI.with(|u| u.borrow().svc.clone())
}

fn peer_of(w: &MainWindow) -> Option<DeviceId> {
    DeviceId::from_hex(&w.global::<NotifyFeature>().get_peer_id())
}

/// Follow the device page: show the panel of the device that is open.
fn follow(w: &MainWindow, snap: &Snapshot) {
    let g = w.global::<NotifyFeature>();
    let open = (w.get_page() == Page::Peer).then(|| w.get_selected_peer().to_string());
    let peer = open
        .as_deref()
        .and_then(|id| snap.peers.iter().find(|p| p.id == id));
    let listed = peer.is_some_and(|p| {
        p.status == PeerStatus::Connected
            && p.remote_permissions
                .iter()
                .any(|r| r.capability == Capability::Notify)
    });
    g.set_available(listed);
    // Like media: showing a phone that mirrors starts the notifications, with no extra step.
    if let Some(p) = peer {
        let id = DeviceId::from_hex(&p.id);
        let denied = p
            .remote_permissions
            .iter()
            .any(|r| r.capability == Capability::Notify && r.mode == Mode::Deny);
        let ask = match (listed && !denied, id, service()) {
            (true, Some(id), Some(svc)) if svc.is_on(id) => UI
                .with(|u| u.borrow_mut().asked.insert(p.id.clone()))
                .then_some((svc, id)),
            _ => {
                UI.with(|u| u.borrow_mut().asked.remove(&p.id));
                None
            }
        };
        if let Some((svc, id)) = ask {
            let _ = svc.subscribe(id, true);
        }
    }
    if let Some(p) = peer
        && g.get_peer_id() != p.id.as_str()
    {
        g.set_peer_id(p.id.clone().into());
        UI.with(|u| u.borrow_mut().refused = false);
    }
    sync(w);
}

/// Hook the Slint callbacks to the core service.
fn wire(w: &MainWindow) {
    let g = w.global::<NotifyFeature>();
    let weak = w.as_weak();

    g.on_clear({
        let weak = weak.clone();
        move || {
            let Some(w) = weak.upgrade() else { return };
            let (Some(id), Some(svc)) = (peer_of(&w), service()) else {
                return;
            };
            svc.clear(id);
            sync(&w);
        }
    });

    g.on_set_on(move |on| {
        let Some(w) = weak.upgrade() else { return };
        let (Some(id), Some(svc)) = (peer_of(&w), service()) else {
            return;
        };
        UI.with(|u| u.borrow_mut().refused = false);
        let _ = svc.subscribe(id, on);
        sync(&w);
    });
}

/// Redraw the panel from the core's current state.
fn sync(w: &MainWindow) {
    let g = w.global::<NotifyFeature>();
    let (Some(id), Some(svc), Some(node)) = UI.with(|u| {
        let u = u.borrow();
        (peer_of(w), u.svc.clone(), u.node.clone())
    }) else {
        return;
    };
    let hex = id.hex();
    let on = svc.is_on(id);
    g.set_on(on);
    let peer = node.snapshot().peers.into_iter().find(|p| p.id == hex);
    let connected = peer
        .as_ref()
        .is_some_and(|p| p.status == PeerStatus::Connected);
    let denied = peer
        .as_ref()
        .and_then(|p| {
            p.remote_permissions
                .iter()
                .find(|r| r.capability == Capability::Notify)
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
            Some(st) if !st.unavailable.is_empty() => kind::UNSUPPORTED,
            Some(st) if st.items.is_empty() => kind::EMPTY,
            Some(_) => kind::NONE,
        }
    };
    g.set_message_kind(message);

    let rows: Vec<NotifyRow> = state
        .iter()
        .flat_map(|st| st.items.iter())
        .map(|i| NotifyRow {
            app: i.app_name.as_str().into(),
            title: i.title.as_str().into(),
            text: i.text.as_str().into(),
            when: clock(i.posted_ms).into(),
        })
        .collect();
    g.set_items(ModelRc::from(Rc::new(VecModel::from(rows))));
}

/// When a notification was posted, in local time.
fn clock(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_a_clock() {
        assert_eq!(clock(u64::MAX), "");
        assert_eq!(clock(0).len(), 5);
    }
}
