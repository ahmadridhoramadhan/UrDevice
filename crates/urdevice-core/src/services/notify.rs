//! Notification mirroring (#26, design §18.2, R18).
//!
//! Service id: [`ServiceId::NOTIFY`], capability `notify.v1` (Release 2,
//! default Always Ask, not watched). Only Android hosts offer it for now
//! (`Capability::android_only`); desktop → Android mirroring is not built.
//!
//! **Host side.** A receiver sends `Subscribe { on: true, apps }`. While at
//! least one receiver is subscribed, the service reads the [`NotifyHost`]
//! every [`POLL_EVERY`] and pushes a [`NotifyState`] to each subscriber when
//! it changed. Only the notifications of the apps the receiver picked leave
//! the host (R18.2). A notification that is dismissed on the host is missing
//! from the next state, so dismissal reaches the receiver (R18.5). `on:
//! false`, a closed session, a change of the setting to Deny, and the end of
//! an Always Ask activity all stop the pushes (R5.5). The host adapter hides
//! secret and private content before it hands a notification over (R18.3).
//!
//! **Receiver side.** [`Notify::subscribe`], [`Notify::set_apps`], and the
//! getters are called by the UI. Whether mirroring is on for a host and which
//! apps are picked are kept in the device settings, so they survive a restart;
//! mirroring is off until the user turns it on (R18.2). A pushed state is
//! accepted only from a host this device subscribed to, is sanitized, and is
//! shown, never acted on. Notification content is kept in memory only, and
//! is dropped when the host dismisses it, when mirroring is turned off, and
//! when the session ends (R18.6).

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

/// Settings are at most 256 bytes, so a list of app ids is stored in chunks.
const CHUNK: usize = 240;

/// Most chunks of picked apps.
const CHUNKS: usize = 16;

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
    /// The apps the receiver picked.
    apps: HashSet<String>,
    /// What this receiver last got.
    last: Option<NotifyState>,
}

/// What the receiver side remembers.
#[derive(Default)]
struct Receiver {
    /// Hosts whose saved settings were read.
    loaded: HashSet<DeviceId>,
    /// Hosts the user turned mirroring on for.
    wanted: HashSet<DeviceId>,
    /// The apps picked for each host.
    apps: HashMap<DeviceId, Vec<String>>,
    /// Apps each host has shown a notification for, with their names.
    catalog: HashMap<DeviceId, Vec<NotifyApp>>,
    /// The newest state from each host.
    latest: HashMap<DeviceId, NotifyState>,
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

    /// Pick the apps to mirror from `host` (R18.2). Ids that are empty, too
    /// long, or hold a comma are ignored.
    pub fn set_apps(&self, host: DeviceId, apps: &[String]) -> Result<()> {
        let ctx = self.ctx()?;
        self.load(ctx, host);
        let mut seen = HashSet::new();
        let apps: Vec<String> = apps
            .iter()
            .filter(|a| valid_app_id(a) && seen.insert(a.as_str()))
            .take(MAX_APPS)
            .cloned()
            .collect();
        let on = {
            let mut r = lock(&self.shared.recv);
            r.apps.insert(host, apps.clone());
            r.wanted.contains(&host)
        };
        save_apps(ctx, host, &apps)?;
        if on {
            self.send_subscribe(ctx, host, true)?;
        }
        Ok(())
    }

    /// The apps picked for `host`.
    pub fn picked_apps(&self, host: DeviceId) -> Vec<String> {
        if let Ok(ctx) = self.ctx() {
            self.load(ctx, host);
        }
        lock(&self.shared.recv)
            .apps
            .get(&host)
            .cloned()
            .unwrap_or_default()
    }

    /// The apps `host` has shown a notification for since this app started.
    pub fn known_apps(&self, host: DeviceId) -> Vec<NotifyApp> {
        lock(&self.shared.recv)
            .catalog
            .get(&host)
            .cloned()
            .unwrap_or_default()
    }

    /// The newest state from `host`. `None` while mirroring is off or before
    /// the first state.
    pub fn state_of(&self, host: DeviceId) -> Option<NotifyState> {
        lock(&self.shared.recv).latest.get(&host).cloned()
    }

    fn send_subscribe(&self, ctx: &ServiceCtx, host: DeviceId, on: bool) -> Result<()> {
        if !ctx.peer_offers(host) {
            return Err(CoreError::Refused("unsupported".into()));
        }
        let apps = lock(&self.shared.recv)
            .apps
            .get(&host)
            .cloned()
            .unwrap_or_default();
        let m = WireNotify::from(Msg::Subscribe(wire::Subscribe { on, apps }));
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
        if ctx.setting(&on_key(host)).as_deref() == Some("1") {
            r.wanted.insert(host);
        }
        r.apps.insert(host, load_apps(ctx, host));
    }
}

fn on_key(host: DeviceId) -> String {
    format!("on.{}", host.hex())
}

fn apps_key(host: DeviceId, i: usize) -> String {
    format!("apps.{}.{i}", host.hex())
}

/// An app id this device accepts: not empty, not too long, and without
/// anything that would break the saved list.
fn valid_app_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && !id.contains(',') && !id.chars().any(char::is_control)
}

/// Save the picked apps in chunks of at most [`CHUNK`] bytes.
fn save_apps(ctx: &ServiceCtx, host: DeviceId, apps: &[String]) -> Result<()> {
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for a in apps.iter().filter(|a| a.len() <= CHUNK) {
        if !cur.is_empty() && cur.len() + 1 + a.len() > CHUNK {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(',');
        }
        cur.push_str(a);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    for i in 0..CHUNKS {
        let v = chunks.get(i).map_or("", String::as_str);
        ctx.set_setting(&apps_key(host, i), v)?;
    }
    Ok(())
}

fn load_apps(ctx: &ServiceCtx, host: DeviceId) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..CHUNKS {
        let Some(v) = ctx.setting(&apps_key(host, i)).filter(|v| !v.is_empty()) else {
            break;
        };
        out.extend(v.split(',').filter(|a| valid_app_id(a)).map(str::to_owned));
    }
    out.truncate(MAX_APPS);
    out
}

/// `st` with only the notifications of `apps`.
fn filtered(st: &NotifyState, apps: &HashSet<String>) -> NotifyState {
    let mut out = st.clone();
    out.items.retain(|i| apps.contains(&i.app_id));
    out
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
    let shown = filtered(st, &sub.apps);
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
                .is_some_and(|s| s.last.as_ref() != Some(&filtered(&st, &s.apps)));
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
            // The user left mirroring on: ask again on the new session.
            self.load(ctx, peer);
            let wanted = lock(&self.shared.recv).wanted.contains(&peer);
            if wanted && ctx.peer_offers(peer) {
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
        let fresh = lock(&self.shared.subs)
            .insert(
                peer,
                Subscriber {
                    activity: grant.activity(),
                    apps: s.apps.into_iter().collect(),
                    last: None,
                },
            )
            .is_none();
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
            let catalog = r.catalog.entry(peer).or_default();
            for a in &st.apps {
                if let Some(c) = catalog.iter_mut().find(|c| c.id == a.id) {
                    c.name.clone_from(&a.name);
                } else if catalog.len() < MAX_APPS {
                    catalog.push(a.clone());
                }
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
    fn only_picked_apps_leave_the_host() {
        let st = NotifyState {
            items: vec![item("1", "chat"), item("2", "mail"), item("3", "chat")],
            apps: vec![
                NotifyApp {
                    id: "chat".into(),
                    name: "Chat".into(),
                },
                NotifyApp {
                    id: "mail".into(),
                    name: "Mail".into(),
                },
            ],
            unavailable: String::new(),
        };
        let picked: HashSet<String> = ["chat".to_owned()].into();
        let out = filtered(&st, &picked);
        assert_eq!(out.items.len(), 2);
        assert!(out.items.iter().all(|i| i.app_id == "chat"));
        // The picker still lists every app.
        assert_eq!(out.apps.len(), 2);
        // Nothing is picked: nothing leaves.
        assert!(filtered(&st, &HashSet::new()).items.is_empty());
    }

    #[test]
    fn app_ids_for_the_saved_list_are_checked() {
        assert!(valid_app_id("com.chat"));
        assert!(!valid_app_id(""));
        assert!(!valid_app_id("a,b"));
        assert!(!valid_app_id("a\nb"));
        assert!(!valid_app_id(&"x".repeat(MAX_ID + 1)));
    }
}
