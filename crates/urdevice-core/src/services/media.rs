//! Media control (#20, design §9.4, R11).
//!
//! Service id: [`ServiceId::MEDIA`], capability `media.v1` (Release 1,
//! default Allow, not watched).
//!
//! **Host side.** A controller sends `Subscribe { on: true }`. While at least
//! one controller is subscribed, the service polls the [`MediaHost`] once a
//! second and pushes a [`MediaState`] to each subscriber when something other
//! than the playback clock changed. `Subscribe { on: false }`, a closed
//! session, a change of the setting to Deny, and the end of an Always Ask
//! activity all stop the pushes (R11.6, R5.5). `Cmd` and `VolumeCmd` reach the
//! host adapter only with the [`Grant`] the node made, so the adapter acts
//! only on traffic the permission engine allowed.
//!
//! **Controller side.** [`Media::subscribe`], [`Media::command`], and
//! [`Media::volume`] are called by the UI. A pushed state is accepted only
//! from a host this device subscribed to, is sanitized, and is shown, never
//! acted on.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, mpsc};
use urdevice_proto::features::media::{self as wire, Media as WireMedia, media::Msg};

use super::{Access, Direction, Grant, Message, Registry, Service, ServiceCtx, ServiceId, Slot};
use crate::identity::DeviceId;
use crate::permission::{Capability, Mode};
use crate::{CoreError, Result};

pub use urdevice_proto::features::media::{
    CmdKind, MAX_ARTWORK, MAX_ID, MAX_SESSIONS, PlayState, Session as MediaSession,
    State as MediaState, volume_cmd::Cmd as VolumeChange,
};

/// Release 2 capabilities this module ships. Media is a Release 1
/// capability, which is always listed, so this stays empty.
pub(crate) const SHIPPED: &[Capability] = &[];

/// How often the host reads its media sessions while someone is subscribed.
pub const POLL_EVERY: Duration = Duration::from_secs(1);

/// A position that drifts less than this from where the playback clock
/// predicts it is not a change worth pushing.
const POSITION_SLACK_MS: u64 = 2000;

/// A volume that moves less than this is not a change.
const VOLUME_EPSILON: f32 = 0.004;

/// What the host OS provides: its media sessions and its system volume.
/// Desktop: `urdevice-platform`. Android: Kotlin through the FFI.
///
/// The methods may block (D-Bus, WinRT, process calls); the service calls
/// them from the blocking thread pool.
pub trait MediaHost: Send + Sync {
    /// All sessions now, and the system volume.
    fn state(&self) -> MediaState;

    /// Control one session. `grant` is proof the permission engine allowed
    /// this peer; an adapter never acts without one.
    fn command(&self, grant: &Grant, session_id: &str, kind: CmdKind, seek_ms: u64) -> Result<()>;

    /// Change the system output volume or mute.
    fn volume(&self, grant: &Grant, change: &VolumeChange) -> Result<()>;
}

/// What the UI listens to, on the controller side. Called from core
/// threads; hand work to the UI thread.
pub trait MediaEvents: Send + Sync {
    /// A new [`MediaState`] arrived from `peer`, or it was cleared (read it
    /// with [`Media::state_of`]).
    fn updated(&self, peer: DeviceId);

    /// `peer` refused the last request: `denied`, `declined`, `timeout`,
    /// `unsupported`, or `offline`.
    fn refused(&self, peer: DeviceId, reason: &str);
}

/// A subscriber on the host side.
struct Subscriber {
    /// The activity the subscription was granted under, or 0 for Allow.
    activity: u64,
    /// What this controller last got, and when.
    last: Option<(MediaState, Instant)>,
}

/// What the controller side remembers.
#[derive(Default)]
struct Controller {
    /// Hosts the user turned the player on for.
    wanted: HashSet<DeviceId>,
    /// The newest state from each host, and when it arrived.
    latest: HashMap<DeviceId, (MediaState, Instant)>,
    /// Always Ask activity per host, once approved.
    activity: HashMap<DeviceId, u64>,
    /// The last message a host refused with `ask` or `no_activity`, to send
    /// again once an activity is approved.
    pending: HashMap<DeviceId, WireMedia>,
    /// Hosts an activity request is running for.
    requesting: HashSet<DeviceId>,
}

/// Work for the host adapter, run one at a time in the order it arrived.
enum Job {
    Cmd {
        grant: Grant,
        session_id: String,
        kind: CmdKind,
        seek_ms: u64,
    },
    Volume {
        grant: Grant,
        change: VolumeChange,
    },
}

/// Most jobs waiting for the host adapter. A controller that floods commands
/// loses the excess instead of growing a queue.
const MAX_QUEUED_JOBS: usize = 64;

struct Shared {
    jobs: mpsc::Sender<Job>,
    queue: Mutex<Option<mpsc::Receiver<Job>>>,
    ctx: OnceLock<ServiceCtx>,
    host: Slot<dyn MediaHost>,
    events: Slot<dyn MediaEvents>,
    subs: Mutex<HashMap<DeviceId, Subscriber>>,
    ctrl: Mutex<Controller>,
    /// Wakes the poll loop early: a new subscriber, or a command was sent.
    wake: Notify,
}

impl Default for Shared {
    fn default() -> Self {
        let (jobs, rx) = mpsc::channel(MAX_QUEUED_JOBS);
        Self {
            jobs,
            queue: Mutex::new(Some(rx)),
            ctx: OnceLock::new(),
            host: Slot::default(),
            events: Slot::default(),
            subs: Mutex::default(),
            ctrl: Mutex::default(),
            wake: Notify::new(),
        }
    }
}

/// Run host-adapter jobs in order. Each waits for the previous one, so a
/// seek followed by a pause is applied in that order.
async fn job_loop(shared: Arc<Shared>, mut rx: mpsc::Receiver<Job>) {
    while let Some(job) = rx.recv().await {
        let Some(host) = shared.host.get() else {
            continue;
        };
        let settle = match &job {
            Job::Cmd { .. } => Duration::from_millis(250),
            Job::Volume { .. } => Duration::from_millis(100),
        };
        let r = tokio::task::spawn_blocking(move || match job {
            Job::Cmd {
                grant,
                session_id,
                kind,
                seek_ms,
            } => host.command(&grant, &session_id, kind, seek_ms),
            Job::Volume { grant, change } => host.volume(&grant, &change),
        })
        .await;
        if !matches!(r, Ok(Ok(()))) {
            tracing::debug!("media job failed");
        }
        // Players take a moment to report the new state.
        tokio::time::sleep(settle).await;
        shared.wake.notify_one();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The media service.
#[derive(Default)]
pub struct Media {
    shared: Arc<Shared>,
}

impl Media {
    /// Give the service this device's media sessions and volume.
    pub fn set_host(&self, host: Arc<dyn MediaHost>) {
        self.shared.host.set(host);
    }

    /// Set the UI listener.
    pub fn set_events(&self, events: Arc<dyn MediaEvents>) {
        self.shared.events.set(events);
    }

    /// Tell the host's poll loop that something changed on this device, so
    /// subscribers hear about it before the next tick.
    pub fn poke(&self) {
        self.shared.wake.notify_one();
    }

    fn ctx(&self) -> Result<&ServiceCtx> {
        self.shared.ctx.get().ok_or(CoreError::Stopped)
    }

    // ----- controller side, called by the UI -----

    /// Turn the player for `host` on or off (R11.6). On: the host replies
    /// with its current state. Off: the host stops sending, and the stored
    /// state is dropped.
    pub fn subscribe(&self, host: DeviceId, on: bool) -> Result<()> {
        let ctx = self.ctx()?;
        {
            let mut c = lock(&self.shared.ctrl);
            if on {
                c.wanted.insert(host);
            } else {
                c.wanted.remove(&host);
                c.latest.remove(&host);
            }
        }
        if !on && let Some(e) = self.shared.events.get() {
            e.updated(host);
        }
        self.send_to_host(
            ctx,
            host,
            wire::media::Msg::Subscribe(wire::Subscribe { on }),
        )
    }

    /// True while the player for `host` is on.
    pub fn is_on(&self, host: DeviceId) -> bool {
        lock(&self.shared.ctrl).wanted.contains(&host)
    }

    /// Control one session on `host` (R11.1, R11.2). `session_id` is from
    /// the last state; `seek_ms` is used by [`CmdKind::Seek`].
    pub fn command(
        &self,
        host: DeviceId,
        session_id: &str,
        kind: CmdKind,
        seek_ms: u64,
    ) -> Result<()> {
        if session_id.is_empty() || session_id.len() > MAX_ID {
            return Err(CoreError::InvalidInput("bad media session id".into()));
        }
        let ctx = self.ctx()?;
        self.send_to_host(
            ctx,
            host,
            Msg::Cmd(wire::Cmd {
                session_id: session_id.into(),
                kind: kind as i32,
                seek_ms,
            }),
        )
    }

    /// Change the volume or mute of `host`'s system output (R11.1).
    pub fn volume(&self, host: DeviceId, change: VolumeChange) -> Result<()> {
        let ctx = self.ctx()?;
        self.send_to_host(
            ctx,
            host,
            Msg::Volume(wire::VolumeCmd { cmd: Some(change) }),
        )
    }

    /// The newest state from `host`, with the positions of playing sessions
    /// moved forward by the time since it arrived. `None` while the player is
    /// off or before the first state.
    pub fn state_of(&self, host: DeviceId) -> Option<MediaState> {
        let c = lock(&self.shared.ctrl);
        let (st, at) = c.latest.get(&host)?;
        Some(advance(st, at.elapsed()))
    }

    fn send_to_host(&self, ctx: &ServiceCtx, host: DeviceId, msg: Msg) -> Result<()> {
        if !ctx.peer_offers(host) {
            return Err(CoreError::Refused("unsupported".into()));
        }
        let m = WireMedia::from(msg);
        let activity = {
            let mut c = lock(&self.shared.ctrl);
            c.pending.insert(host, m.clone());
            c.activity.get(&host).copied().unwrap_or(0)
        };
        ctx.send(host, Direction::ToHost, activity, &m)
    }
}

/// `st` with each playing session's position moved forward by `by`.
fn advance(st: &MediaState, by: Duration) -> MediaState {
    let mut out = st.clone();
    let ms = u64::try_from(by.as_millis()).unwrap_or(u64::MAX);
    for s in &mut out.sessions {
        if s.play_state() == PlayState::Playing
            && let Some(p) = s.position_ms
        {
            let moved = p.saturating_add(ms);
            s.position_ms = Some(
                s.duration_ms
                    .filter(|d| *d > 0)
                    .map_or(moved, |d| moved.min(d)),
            );
        }
    }
    out
}

/// True when `new` differs from what a controller already has (`old`, sent
/// `age` ago) in more than the playback clock.
fn differs(old: &MediaState, age: Duration, new: &MediaState) -> bool {
    if old.sessions.len() != new.sessions.len()
        || old.muted != new.muted
        || old.can_volume != new.can_volume
        || old.unavailable != new.unavailable
        || (old.volume - new.volume).abs() > VOLUME_EPSILON
    {
        return true;
    }
    let predicted = advance(old, age);
    old.sessions
        .iter()
        .zip(&predicted.sessions)
        .zip(&new.sessions)
        .any(|((o, p), n)| {
            let (mut a, mut b) = (o.clone(), n.clone());
            a.position_ms = None;
            b.position_ms = None;
            if a != b {
                return true;
            }
            match (p.position_ms, n.position_ms) {
                (Some(x), Some(y)) => x.abs_diff(y) > POSITION_SLACK_MS,
                (None, None) => false,
                _ => true,
            }
        })
}

/// Read the host's state off the async runtime, with the limits applied.
async fn read_host(shared: &Arc<Shared>) -> MediaState {
    let Some(host) = shared.host.get() else {
        return MediaState {
            unavailable: "unsupported".into(),
            ..MediaState::default()
        };
    };
    let mut st = tokio::task::spawn_blocking(move || host.state())
        .await
        .unwrap_or_else(|_| MediaState {
            unavailable: "unsupported".into(),
            ..MediaState::default()
        });
    st.sanitize();
    st
}

/// Send `st` to `peer` and remember it as what the peer has.
fn push(shared: &Shared, ctx: &ServiceCtx, peer: DeviceId, st: &MediaState) {
    let mut subs = lock(&shared.subs);
    let Some(sub) = subs.get_mut(&peer) else {
        return;
    };
    let activity = sub.activity;
    let m = WireMedia::from(Msg::State(st.clone()));
    if ctx
        .send(peer, Direction::ToController, activity, &m)
        .is_ok()
    {
        sub.last = Some((st.clone(), Instant::now()));
    }
}

/// Drop subscribers the host's setting no longer allows.
fn enforce_mode(shared: &Shared, ctx: &ServiceCtx) {
    let mode = ctx.local_mode();
    let mut subs = lock(&shared.subs);
    match mode {
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
        let peers: Vec<DeviceId> = lock(&shared.subs).keys().copied().collect();
        if peers.is_empty() {
            continue;
        }
        let st = read_host(&shared).await;
        for peer in peers {
            let send = {
                let subs = lock(&shared.subs);
                match subs.get(&peer) {
                    None => false,
                    Some(s) => s
                        .last
                        .as_ref()
                        .is_none_or(|(old, at)| differs(old, at.elapsed(), &st)),
                }
            };
            if send {
                push(&shared, &ctx, peer, &st);
            }
        }
    }
}

impl Service for Media {
    fn id(&self) -> ServiceId {
        ServiceId::MEDIA
    }

    fn capability(&self) -> Capability {
        Capability::Media
    }

    fn start(&self, ctx: ServiceCtx) {
        if self.shared.ctx.set(ctx.clone()).is_ok() {
            ctx.spawn(poll_loop(self.shared.clone(), ctx.clone()));
            if let Some(rx) = lock(&self.shared.queue).take() {
                ctx.spawn(job_loop(self.shared.clone(), rx));
            }
        }
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> Result<()> {
        // Untrusted: a body that does not decode closes the session.
        let m: WireMedia = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad media message".into()))?;
        let Some(inner) = m.msg else {
            return Ok(());
        };
        match (msg.access, inner) {
            (Access::Host(grant), Msg::Subscribe(s)) => self.host_subscribe(ctx, &grant, s.on),
            (Access::Host(grant), Msg::Cmd(c)) => self.host_command(grant, c),
            (Access::Host(grant), Msg::Volume(v)) => self.host_volume(grant, v),
            (Access::Controller { .. }, Msg::State(st)) => self.on_state(msg.peer, st),
            // A state sent to the host, or a command sent to the controller:
            // never act on it.
            _ => {}
        }
        Ok(())
    }

    fn on_refused(&self, ctx: &ServiceCtx, peer: DeviceId, _activity: u64, reason: &str) {
        let retry = matches!(reason, "ask" | "no_activity");
        let pending = {
            let mut c = lock(&self.shared.ctrl);
            if retry {
                c.activity.remove(&peer);
            }
            if retry && !c.requesting.contains(&peer) {
                c.pending.remove(&peer)
            } else {
                None
            }
        };
        let (Some(m), true) = (pending, retry) else {
            self.refused(peer, reason);
            return;
        };
        let shared = self.shared.clone();
        let ctx = ctx.clone();
        lock(&shared.ctrl).requesting.insert(peer);
        let task_ctx = ctx.clone();
        ctx.spawn(async move {
            // One approval, then the message goes again. A second refusal
            // has nothing pending, so it is reported, not retried.
            let r = task_ctx.request(peer).await;
            lock(&shared.ctrl).requesting.remove(&peer);
            match r {
                Ok(activity) => {
                    lock(&shared.ctrl).activity.insert(peer, activity);
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
        let mut c = lock(&self.shared.ctrl);
        if c.activity.get(&peer) == Some(&activity) {
            c.activity.remove(&peer);
        }
    }

    fn on_peer(&self, ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        if connected {
            // The user left the player on: ask again on the new session.
            let wanted = lock(&self.shared.ctrl).wanted.contains(&peer);
            if wanted && ctx.peer_offers(peer) {
                let m = WireMedia::from(Msg::Subscribe(wire::Subscribe { on: true }));
                let _ = ctx.send(peer, Direction::ToHost, 0, &m);
            }
            return;
        }
        lock(&self.shared.subs).remove(&peer);
        let had = {
            let mut c = lock(&self.shared.ctrl);
            c.activity.remove(&peer);
            c.pending.remove(&peer);
            c.requesting.remove(&peer);
            c.latest.remove(&peer).is_some()
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

impl Media {
    fn refused(&self, peer: DeviceId, reason: &str) {
        if reason == "denied" || reason == "unsupported" {
            let mut c = lock(&self.shared.ctrl);
            c.wanted.remove(&peer);
            c.latest.remove(&peer);
        }
        notify_refused(&self.shared, peer, reason);
        if let Some(e) = self.shared.events.get() {
            e.updated(peer);
        }
    }

    fn host_subscribe(&self, ctx: &ServiceCtx, grant: &Grant, on: bool) {
        let peer = grant.peer();
        if !on {
            lock(&self.shared.subs).remove(&peer);
            ctx.log(peer, "media player off");
            return;
        }
        let fresh = lock(&self.shared.subs)
            .insert(
                peer,
                Subscriber {
                    activity: grant.activity(),
                    last: None,
                },
            )
            .is_none();
        if fresh {
            ctx.log(peer, "media player on");
        }
        // The current state goes out now, not after the next tick.
        let shared = self.shared.clone();
        let task_ctx = ctx.clone();
        ctx.spawn(async move {
            let st = read_host(&shared).await;
            push(&shared, &task_ctx, peer, &st);
        });
    }

    fn host_command(&self, grant: Grant, c: wire::Cmd) {
        let Ok(kind) = CmdKind::try_from(c.kind) else {
            return;
        };
        if c.session_id.is_empty() || c.session_id.len() > MAX_ID {
            return;
        }
        let _ = self.shared.jobs.try_send(Job::Cmd {
            grant,
            session_id: c.session_id,
            kind,
            seek_ms: c.seek_ms,
        });
    }

    fn host_volume(&self, grant: Grant, v: wire::VolumeCmd) {
        let Some(change) = v.cmd.map(clamp_volume) else {
            return;
        };
        let _ = self.shared.jobs.try_send(Job::Volume { grant, change });
    }

    fn on_state(&self, peer: DeviceId, mut st: MediaState) {
        {
            let mut c = lock(&self.shared.ctrl);
            // Only a host this device turned the player on for may push.
            if !c.wanted.contains(&peer) {
                return;
            }
            st.sanitize();
            c.latest.insert(peer, (st, Instant::now()));
        }
        if let Some(e) = self.shared.events.get() {
            e.updated(peer);
        }
    }
}

/// Keep a volume command inside what the host will apply.
fn clamp_volume(c: VolumeChange) -> VolumeChange {
    match c {
        VolumeChange::Set(v) => VolumeChange::Set(if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }),
        VolumeChange::Step(s) => {
            VolumeChange::Step(s.clamp(-wire::MAX_VOLUME_STEP, wire::MAX_VOLUME_STEP))
        }
        VolumeChange::Mute(m) => VolumeChange::Mute(m),
    }
}

/// Add this feature's services to a starting node. Every platform offers
/// `media.v1`: a host without a [`MediaHost`] answers with an empty state.
pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Arc::new(Media::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(pos: u64, state: PlayState) -> MediaSession {
        MediaSession {
            id: "a".into(),
            app_name: "App".into(),
            state: state as i32,
            position_ms: Some(pos),
            duration_ms: Some(100_000),
            ..MediaSession::default()
        }
    }

    fn state(s: MediaSession) -> MediaState {
        MediaState {
            sessions: vec![s],
            volume: 0.5,
            can_volume: true,
            ..MediaState::default()
        }
    }

    #[test]
    fn playback_clock_alone_is_not_a_change() {
        let old = state(session(10_000, PlayState::Playing));
        // 3 s later the player reports 13 s: the clock predicted that.
        let new = state(session(13_100, PlayState::Playing));
        assert!(!differs(&old, Duration::from_secs(3), &new));
        // A jump (seek) is a change.
        let new = state(session(60_000, PlayState::Playing));
        assert!(differs(&old, Duration::from_secs(3), &new));
    }

    #[test]
    fn paused_position_does_not_advance() {
        let old = state(session(10_000, PlayState::Paused));
        let new = state(session(10_050, PlayState::Paused));
        assert!(!differs(&old, Duration::from_secs(30), &new));
        let new = state(session(14_000, PlayState::Paused));
        assert!(differs(&old, Duration::from_secs(30), &new));
    }

    #[test]
    fn metadata_volume_and_flags_are_changes() {
        let old = state(session(0, PlayState::Playing));
        let mut new = old.clone();
        new.sessions[0].title = "next".into();
        assert!(differs(&old, Duration::ZERO, &new));
        let mut new = old.clone();
        new.volume = 0.6;
        assert!(differs(&old, Duration::ZERO, &new));
        let mut new = old.clone();
        new.muted = true;
        assert!(differs(&old, Duration::ZERO, &new));
        let mut new = old.clone();
        new.sessions[0].can_seek = true;
        assert!(differs(&old, Duration::ZERO, &new));
        let mut new = old.clone();
        new.sessions.clear();
        assert!(differs(&old, Duration::ZERO, &new));
        assert!(!differs(&old, Duration::ZERO, &old));
    }

    #[test]
    fn advance_stops_at_the_end_of_the_track() {
        let st = state(session(99_000, PlayState::Playing));
        let s = advance(&st, Duration::from_secs(10));
        assert_eq!(s.sessions[0].position_ms, Some(100_000));
        let st = state(session(1_000, PlayState::Paused));
        assert_eq!(
            advance(&st, Duration::from_secs(10)).sessions[0].position_ms,
            Some(1_000)
        );
    }

    #[test]
    fn volume_commands_are_clamped() {
        assert_eq!(clamp_volume(VolumeChange::Set(3.0)), VolumeChange::Set(1.0));
        assert_eq!(
            clamp_volume(VolumeChange::Set(f32::NAN)),
            VolumeChange::Set(0.0)
        );
        assert_eq!(
            clamp_volume(VolumeChange::Step(-1000)),
            VolumeChange::Step(-25)
        );
        assert_eq!(
            clamp_volume(VolumeChange::Mute(true)),
            VolumeChange::Mute(true)
        );
    }
}
