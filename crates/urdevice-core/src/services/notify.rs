//! Notification mirroring (#26, design §18.2, R18).
//!
//! Service id: [`ServiceId::NOTIFY`], capability `notify.v1` (Release 2,
//! default Always Ask, not watched). Only Android hosts offer it for now
//! (`Capability::android_only`); desktop → Android mirroring is not built.
//!
//! **Host side.** A receiver sends `Subscribe { on: true, apps }`. While at
//! least one receiver is subscribed, the service reads the [`NotifyHost`]
//! every [`POLL_EVERY`] and pushes a [`NotifyState`] to each subscriber when
//! it changed. A receiver sends no app list, which means every app; a list, if
//! one is sent, limits what leaves the host (R18.2). Only notifications posted
//! after the receiver subscribed leave the host, so turning mirroring on does
//! not bring in what piled up before. A notification that is dismissed on the host is missing
//! from the next state, so dismissal reaches the receiver (R18.5). `on:
//! false`, a closed session, a change of the setting to Deny, and the end of
//! an Always Ask activity all stop the pushes (R5.5). The host adapter hides
//! secret and private content before it hands a notification over (R18.3).
//!
//! **Receiver side.** [`Notify::subscribe`] and the getters are called by the
//! UI. Whether mirroring is on for a host is kept in the device settings, so it
//! survives a restart; it is on until the user turns it off (R18.2). A pushed state is
//! accepted only from a host this device subscribed to, is sanitized, and is
//! shown, never acted on. Notification content is kept in memory only, and
//! is dropped when the host dismisses it, when mirroring is turned off, and
//! when the session ends (R18.6). [`Notify::clear`] hides what is shown now on
//! this device; a notification posted again later shows up again.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use tokio::sync::Notify as Wake;
use urdevice_proto::features::notify::{self as wire, Notify as WireNotify, message::Msg};

use super::{Access, Direction, Grant, Message, Registry, Service, ServiceCtx, ServiceId, Slot};
use crate::identity::DeviceId;
use crate::permission::{Capability, Mode};
use crate::{CoreError, Result};

pub use urdevice_proto::features::notify::{
    App as NotifyApp, Item as NotifyItem, MAX_APPS, MAX_ID, MAX_ITEMS, State as NotifyState,
};

/// Release 2 capabilities this module ships.
pub(crate) const SHIPPED: &[Capability] = &[Capability::Notify];

/// How often the host reads its notifications while someone is subscribed.
pub const POLL_EVERY: Duration = Duration::from_secs(2);

/// What the host OS provides: the notifications that are shown now.
/// Android: Kotlin through the FFI.
///
/// The method may block; the service calls it from the blocking thread pool.
pub trait NotifyHost: Send + Sync {
    /// Every notification on this device now, newest first, with secret and
    /// private content already hidden (R18.3).
    fn state(&self) -> NotifyState;
}

/// What the UI listens to, on the receiver side. Called from core threads;
/// hand work to the UI thread.
pub trait NotifyEvents: Send + Sync {
    /// A new [`NotifyState`] arrived from `peer`, or it was cleared (read it
    /// with [`Notify::state_of`]).
    fn updated(&self, peer: DeviceId);

    /// `peer` refused the last request: `denied`, `declined`, `timeout`,
    /// `unsupported`, or `offline`.
    fn refused(&self, peer: DeviceId, reason: &str);
}

/// A subscriber on the host side.
struct Subscriber {
    /// The activity the subscription was granted under, or 0 for Allow.
    activity: u64,
    /// The apps the receiver picked. Empty means every app.
    apps: HashSet<String>,
    /// Notifications posted before this time (host clock, ms) are not sent.
    since_ms: u64,
    /// What this receiver last got.
    last: Option<NotifyState>,
}

/// What the receiver side remembers.
#[derive(Default)]
struct Receiver {
    /// Hosts whose saved settings were read.
    loaded: HashSet<DeviceId>,
    /// Hosts mirroring is on for: all of them, except the ones the user turned it off for.
    wanted: HashSet<DeviceId>,
    /// The newest state from each host.
    latest: HashMap<DeviceId, NotifyState>,
    /// Notifications the user cleared: key and the time it was posted. One that is posted
    /// again (a new message in the same chat) has a later time and shows again.
    cleared: HashMap<DeviceId, HashMap<String, u64>>,
    /// Always Ask activity per host, once approved.
    activity: HashMap<DeviceId, u64>,
    /// The last message a host refused with `ask` or `no_activity`.
    pending: HashMap<DeviceId, WireNotify>,
    /// Hosts an activity request is running for.
    requesting: HashSet<DeviceId>,
}

struct Shared {
    ctx: OnceLock<ServiceCtx>,
    host: Slot<dyn NotifyHost>,
    events: Slot<dyn NotifyEvents>,
    subs: Mutex<HashMap<DeviceId, Subscriber>>,
    recv: Mutex<Receiver>,
    /// Wakes the poll loop early: a new subscriber.
    wake: Wake,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            ctx: OnceLock::new(),
            host: Slot::default(),
            events: Slot::default(),
            subs: Mutex::default(),
            recv: Mutex::default(),
            wake: Wake::new(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The notification service.
#[derive(Default)]
pub struct Notify {
    shared: Arc<Shared>,
}

impl Notify {
    /// Give the service this device's notifications.
    pub fn set_host(&self, host: Arc<dyn NotifyHost>) {
        self.shared.host.set(host);
    }

    /// Set the UI listener.
    pub fn set_events(&self, events: Arc<dyn NotifyEvents>) {
        self.shared.events.set(events);
    }

    fn ctx(&self) -> Result<&ServiceCtx> {
        self.shared.ctx.get().ok_or(CoreError::Stopped)
    }

    // ----- receiver side, called by the UI -----

    /// Turn mirroring from `host` on or off (R18.2). On: the host replies with
    /// its notifications. Off: the host stops sending, and what this device
    /// holds is dropped.
    pub fn subscribe(&self, host: DeviceId, on: bool) -> Result<()> {
        let ctx = self.ctx()?;
        self.load(ctx, host);
        {
            let mut r = lock(&self.shared.recv);
            if on {
                r.wanted.insert(host);
            } else {
                r.wanted.remove(&host);
                r.latest.remove(&host);
                r.cleared.remove(&host);
            }
        }
        ctx.set_setting(&on_key(host), if on { "1" } else { "0" })?;
        if !on && let Some(e) = self.shared.events.get() {
            e.updated(host);
        }
        self.send_subscribe(ctx, host, on)
    }

    /// True while mirroring from `host` is on.
    pub fn is_on(&self, host: DeviceId) -> bool {
        if let Ok(ctx) = self.ctx() {
            self.load(ctx, host);
        }
        lock(&self.shared.recv).wanted.contains(&host)
    }

    /// The newest state from `host`. `None` while mirroring is off or before
    /// the first state.
    pub fn state_of(&self, host: DeviceId) -> Option<NotifyState> {
        let r = lock(&self.shared.recv);
        let mut st = r.latest.get(&host)?.clone();
        if let Some(cleared) = r.cleared.get(&host) {
            st.items
                .retain(|i| cleared.get(&i.key).is_none_or(|at| i.posted_ms > *at));
        }
        Some(st)
    }

    /// Clear the notifications shown for `host`. This only hides them here; the
    /// phone keeps its own.
    pub fn clear(&self, host: DeviceId) {
        {
            let mut r = lock(&self.shared.recv);
            let Some(st) = r.latest.get(&host) else {
                return;
            };
            let all: HashMap<String, u64> = st
                .items
                .iter()
                .map(|i| (i.key.clone(), i.posted_ms))
                .collect();
            r.cleared.insert(host, all);
        }
        if let Some(e) = self.shared.events.get() {
            e.updated(host);
        }
    }

    fn send_subscribe(&self, ctx: &ServiceCtx, host: DeviceId, on: bool) -> Result<()> {
        if !ctx.peer_offers(host) {
            return Err(CoreError::Refused("unsupported".into()));
        }
        // No app list: every app (R18.2).
        let m = WireNotify::from(Msg::Subscribe(wire::Subscribe {
            on,
            apps: Vec::new(),
        }));
        let activity = {
            let mut r = lock(&self.shared.recv);
            r.pending.insert(host, m.clone());
            r.activity.get(&host).copied().unwrap_or(0)
        };
        ctx.send(host, Direction::ToHost, activity, &m)
    }

    /// Read the saved settings for `host` once.
    fn load(&self, ctx: &ServiceCtx, host: DeviceId) {
        let mut r = lock(&self.shared.recv);
        if !r.loaded.insert(host) {
            return;
        }
        // On until the user turns it off for this host (R18.2).
        if ctx.setting(&on_key(host)).as_deref() != Some("0") {
            r.wanted.insert(host);
        }
    }
}

fn on_key(host: DeviceId) -> String {
    format!("on.{}", host.hex())
}

/// `st` with only the notifications posted at or after `since_ms`, and of `apps` (an empty
/// set means every app).
fn filtered(st: &NotifyState, apps: &HashSet<String>, since_ms: u64) -> NotifyState {
    let mut out = st.clone();
    out.items
        .retain(|i| i.posted_ms >= since_ms && (apps.is_empty() || apps.contains(&i.app_id)));
    out
}

/// Now, in milliseconds since the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Read the host's state off the async runtime, with the limits applied.
async fn read_host(shared: &Arc<Shared>) -> NotifyState {
    let unsupported = || NotifyState {
        unavailable: "unsupported".into(),
        ..NotifyState::default()
    };
    let Some(host) = shared.host.get() else {
        return unsupported();
    };
    let mut st = tokio::task::spawn_blocking(move || host.state())
        .await
        .unwrap_or_else(|_| unsupported());
    st.items.sort_by_key(|i| std::cmp::Reverse(i.posted_ms));
    st.sanitize();
    st
}

/// Send `st` (what `peer` is allowed to see) and remember it.
fn push(shared: &Shared, ctx: &ServiceCtx, peer: DeviceId, st: &NotifyState) {
    let mut subs = lock(&shared.subs);
    let Some(sub) = subs.get_mut(&peer) else {
        return;
    };
    let shown = filtered(st, &sub.apps, sub.since_ms);
    let m = WireNotify::from(Msg::State(shown.clone()));
    if ctx
        .send(peer, Direction::ToController, sub.activity, &m)
        .is_ok()
    {
        sub.last = Some(shown);
    }
}

/// Drop subscribers the host's setting no longer allows.
fn enforce_mode(shared: &Shared, ctx: &ServiceCtx) {
    let mut subs = lock(&shared.subs);
    match ctx.local_mode() {
        Mode::Deny => subs.clear(),
        Mode::Ask => subs.retain(|_, s| s.activity != 0),
        Mode::Allow => {}
    }
}

async fn poll_loop(shared: Arc<Shared>, ctx: ServiceCtx) {
    loop {
        tokio::select! {
            () = tokio::time::sleep(POLL_EVERY) => {}
            () = shared.wake.notified() => {}
        }
        enforce_mode(&shared, &ctx);
        if lock(&shared.subs).is_empty() {
            continue;
        }
        let st = read_host(&shared).await;
        let peers: Vec<DeviceId> = lock(&shared.subs).keys().copied().collect();
        for peer in peers {
            let send = lock(&shared.subs)
                .get(&peer)
                .is_some_and(|s| s.last.as_ref() != Some(&filtered(&st, &s.apps, s.since_ms)));
            if send {
                push(&shared, &ctx, peer, &st);
            }
        }
    }
}

impl Service for Notify {
    fn id(&self) -> ServiceId {
        ServiceId::NOTIFY
    }

    fn capability(&self) -> Capability {
        Capability::Notify
    }

    fn start(&self, ctx: ServiceCtx) {
        if self.shared.ctx.set(ctx.clone()).is_ok() {
            ctx.spawn(poll_loop(self.shared.clone(), ctx.clone()));
        }
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> Result<()> {
        // Untrusted: a body that does not decode closes the session.
        let m: WireNotify = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad notify message".into()))?;
        let Some(inner) = m.msg else {
            return Ok(());
        };
        match (msg.access, inner) {
            (Access::Host(grant), Msg::Subscribe(s)) => self.host_subscribe(ctx, &grant, s),
            (Access::Controller { .. }, Msg::State(st)) => self.on_state(msg.peer, st),
            // A state sent to the host: never act on it.
            _ => {}
        }
        Ok(())
    }

    fn on_refused(&self, ctx: &ServiceCtx, peer: DeviceId, _activity: u64, reason: &str) {
        let retry = matches!(reason, "ask" | "no_activity");
        let pending = {
            let mut r = lock(&self.shared.recv);
            if retry {
                r.activity.remove(&peer);
            }
            if retry && !r.requesting.contains(&peer) {
                r.pending.remove(&peer)
            } else {
                None
            }
        };
        let (Some(m), true) = (pending, retry) else {
            self.refused(peer, reason);
            return;
        };
        let shared = self.shared.clone();
        lock(&shared.recv).requesting.insert(peer);
        let task_ctx = ctx.clone();
        ctx.spawn(async move {
            // One approval, then the message goes again. A second refusal has
            // nothing pending, so it is reported, not retried.
            let r = task_ctx.request(peer).await;
            lock(&shared.recv).requesting.remove(&peer);
            match r {
                Ok(activity) => {
                    lock(&shared.recv).activity.insert(peer, activity);
                    let _ = task_ctx.send(peer, Direction::ToHost, activity, &m);
                }
                Err(CoreError::Refused(why)) => notify_refused(&shared, peer, &why),
                Err(_) => notify_refused(&shared, peer, "offline"),
            }
        });
    }

    fn on_activity_end(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64) {
        {
            let mut subs = lock(&self.shared.subs);
            if subs.get(&peer).is_some_and(|s| s.activity == activity) {
                subs.remove(&peer);
            }
        }
        let mut r = lock(&self.shared.recv);
        if r.activity.get(&peer) == Some(&activity) {
            r.activity.remove(&peer);
        }
    }

    fn on_peer(&self, ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        if connected {
            // Mirroring the user has already used (the setting is saved) starts again on the
            // new session. Until then nothing is asked: the host would otherwise prompt for
            // notifications as soon as it connects, before anyone looked at them.
            self.load(ctx, peer);
            let used = ctx.setting(&on_key(peer)).as_deref() == Some("1");
            let wanted = lock(&self.shared.recv).wanted.contains(&peer);
            if used && wanted && ctx.peer_offers(peer) {
                let _ = self.send_subscribe(ctx, peer, true);
            }
            return;
        }
        lock(&self.shared.subs).remove(&peer);
        let had = {
            let mut r = lock(&self.shared.recv);
            r.activity.remove(&peer);
            r.pending.remove(&peer);
            r.requesting.remove(&peer);
            r.cleared.remove(&peer);
            r.latest.remove(&peer).is_some()
        };
        if had && let Some(e) = self.shared.events.get() {
            e.updated(peer);
        }
    }
}

fn notify_refused(shared: &Shared, peer: DeviceId, reason: &str) {
    if let Some(e) = shared.events.get() {
        e.refused(peer, reason);
    }
}

impl Notify {
    fn refused(&self, peer: DeviceId, reason: &str) {
        if reason == "denied" || reason == "unsupported" {
            lock(&self.shared.recv).latest.remove(&peer);
        }
        notify_refused(&self.shared, peer, reason);
        if let Some(e) = self.shared.events.get() {
            e.updated(peer);
        }
    }

    fn host_subscribe(&self, ctx: &ServiceCtx, grant: &Grant, mut s: wire::Subscribe) {
        let peer = grant.peer();
        if !s.on {
            lock(&self.shared.subs).remove(&peer);
            ctx.log(peer, "notifications off");
            return;
        }
        s.sanitize();
        // A new receiver starts from now; one that asks again keeps its start.
        let mut subs = lock(&self.shared.subs);
        let since_ms = subs.get(&peer).map_or_else(now_ms, |old| old.since_ms);
        let fresh = subs
            .insert(
                peer,
                Subscriber {
                    activity: grant.activity(),
                    apps: s.apps.into_iter().collect(),
                    since_ms,
                    last: None,
                },
            )
            .is_none();
        drop(subs);
        if fresh {
            ctx.log(peer, "notifications on");
        }
        // The current state goes out now, not after the next tick.
        let shared = self.shared.clone();
        let task_ctx = ctx.clone();
        ctx.spawn(async move {
            let st = read_host(&shared).await;
            push(&shared, &task_ctx, peer, &st);
        });
    }

    fn on_state(&self, peer: DeviceId, mut st: NotifyState) {
        {
            let mut r = lock(&self.shared.recv);
            // Only a host this device turned mirroring on for may push.
            if !r.wanted.contains(&peer) {
                return;
            }
            st.sanitize();
            // A cleared notification that is gone from the host is forgotten.
            if let Some(cleared) = r.cleared.get_mut(&peer) {
                cleared.retain(|k, _| st.items.iter().any(|i| i.key == *k));
            }
            r.latest.insert(peer, st);
        }
        if let Some(e) = self.shared.events.get() {
            e.updated(peer);
        }
    }
}

/// Add this feature's services to a starting node. Every platform registers
/// the service so it can ask a host; only Android hosts offer `notify.v1`
/// (`Capability::android_only`). A host without a [`NotifyHost`] answers with
/// an empty state.
pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Arc::new(Notify::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str, app: &str) -> NotifyItem {
        NotifyItem {
            key: key.into(),
            app_id: app.into(),
            ..NotifyItem::default()
        }
    }

    #[test]
    fn no_app_list_means_every_app_and_a_list_limits_it() {
        let st = NotifyState {
            items: vec![item("1", "chat"), item("2", "mail"), item("3", "chat")],
            ..NotifyState::default()
        };
        assert_eq!(filtered(&st, &HashSet::new(), 0).items.len(), 3);
        let picked: HashSet<String> = ["chat".to_owned()].into();
        let out = filtered(&st, &picked, 0);
        assert_eq!(out.items.len(), 2);
        assert!(out.items.iter().all(|i| i.app_id == "chat"));
    }

    #[test]
    fn what_was_posted_before_subscribing_is_not_sent() {
        let mut old = item("1", "chat");
        old.posted_ms = 100;
        let mut new = item("2", "chat");
        new.posted_ms = 200;
        let st = NotifyState {
            items: vec![old, new],
            ..NotifyState::default()
        };
        let out = filtered(&st, &HashSet::new(), 150);
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].key, "2");
    }
}
