//! The node: one running UrDevice core (design §1).
//!
//! A [`Node`] owns the identity, trust store, QUIC endpoint, discovery, and
//! every session. Its methods are synchronous and return at once; network work
//! runs on the node's own tokio runtime, and results come back as
//! [`NodeEvent`]s plus a fresh [`Snapshot`].

mod net;
mod types;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, oneshot};
use urdevice_proto::{Announce, PROTO_VERSION, control};

pub use types::*;

use crate::discovery::{DISCOVERY_PORT, Discovery, DiscoveryConfig, Seen, Via};
use crate::identity::{DeviceId, Identity};
use crate::pairing::{self, Lockout, TOKEN_LEN, TOKEN_TTL};
use crate::permission::{Capability, Mode};
use crate::secret::{self, SecretStore};
use crate::store::{PairedDevice, TrustStore};
use crate::transport::{self, TrustPolicy, close};
use crate::{CoreError, Result};

/// Default QUIC port (design §17.1).
pub const QUIC_PORT: u16 = 47811;
/// How long an Always Ask prompt waits (R5.3).
pub const ASK_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest reconnect delay (R4.5).
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// A nearby device that has not announced for this long is dropped from the list.
pub const NEARBY_TTL: Duration = Duration::from_secs(20);
/// At most one battery update per this interval (R4.9).
pub const BATTERY_INTERVAL: Duration = Duration::from_secs(60);
/// How long the list of this device's addresses is reused.
const ADDR_CACHE_TTL: Duration = Duration::from_secs(5);

/// Settings for [`Node::start`].
pub struct NodeConfig {
    /// App data directory. The trust store goes here.
    pub data_dir: PathBuf,
    /// Platform string: `linux`, `windows`, `macos`, or `android`.
    pub platform: String,
    /// Name used until the user picks one, for example the host name.
    pub default_name: String,
    /// Secure storage for the identity key.
    pub secrets: Arc<dyn SecretStore>,
    /// Where events go.
    pub events: Arc<dyn EventSink>,
    /// QUIC port; 0 picks a free port.
    pub quic_port: u16,
    /// Discovery settings.
    pub discovery: DiscoveryConfig,
    /// Address the QUIC endpoint binds to.
    pub bind_ip: IpAddr,
    /// Addresses to put in QR codes instead of the interface addresses (tests).
    pub invite_addrs: Option<Vec<IpAddr>>,
    /// Always Ask timeout.
    pub ask_timeout: Duration,
}

impl NodeConfig {
    /// Defaults for a real device.
    pub fn new(
        data_dir: PathBuf,
        platform: &str,
        default_name: &str,
        secrets: Arc<dyn SecretStore>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            data_dir,
            platform: platform.to_owned(),
            default_name: default_name.to_owned(),
            secrets,
            events,
            quic_port: QUIC_PORT,
            discovery: DiscoveryConfig {
                udp_port: DISCOVERY_PORT,
                mdns: cfg!(feature = "mdns"),
            },
            bind_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            invite_addrs: None,
            ask_timeout: ASK_TIMEOUT,
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding a lock is a bug; keep going with the data as is.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

struct Session {
    conn: quinn::Connection,
    tx: mpsc::UnboundedSender<control::Msg>,
    remote_perms: Vec<PermissionView>,
    battery: Option<(u8, bool)>,
}

struct Activity {
    id: u64,
    peer: DeviceId,
    peer_name: String,
    cap: Capability,
    request_id: u64,
    incoming: bool,
    started_at: u64,
}

struct Nearby {
    name: String,
    platform: String,
    addrs: Vec<SocketAddr>,
    last_seen: Instant,
}

struct PairScreen {
    token: [u8; TOKEN_LEN],
    expires: Instant,
    uri: String,
}

struct CodePairing {
    name: String,
    platform: String,
    code: String,
    confirmed_here: bool,
    incoming: bool,
    cmd: mpsc::UnboundedSender<bool>,
}

struct Backoff {
    next: Instant,
    delay: Duration,
}

struct Prompt {
    reply: oneshot::Sender<bool>,
}

#[derive(Default)]
struct State {
    name: String,
    hidden: bool,
    onboarded: bool,
    paired: Vec<PairedDevice>,
    permissions: HashMap<Capability, Mode>,
    sessions: HashMap<DeviceId, Session>,
    connecting: HashSet<DeviceId>,
    backoff: HashMap<DeviceId, Backoff>,
    violations: HashMap<DeviceId, VecDeque<Instant>>,
    paused: HashMap<DeviceId, Instant>,
    nearby: HashMap<[u8; 8], Nearby>,
    pair_screen: Option<PairScreen>,
    lockout: Lockout,
    code_pairings: HashMap<DeviceId, CodePairing>,
    activities: Vec<Activity>,
    pending_requests: HashMap<(DeviceId, u64), Capability>,
    prompts: HashMap<u64, Prompt>,
    battery: Option<(u8, bool)>,
    battery_sent: Option<(Instant, Option<(u8, bool)>)>,
    next_id: u64,
}

impl State {
    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn mode(&self, cap: Capability) -> Mode {
        // A missing or tampered row reads as Deny.
        self.permissions.get(&cap).copied().unwrap_or(Mode::Deny)
    }

    fn paired(&self, id: DeviceId) -> Option<&PairedDevice> {
        self.paired.iter().find(|d| d.id == id)
    }

    fn peer_name(&self, id: DeviceId) -> String {
        self.paired(id)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "Unknown device".into())
    }

    fn permission_state(&self, platform: &str) -> control::Msg {
        control::Msg::PermissionState(urdevice_proto::PermissionState {
            entries: Capability::for_platform(platform)
                .into_iter()
                .map(|c| urdevice_proto::PermissionEntry {
                    capability: c.as_str().into(),
                    mode: self.mode(c).to_wire() as i32,
                })
                .collect(),
        })
    }

    fn device_status(&self) -> control::Msg {
        control::Msg::DeviceStatus(urdevice_proto::DeviceStatus {
            battery_pct: self.battery.map(|(p, _)| u32::from(p)),
            charging: self.battery.map(|(_, c)| c),
        })
    }
}

pub(crate) struct Inner {
    rt: tokio::runtime::Handle,
    platform: String,
    secrets: Arc<dyn SecretStore>,
    events: Arc<dyn EventSink>,
    identity: RwLock<Arc<Identity>>,
    store: Mutex<TrustStore>,
    policy: Arc<TrustPolicy>,
    endpoint: quinn::Endpoint,
    state: Mutex<State>,
    discovery: Mutex<Option<Discovery>>,
    discovery_cfg: DiscoveryConfig,
    seen_tx: mpsc::Sender<Seen>,
    invite_addrs: Option<Vec<IpAddr>>,
    ask_timeout: Duration,
    addr_cache: Mutex<Option<(Instant, Vec<IpAddr>)>>,
}

/// A running UrDevice core.
pub struct Node {
    inner: Arc<Inner>,
    rt: Option<tokio::runtime::Runtime>,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node").finish_non_exhaustive()
    }
}

impl Node {
    /// Start the core: load or create the identity, open the trust store, bind
    /// the QUIC endpoint, and start discovery.
    pub fn start(cfg: NodeConfig) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("urdevice-core")
            .enable_all()
            .build()?;
        let _guard = rt.enter();

        std::fs::create_dir_all(&cfg.data_dir)?;
        let (identity, created) = Identity::load_or_create(cfg.secrets.as_ref())?;
        tracing::info!(peer = %identity.id().short_hex(), created, "identity loaded");
        let mac_key =
            secret::get_or_create_random(cfg.secrets.as_ref(), secret::STORE_MAC_KEY, 32)?;
        let store = TrustStore::open(&cfg.data_dir.join("trust.sqlite3"), mac_key)?;
        let _ = store.prune_activity(unix_now());

        let mut state = State {
            name: pairing::clean_name(&store.meta("name")?.unwrap_or(cfg.default_name.clone())),
            hidden: store.meta("hidden")?.as_deref() == Some("1"),
            onboarded: store.meta("onboarded")?.as_deref() == Some("1"),
            paired: store.paired_devices()?,
            permissions: store.permissions()?,
            ..State::default()
        };
        state.next_id = crate::crypto::random_u64()? >> 16;

        let policy = Arc::new(TrustPolicy::default());
        policy.set_paired(state.paired.iter().map(|d| d.id));
        let endpoint =
            transport::bind_endpoint(&identity, policy.clone(), cfg.bind_ip, cfg.quic_port)?;
        let (seen_tx, seen_rx) = mpsc::channel(256);

        let inner = Arc::new(Inner {
            rt: rt.handle().clone(),
            platform: pairing::clean_platform(&cfg.platform),
            secrets: cfg.secrets,
            events: cfg.events,
            identity: RwLock::new(Arc::new(identity)),
            store: Mutex::new(store),
            policy,
            endpoint,
            state: Mutex::new(state),
            discovery: Mutex::new(None),
            discovery_cfg: cfg.discovery,
            seen_tx,
            invite_addrs: cfg.invite_addrs,
            ask_timeout: cfg.ask_timeout,
            addr_cache: Mutex::new(None),
        });
        inner.restart_discovery()?;

        tokio::spawn(net::accept_loop(inner.clone()));
        tokio::spawn(net::seen_loop(inner.clone(), seen_rx));
        tokio::spawn(net::connector_loop(inner.clone()));
        tokio::spawn(net::housekeeping_loop(inner.clone()));

        Ok(Self {
            inner,
            rt: Some(rt),
        })
    }

    /// Everything the UI draws.
    pub fn snapshot(&self) -> Snapshot {
        self.inner.snapshot()
    }

    /// The activity log, newest first (R5.6).
    pub fn activity_log(&self, limit: u32) -> Result<Vec<ActivityLogView>> {
        let store = lock(&self.inner.store);
        Ok(store
            .activity_log(limit as usize)?
            .into_iter()
            .map(|e| ActivityLogView {
                device_name: e.device_name,
                capability: e.capability,
                decision: e.decision,
                ts: e.ts,
            })
            .collect())
    }

    /// Save onboarding: the name and the first permission settings (R5.2).
    /// Capabilities not listed get their defaults.
    pub fn complete_onboarding(&self, name: &str, settings: &[PermissionView]) -> Result<()> {
        let mut modes: HashMap<Capability, Mode> = Capability::ALL
            .into_iter()
            .map(|c| (c, c.default_mode()))
            .collect();
        for s in settings {
            // Terminal and file changes stay Deny here: switching them to
            // Allow needs the warning on "This device" (R5.2).
            if !s.capability.local_only() {
                modes.insert(s.capability, s.mode);
            }
        }
        let name = pairing::clean_name(name);
        {
            let store = lock(&self.inner.store);
            for (c, m) in &modes {
                store.set_permission(*c, *m)?;
            }
            store.set_meta("name", &name)?;
            store.set_meta("onboarded", "1")?;
        }
        {
            let mut st = lock(&self.inner.state);
            st.permissions = modes;
            st.name = name.clone();
            st.onboarded = true;
        }
        if let Some(d) = lock(&self.inner.discovery).as_ref() {
            d.set_name(&name);
        }
        self.inner.broadcast_permissions();
        self.inner.changed();
        Ok(())
    }

    /// Change one permission setting (R5.1). For terminal and file changes,
    /// `confirmed` must be true to set Allow: the UI shows the warning and the
    /// extra confirmation first (R5.2). Only the local UI calls this; there is
    /// no remote path to it (R21.2).
    pub fn set_permission(&self, cap: Capability, mode: Mode, confirmed: bool) -> Result<()> {
        if cap.local_only() && mode == Mode::Allow && !confirmed {
            return Err(CoreError::InvalidInput("confirmation required".into()));
        }
        lock(&self.inner.store).set_permission(cap, mode)?;
        lock(&self.inner.state).permissions.insert(cap, mode);
        tracing::info!(cap = %cap, mode = mode.as_str(), "permission changed");
        // Within 1 s of the change (R5.7): send now.
        self.inner.broadcast_permissions();
        if mode == Mode::Deny {
            self.inner.revoke_capability(cap);
        }
        self.inner.changed();
        Ok(())
    }

    /// Rename this device.
    pub fn set_name(&self, name: &str) -> Result<()> {
        let name = pairing::clean_name(name);
        lock(&self.inner.store).set_meta("name", &name)?;
        lock(&self.inner.state).name = name.clone();
        if let Some(d) = lock(&self.inner.discovery).as_ref() {
            d.set_name(&name);
        }
        self.inner.changed();
        Ok(())
    }

    /// Hidden mode (R2.7).
    pub fn set_hidden(&self, hidden: bool) -> Result<()> {
        lock(&self.inner.store).set_meta("hidden", if hidden { "1" } else { "0" })?;
        lock(&self.inner.state).hidden = hidden;
        if let Some(d) = lock(&self.inner.discovery).as_ref() {
            d.set_hidden(hidden);
        }
        self.inner.update_policy();
        self.inner.changed();
        Ok(())
    }

    /// Open the Pair screen: pairing is accepted only while it is open (R3.8).
    pub fn open_pair_screen(&self) {
        {
            let mut st = lock(&self.inner.state);
            if st.pair_screen.is_none() {
                match self.inner.new_pair_screen(&st) {
                    Ok(s) => st.pair_screen = Some(s),
                    Err(e) => tracing::warn!(error = %e, "could not create a pairing token"),
                }
            }
        }
        self.inner.update_policy();
        if let Some(d) = lock(&self.inner.discovery).as_ref() {
            d.announce_now();
        }
        self.inner.changed();
    }

    /// Close the Pair screen. Cancels code pairings that are still open.
    pub fn close_pair_screen(&self) {
        {
            let mut st = lock(&self.inner.state);
            st.pair_screen = None;
            for p in st.code_pairings.values() {
                let _ = p.cmd.send(false);
            }
        }
        self.inner.update_policy();
        self.inner.changed();
    }

    /// Pair by a scanned QR code (R3.3, R3.4). The result arrives as
    /// [`NodeEvent::PairingFinished`].
    pub fn scan_qr(&self, text: &str) -> Result<()> {
        let invite = pairing::parse_invite_uri(text, unix_now())?;
        if invite.device_id == self.inner.id() {
            return Err(CoreError::InvalidInput(
                "this is this device's own code".into(),
            ));
        }
        self.spawn(net::qr_scan(self.inner.clone(), invite));
        Ok(())
    }

    /// Start code-confirmation pairing with a nearby device (R3.5).
    pub fn pair_nearby(&self, key: &str) -> Result<()> {
        let short = data_encoding::HEXLOWER_PERMISSIVE
            .decode(key.as_bytes())
            .ok()
            .and_then(|b| <[u8; 8]>::try_from(b.as_slice()).ok())
            .ok_or(CoreError::NotFound)?;
        let addrs = {
            let st = lock(&self.inner.state);
            st.nearby
                .get(&short)
                .map(|n| n.addrs.clone())
                .ok_or(CoreError::NotFound)?
        };
        self.spawn(net::code_initiate(self.inner.clone(), short, addrs));
        Ok(())
    }

    /// "Codes match" (`true`) or "Cancel" (`false`) for a code pairing.
    pub fn confirm_code(&self, key: &str, matches: bool) -> Result<()> {
        let id = DeviceId::from_hex(key).ok_or(CoreError::NotFound)?;
        let st = lock(&self.inner.state);
        let p = st.code_pairings.get(&id).ok_or(CoreError::NotFound)?;
        let _ = p.cmd.send(matches);
        Ok(())
    }

    /// Disconnect without unpairing (R4.7).
    pub fn disconnect(&self, device: &str) -> Result<()> {
        let id = DeviceId::from_hex(device).ok_or(CoreError::NotFound)?;
        let me = lock(&self.inner.state).name.clone();
        self.inner.set_auto_connect(id, false, Some(me))?;
        self.inner
            .send_then_close(id, control::Msg::Disconnect(urdevice_proto::Disconnect {}));
        self.inner.changed();
        Ok(())
    }

    /// Reconnect after Disconnect (R4.7).
    pub fn reconnect(&self, device: &str) -> Result<()> {
        let id = DeviceId::from_hex(device).ok_or(CoreError::NotFound)?;
        self.inner.set_auto_connect(id, true, None)?;
        {
            let mut st = lock(&self.inner.state);
            st.paused.remove(&id);
            st.backoff.insert(
                id,
                Backoff {
                    next: Instant::now(),
                    delay: Duration::from_secs(1),
                },
            );
        }
        self.inner.changed();
        Ok(())
    }

    /// Forget a paired device (R3.9).
    pub fn forget(&self, device: &str) -> Result<()> {
        let id = DeviceId::from_hex(device).ok_or(CoreError::NotFound)?;
        self.inner
            .send_then_close(id, control::Msg::Unpair(urdevice_proto::Unpair {}));
        self.inner.remove_pairing(id)?;
        self.inner.changed();
        Ok(())
    }

    /// Ask a paired device to start an activity (design §7.4). The answer
    /// arrives as [`NodeEvent::ActivityResult`].
    pub fn request_activity(&self, device: &str, cap: Capability) -> Result<()> {
        let id = DeviceId::from_hex(device).ok_or(CoreError::NotFound)?;
        let mut st = lock(&self.inner.state);
        let request_id = st.next_id();
        let Some(tx) = st.sessions.get(&id).map(|s| s.tx.clone()) else {
            drop(st);
            self.inner.emit(NodeEvent::ActivityResult {
                device_id: id.hex(),
                device_name: lock(&self.inner.state).peer_name(id),
                capability: cap.as_str().into(),
                granted: false,
                reason: "offline".into(),
            });
            return Ok(());
        };
        st.pending_requests.insert((id, request_id), cap);
        let _ = tx.send(control::Msg::ActivityRequest(
            urdevice_proto::ActivityRequest {
                request_id,
                capability: cap.as_str().into(),
            },
        ));
        Ok(())
    }

    /// Answer an Always Ask prompt.
    pub fn answer_prompt(&self, id: u64, allow: bool) {
        if let Some(p) = lock(&self.inner.state).prompts.remove(&id) {
            let _ = p.reply.send(allow);
        }
    }

    /// Stop a running activity (R5.4, R5.5).
    pub fn stop_activity(&self, id: u64) {
        self.inner.stop_activities(|a| a.id == id, "stopped");
    }

    /// Report this device's battery (R4.9). `None` when it has none.
    pub fn set_battery(&self, battery: Option<(u8, bool)>) {
        let battery = battery.map(|(p, c)| (p.min(100), c));
        let send = {
            let mut st = lock(&self.inner.state);
            st.battery = battery;
            let due = match st.battery_sent {
                None => true,
                Some((at, last)) => {
                    let moved = match (last, battery) {
                        (Some((a, ca)), Some((b, cb))) => a.abs_diff(b) >= 1 || ca != cb,
                        (a, b) => a != b,
                    };
                    moved && at.elapsed() >= BATTERY_INTERVAL
                }
            };
            if due {
                st.battery_sent = Some((Instant::now(), battery));
                Some(st.device_status())
            } else {
                None
            }
        };
        if let Some(msg) = send {
            self.inner.broadcast(msg);
        }
        self.inner.changed();
    }

    /// Feed a device found by the platform's own mDNS (Android NsdManager).
    pub fn add_discovered(&self, short_id: &str, name: &str, platform: &str, addrs: &[SocketAddr]) {
        let Some(short) = data_encoding::HEXLOWER_PERMISSIVE
            .decode(short_id.as_bytes())
            .ok()
            .and_then(|b| <[u8; 8]>::try_from(b.as_slice()).ok())
        else {
            return;
        };
        for addr in addrs.iter().take(4) {
            let _ = self.inner.seen_tx.try_send(Seen {
                short_id: short,
                name: pairing::clean_name(name),
                platform: pairing::clean_platform(platform),
                addr: *addr,
                via: Via::Mdns,
            });
        }
    }

    /// TXT records and instance name for the platform's own mDNS registration.
    pub fn mdns_record(&self) -> (String, Vec<(String, String)>) {
        let st = lock(&self.inner.state);
        (
            st.name.clone(),
            vec![
                ("v".into(), PROTO_VERSION.to_string()),
                ("id".into(), self.inner.id().short_hex()),
                ("p".into(), self.inner.platform.clone()),
            ],
        )
    }

    /// Create a new identity and delete every pairing (R1.5).
    pub fn reset_identity(&self) -> Result<()> {
        let identity = Identity::reset(self.inner.secrets.as_ref())?;
        self.inner.close_all(close::USER);
        lock(&self.inner.store).remove_all_paired()?;
        let cfg = transport::server_config(&identity, self.inner.policy.clone())?;
        self.inner.endpoint.set_server_config(Some(cfg));
        if let Ok(mut id) = self.inner.identity.write() {
            *id = Arc::new(identity);
        }
        self.inner.reload_paired()?;
        {
            let _g = self.rt.as_ref().map(|rt| rt.enter());
            self.inner.restart_discovery()?;
        }
        self.inner.changed();
        Ok(())
    }

    /// Text export of the stored data (Settings → Export data).
    pub fn export_data(&self) -> Result<String> {
        let snap = self.snapshot();
        let mut out = String::new();
        out.push_str(&format!(
            "UrDevice data export\nDevice: {}\nDevice ID: {}\nPlatform: {}\n\nPermissions:\n",
            snap.me.name, snap.me.fingerprint, snap.me.platform
        ));
        for p in &snap.permissions {
            out.push_str(&format!("  {} = {}\n", p.capability, p.mode.as_str()));
        }
        out.push_str("\nPaired devices:\n");
        for p in &snap.peers {
            out.push_str(&format!(
                "  {} ({}) {}\n",
                p.name, p.platform, p.fingerprint
            ));
        }
        out.push_str("\nActivity log:\n");
        for e in self.activity_log(10_000)? {
            out.push_str(&format!(
                "  {} {} {} {}\n",
                e.ts, e.device_name, e.capability, e.decision
            ));
        }
        Ok(out)
    }

    /// Delete all data and the identity. The app shows onboarding again.
    pub fn delete_all_data(&self) -> Result<()> {
        self.inner.close_all(close::USER);
        lock(&self.inner.store).wipe()?;
        {
            let mut st = lock(&self.inner.state);
            st.onboarded = false;
            st.permissions.clear();
            st.activities.clear();
        }
        self.reset_identity()
    }

    fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        if let Some(rt) = &self.rt {
            rt.spawn(fut);
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.inner.close_all(close::SHUTDOWN);
        self.inner
            .endpoint
            .close(close::SHUTDOWN.into(), b"shutdown");
        lock(&self.inner.discovery).take();
        if let Some(rt) = self.rt.take() {
            // Let the close frames go out, so peers see the device leave at once.
            if tokio::runtime::Handle::try_current().is_err() {
                let ep = self.inner.endpoint.clone();
                rt.block_on(async move {
                    let _ = tokio::time::timeout(Duration::from_millis(500), ep.wait_idle()).await;
                });
            }
            rt.shutdown_timeout(Duration::from_millis(500));
        }
    }
}

impl Inner {
    fn id(&self) -> DeviceId {
        self.identity().id()
    }

    fn identity(&self) -> Arc<Identity> {
        match self.identity.read() {
            Ok(i) => i.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    fn emit(&self, e: NodeEvent) {
        self.events.on_event(e);
    }

    fn changed(&self) {
        self.emit(NodeEvent::Changed);
    }

    fn port(&self) -> u16 {
        self.endpoint.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    fn announce(&self, name: &str) -> Announce {
        Announce {
            v: PROTO_VERSION,
            short_id: self.id().short().to_vec(),
            name: name.to_owned(),
            port: u32::from(self.port()),
            platform: self.platform.clone(),
        }
    }

    fn restart_discovery(&self) -> Result<()> {
        let (name, hidden) = {
            let st = lock(&self.state);
            (st.name.clone(), st.hidden)
        };
        let mut d = lock(&self.discovery);
        d.take();
        *d = Some(Discovery::start(
            &self.discovery_cfg,
            self.announce(&name),
            hidden,
            self.seen_tx.clone(),
        )?);
        Ok(())
    }

    /// This device's addresses. Reading the interfaces is slow on some
    /// systems, and the UI asks on every redraw, so the list is kept for
    /// [`ADDR_CACHE_TTL`].
    fn local_addrs(&self) -> Vec<IpAddr> {
        if let Some(a) = &self.invite_addrs {
            return a.clone();
        }
        let mut cache = lock(&self.addr_cache);
        if let Some((at, addrs)) = cache.as_ref()
            && at.elapsed() < ADDR_CACHE_TTL
        {
            return addrs.clone();
        }
        let addrs = local_ipv4_addrs();
        *cache = Some((Instant::now(), addrs.clone()));
        addrs
    }

    fn new_pair_screen(&self, st: &State) -> Result<PairScreen> {
        let token_v = crate::crypto::random_bytes(TOKEN_LEN)?;
        let mut token = [0; TOKEN_LEN];
        token.copy_from_slice(&token_v);
        let invite = urdevice_proto::PairInvite {
            v: PROTO_VERSION,
            device_id: self.id().0.to_vec(),
            name: st.name.clone(),
            platform: self.platform.clone(),
            addrs: self.local_addrs().iter().map(|a| a.to_string()).collect(),
            port: u32::from(self.port()),
            token: token.to_vec(),
            expires_at: unix_now() + TOKEN_TTL.as_secs(),
        };
        Ok(PairScreen {
            token,
            expires: Instant::now() + TOKEN_TTL,
            uri: pairing::invite_uri(&invite),
        })
    }

    /// Take the current QR token (single use, R3.2) and show a new one.
    fn take_token(&self) -> Option<[u8; TOKEN_LEN]> {
        let mut st = lock(&self.state);
        let screen = st.pair_screen.as_ref()?;
        let token = (screen.expires > Instant::now()).then_some(screen.token);
        match self.new_pair_screen(&st) {
            Ok(s) => st.pair_screen = Some(s),
            Err(_) => st.pair_screen = None,
        }
        drop(st);
        self.changed();
        token
    }

    /// Recompute whether unknown devices may connect.
    fn update_policy(&self) {
        let mut st = lock(&self.state);
        let locked = st.lockout.remaining(Instant::now()).is_some();
        let open = st.pair_screen.is_some() && !locked && !st.hidden;
        self.policy.set_pairing_open(open);
        self.policy.set_paired(st.paired.iter().map(|d| d.id));
    }

    fn pairing_allowed(&self) -> std::result::Result<(), &'static str> {
        let mut st = lock(&self.state);
        if st.hidden {
            return Err("hidden");
        }
        if st.lockout.remaining(Instant::now()).is_some() {
            return Err("locked");
        }
        if st.pair_screen.is_none() {
            return Err("closed");
        }
        Ok(())
    }

    fn pairing_failed(&self, name: &str, reason: &str) {
        let locked = lock(&self.state).lockout.fail(Instant::now());
        tracing::info!(reason, "pairing failed");
        self.update_policy();
        self.emit(NodeEvent::PairingFinished {
            name: name.into(),
            ok: false,
            reason: reason.into(),
        });
        if locked {
            self.emit(NodeEvent::Notice {
                code: "pairing_locked".into(),
                device_name: String::new(),
            });
        }
        self.changed();
    }

    fn reload_paired(&self) -> Result<()> {
        let paired = lock(&self.store).paired_devices()?;
        lock(&self.state).paired = paired;
        self.update_policy();
        Ok(())
    }

    fn store_pairing(
        &self,
        id: DeviceId,
        name: &str,
        platform: &str,
        addr: SocketAddr,
    ) -> Result<()> {
        let d = PairedDevice {
            id,
            name: pairing::clean_name(name),
            platform: pairing::clean_platform(platform),
            paired_at: unix_now(),
            last_seen: unix_now(),
            last_addrs: vec![addr],
            auto_connect: true,
            disconnected_by: None,
        };
        lock(&self.store).upsert_paired(&d)?;
        self.reload_paired()?;
        tracing::info!(peer = %id.short_hex(), "paired");
        Ok(())
    }

    fn remove_pairing(&self, id: DeviceId) -> Result<()> {
        lock(&self.store).remove_paired(id)?;
        {
            let mut st = lock(&self.state);
            st.backoff.remove(&id);
            st.pending_requests.retain(|(p, _), _| *p != id);
        }
        self.reload_paired()?;
        self.stop_activities(|a| a.peer == id, "stopped");
        Ok(())
    }

    fn set_auto_connect(&self, id: DeviceId, on: bool, by: Option<String>) -> Result<()> {
        let mut d = lock(&self.state)
            .paired(id)
            .cloned()
            .ok_or(CoreError::NotFound)?;
        if d.auto_connect == on && d.disconnected_by == by {
            return Ok(());
        }
        d.auto_connect = on;
        d.disconnected_by = by;
        lock(&self.store).upsert_paired(&d)?;
        self.reload_paired()
    }

    /// Update a paired device's name and platform when they changed.
    fn refresh_peer_info(&self, id: DeviceId, name: &str, platform: &str) {
        let Some(mut d) = lock(&self.state).paired(id).cloned() else {
            return;
        };
        let (name, platform) = (pairing::clean_name(name), pairing::clean_platform(platform));
        if d.name == name && d.platform == platform {
            return;
        }
        d.name = name;
        d.platform = platform;
        if lock(&self.store).upsert_paired(&d).is_ok() {
            let _ = self.reload_paired();
        }
    }

    fn broadcast(&self, msg: control::Msg) {
        let st = lock(&self.state);
        for s in st.sessions.values() {
            let _ = s.tx.send(msg.clone());
        }
    }

    fn broadcast_permissions(&self) {
        let msg = lock(&self.state).permission_state(&self.platform);
        self.broadcast(msg);
    }

    /// Queue a message, then close the session shortly after so it gets out.
    fn send_then_close(&self, id: DeviceId, msg: control::Msg) {
        let session = lock(&self.state).sessions.remove(&id);
        if let Some(s) = session {
            let _ = s.tx.send(msg);
            let conn = s.conn;
            self.rt.spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                conn.close(close::USER.into(), b"user");
            });
        }
    }

    fn close_all(&self, code: u32) {
        let sessions: Vec<_> = lock(&self.state).sessions.drain().collect();
        for (_, s) in sessions {
            s.conn.close(code.into(), b"");
        }
    }

    fn revoke_capability(&self, cap: Capability) {
        self.stop_activities(|a| a.incoming && a.cap == cap, "stopped");
    }

    /// End matching activities and tell the other side.
    fn stop_activities(&self, pred: impl Fn(&Activity) -> bool, decision: &str) {
        let stopped: Vec<(Activity, Option<mpsc::UnboundedSender<control::Msg>>)> = {
            let mut st = lock(&self.state);
            let (gone, keep): (Vec<_>, Vec<_>) = st.activities.drain(..).partition(|a| pred(a));
            st.activities = keep;
            gone.into_iter()
                .map(|a| {
                    let tx = st.sessions.get(&a.peer).map(|s| s.tx.clone());
                    (a, tx)
                })
                .collect()
        };
        if stopped.is_empty() {
            return;
        }
        for (a, tx) in &stopped {
            if let Some(tx) = tx {
                let _ = tx.send(control::Msg::ActivityStop(urdevice_proto::ActivityStop {
                    request_id: a.request_id,
                }));
            }
            if a.incoming {
                let _ = lock(&self.store).log_activity(
                    a.peer,
                    &a.peer_name,
                    a.cap.as_str(),
                    decision,
                    unix_now(),
                );
            }
        }
        self.changed();
    }

    fn snapshot(&self) -> Snapshot {
        let identity = self.identity();
        let id = identity.id();
        let mut st = lock(&self.state);
        let now = Instant::now();
        let locked = st.lockout.remaining(now);
        let me = LocalView {
            name: st.name.clone(),
            id: id.hex(),
            fingerprint: id.fingerprint(),
            platform: self.platform.clone(),
            addresses: self.local_addrs().iter().map(|a| a.to_string()).collect(),
            port: self.port(),
            hidden: st.hidden,
            onboarded: st.onboarded,
            battery_pct: st.battery.map(|b| b.0),
            charging: st.battery.is_some_and(|b| b.1),
        };
        let permissions = Capability::for_platform(&self.platform)
            .into_iter()
            .map(|c| PermissionView {
                capability: c,
                mode: st.mode(c),
            })
            .collect();
        let peers = st
            .paired
            .iter()
            .map(|d| {
                let s = st.sessions.get(&d.id);
                let status = match (s, d.auto_connect) {
                    (Some(_), _) => PeerStatus::Connected,
                    (None, false) => PeerStatus::Disconnected {
                        by: d.disconnected_by.clone().unwrap_or_default(),
                    },
                    (None, true) => PeerStatus::Offline,
                };
                let address = s
                    .map(|s| s.conn.remote_address().ip().to_string())
                    .or_else(|| d.last_addrs.first().map(|a| a.ip().to_string()))
                    .unwrap_or_default();
                PeerView {
                    id: d.id.hex(),
                    fingerprint: d.id.fingerprint(),
                    name: d.name.clone(),
                    platform: d.platform.clone(),
                    status,
                    last_seen: if s.is_some() { unix_now() } else { d.last_seen },
                    address,
                    path: "wifi".into(),
                    rtt_ms: s.map(|s| rtt_ms(&s.conn)),
                    battery_pct: s.and_then(|s| s.battery.map(|b| b.0)),
                    charging: s.and_then(|s| s.battery).is_some_and(|b| b.1),
                    remote_permissions: s.map(|s| s.remote_perms.clone()).unwrap_or_default(),
                }
            })
            .collect();
        let paired_short: HashSet<[u8; 8]> = st.paired.iter().map(|d| d.id.short()).collect();
        let mut nearby: Vec<NearbyView> = st
            .nearby
            .iter()
            .filter(|(k, n)| {
                !paired_short.contains(*k) && now.duration_since(n.last_seen) < NEARBY_TTL
            })
            .map(|(k, n)| NearbyView {
                key: data_encoding::HEXLOWER.encode(k),
                name: n.name.clone(),
                platform: n.platform.clone(),
                address: n
                    .addrs
                    .first()
                    .map(|a| a.ip().to_string())
                    .unwrap_or_default(),
            })
            .collect();
        nearby.sort_by(|a, b| a.name.cmp(&b.name).then(a.key.cmp(&b.key)));
        let pair_screen = st.pair_screen.as_ref().map(|p| PairScreenView {
            qr_uri: if locked.is_some() || st.hidden {
                String::new()
            } else {
                p.uri.clone()
            },
            expires_in_secs: u32::try_from(p.expires.saturating_duration_since(now).as_secs())
                .unwrap_or(0),
            locked_secs: locked
                .map(|d| u32::try_from(d.as_secs()).unwrap_or(0) + 1)
                .unwrap_or(0),
            blocked_by_hidden: st.hidden,
        });
        let code_pairings = st
            .code_pairings
            .iter()
            .map(|(k, p)| CodePairingView {
                key: k.hex(),
                name: p.name.clone(),
                platform: p.platform.clone(),
                code: p.code.clone(),
                confirmed_here: p.confirmed_here,
                incoming: p.incoming,
            })
            .collect();
        let activities = st
            .activities
            .iter()
            .map(|a| ActivityView {
                id: a.id,
                device_id: a.peer.hex(),
                device_name: a.peer_name.clone(),
                capability: a.cap.as_str().into(),
                incoming: a.incoming,
                started_at: a.started_at,
            })
            .collect();
        Snapshot {
            me,
            permissions,
            peers,
            nearby,
            pair_screen,
            code_pairings,
            activities,
        }
    }
}

fn rtt_ms(conn: &quinn::Connection) -> u32 {
    u32::try_from(conn.rtt().as_millis()).unwrap_or(u32::MAX)
}

/// Non-loopback IPv4 addresses, skipping container and VM bridges.
pub fn local_ipv4_addrs() -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !crate::discovery::skip_interface(&i.name))
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v4) if !v4.ip.is_loopback() && !v4.ip.is_link_local() => {
                Some(IpAddr::V4(v4.ip))
            }
            _ => None,
        })
        .collect();
    out.sort_by_key(|ip| match ip {
        IpAddr::V4(v4) if v4.is_private() => 0,
        _ => 1,
    });
    out.dedup();
    out.truncate(6);
    out
}
