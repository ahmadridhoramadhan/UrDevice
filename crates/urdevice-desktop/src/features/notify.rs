//! Notification mirroring (#26): the desktop side.
//!
//! The notifications of a phone are a panel at the bottom of the device page
//! (`NotifyPanel` in `ui/features/notify.slint`, global `NotifyFeature`) with
//! an Active switch, not a tile. The core service
//! (`urdevice_core::services::notify`) does the protocol and keeps the
//! choices (switch and apps) in the device settings; this file draws what a
//! host sends. See `docs/dev/adding-a-feature.md`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};
use urdevice_core::identity::DeviceId;
use urdevice_core::node::{PeerStatus, Snapshot};
use urdevice_core::services::notify::{Notify as NotifyService, NotifyEvents};
use urdevice_core::{Capability, Mode, Node};

use super::{Feature, FeatureCtx};
use crate::ui::{MainWindow, NotifyAppRow, NotifyFeature, NotifyRow, Page};

pub(crate) struct Notify;

/// The message kinds `ui/features/notify.slint` knows.
mod kind {
    pub const NONE: i32 = 0;
    pub const OFFLINE: i32 = 1;
    pub const DENIED: i32 = 2;
    pub const OFF: i32 = 3;
    pub const WAITING: i32 = 4;
    pub const LISTENER: i32 = 5;
    pub const UNSUPPORTED: i32 = 6;
    pub const NO_APPS: i32 = 7;
    pub const EMPTY: i32 = 8;
    pub const REFUSED: i32 = 9;
}

/// Everything the UI thread keeps for this panel.
#[derive(Default)]
struct Ui {
    node: Option<Arc<Node>>,
    svc: Option<Arc<NotifyService>>,
    /// The ids of the apps in the list, in the order shown.
    app_ids: Vec<String>,
    /// The host refused the last request, until the next state arrives.
    refused: bool,
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

    g.on_set_on({
        let weak = weak.clone();
        move |on| {
            let Some(w) = weak.upgrade() else { return };
            let (Some(id), Some(svc)) = (peer_of(&w), service()) else {
                return;
            };
            UI.with(|u| u.borrow_mut().refused = false);
            let _ = svc.subscribe(id, on);
            sync(&w);
        }
    });

    g.on_pick_app(move |i, picked| {
        let Some(w) = weak.upgrade() else { return };
        let (Some(id), Some(svc)) = (peer_of(&w), service()) else {
            return;
        };
        let Some(app) = usize::try_from(i)
            .ok()
            .and_then(|i| UI.with(|u| u.borrow().app_ids.get(i).cloned()))
        else {
            return;
        };
        let mut apps = svc.picked_apps(id);
        apps.retain(|a| *a != app);
        if picked {
            apps.push(app);
        }
        let _ = svc.set_apps(id, &apps);
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
    let picked = svc.picked_apps(id);
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
            _ if picked.is_empty() => kind::NO_APPS,
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

    // The apps to choose from: the ones the host showed, then the picked ones it has not
    // shown lately (named by their id).
    let mut apps = svc.known_apps(id);
    for a in &picked {
        if !apps.iter().any(|k| k.id == *a) {
            apps.push(urdevice_core::services::notify::NotifyApp {
                id: a.clone(),
                name: a.clone(),
            });
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    let app_rows: Vec<NotifyAppRow> = apps
        .iter()
        .map(|a| NotifyAppRow {
            name: a.name.as_str().into(),
            picked: picked.contains(&a.id),
        })
        .collect();
    UI.with(|u| u.borrow_mut().app_ids = apps.into_iter().map(|a| a.id).collect());
    g.set_apps(ModelRc::from(Rc::new(VecModel::from(app_rows))));
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
