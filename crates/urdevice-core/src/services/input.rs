//! Remote control: pointer, keyboard, text, and presenter (#18, #19, design §9.3).
//!
//! Service id: [`ServiceId::INPUT`], capability `input.v1` (Release 1, default
//! Always Ask, **watched**: it always needs an activity, so the host shows the
//! indicator and Stop button, R5.4).
//!
//! **Controller side.** The UI calls [`Input::connect`], which asks the host
//! for an activity (`ctx.request`, so the host's user approves under Always
//! Ask). Once approved, [`Input::pointer_move`], [`Input::button`],
//! [`Input::key`], [`Input::text`], [`Input::laser`] and the others queue
//! events. A flush task sends them every [`FLUSH_EVERY`] as QUIC datagrams
//! (moves, scrolls, and laser positions, where a lost event is harmless) or
//! on the control stream (buttons, keys, and text, which must not be lost, and
//! everything when the path takes no datagrams). A tap that follows moves flushes the
//! moves first, so the click lands where the user stopped.
//!
//! **Host side.** Events reach the [`InputSink`] only with the [`Grant`] the
//! node made, and only after the node checked the activity. A single worker
//! thread applies them in order, so a slow OS call never blocks the node. The
//! service remembers which keys and buttons each activity holds and releases
//! them when the activity ends, the session closes, or the setting changes to
//! Deny (R5.5), so Stop never leaves a key stuck. Everything a controller
//! sends is clamped by [`urdevice_proto::features::input`] and rate limited.
//! Keys, text, and positions are never logged.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use urdevice_proto::features::input::{
    self as wire, Batch, Event, Hello, Input as WireInput, Status as WireStatus, event::Kind,
    input::Msg,
};

use super::{
    Access, Datagram, Direction, Grant, Message, Registry, Service, ServiceCtx, ServiceId, Slot,
};
use crate::identity::DeviceId;
use crate::permission::Capability;
use crate::{CoreError, Result};

pub use urdevice_proto::features::input::{
    COORD_MAX, MAX_DELTA, MAX_EVENTS, MAX_TEXT_BYTES, SCROLL_NOTCH, button, modifier, nav, usage,
};

mod capture;
mod touch;

pub use capture::{
    KeyOut, KeyboardCapture, Mods, RELEASE_HOTKEY, RelativePointer, host_moves_by_distance,
    normalize, scroll_units,
};
pub use touch::{Gesture, TouchHost, TouchSink, android_keycode, meta};

/// Release 2 capabilities this module ships. Remote control is a Release 1
/// capability, which is always listed, so this stays empty.
pub(crate) const SHIPPED: &[Capability] = &[];

/// How often the controller sends queued pointer events (design §9.3).
pub const FLUSH_EVERY: Duration = Duration::from_millis(8);

/// Most events per second the host takes from one controller. Real use is
/// about 125 batches of a few events; this only stops a flood.
const EVENTS_PER_SEC: f64 = 6000.0;

/// Burst the host takes before the rate applies.
const EVENT_BURST: f64 = 1500.0;

/// Most jobs waiting for the worker. A controller that floods loses the rest.
const MAX_QUEUED_JOBS: usize = 1024;

/// What the host's OS lets this process do (R9.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermStatus {
    /// Input can be injected.
    Ready,
    /// The user must grant a permission first.
    Missing(Missing),
    /// Input cannot work here: no display, no back-end, or the user declined.
    Unavailable,
}

/// A permission the host's OS needs, with the settings screen that grants it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// macOS: Privacy & Security > Accessibility.
    Accessibility,
    /// Linux without a RemoteDesktop portal: write access to `/dev/uinput`,
    /// which needs a udev rule.
    Uinput,
    /// Wayland: the user did not approve the RemoteDesktop portal dialog.
    Portal,
    /// Android: the UrDevice accessibility service, which taps, swipes and
    /// runs Back, Home and Recents, is not turned on in Android settings.
    AccessibilityService,
    /// Android: the UrDevice keyboard, which types the text, is not turned on
    /// or not the keyboard in use.
    InputMethod,
}

impl Missing {
    /// The name that goes on the wire in `Status::permission`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accessibility => "accessibility",
            Self::Uinput => "uinput",
            Self::Portal => "portal",
            Self::AccessibilityService => "accessibility_service",
            Self::InputMethod => "input_method",
        }
    }

    /// The inverse of [`Missing::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "accessibility" => Some(Self::Accessibility),
            "uinput" => Some(Self::Uinput),
            "portal" => Some(Self::Portal),
            "accessibility_service" => Some(Self::AccessibilityService),
            "input_method" => Some(Self::InputMethod),
            _ => None,
        }
    }
}

/// A pointer event for the host, after the core checked it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerEvent {
    /// Move by a distance, in pixels.
    Move { dx: i32, dy: i32 },
    /// Go to a place: 0 to [`COORD_MAX`] across display `display`.
    Abs { x: u32, y: u32, display: u32 },
    /// A button changed state; `button` is one of [`button`].
    Button { button: u32, down: bool },
    /// Wheel movement in [`SCROLL_NOTCH`] units; down and right are positive.
    Scroll { dx: i32, dy: i32 },
}

/// A plain key press or release for the host. Modifier keys are keys too:
/// the core turns `Key::modifiers` into presses of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// USB HID usage, page 0x07.
    pub usage: u32,
    /// True when pressed.
    pub down: bool,
}

/// What the host OS provides to inject input. Desktop: `urdevice-platform`.
/// Android (#19): Kotlin through the FFI.
///
/// Every method may block (D-Bus, `SendInput`); the service calls them from
/// its own worker thread, one at a time, in the order events arrived. Each
/// takes the [`Grant`]: an adapter never acts without proof the permission
/// engine allowed this peer. Never log what is typed or where the pointer is.
pub trait InputSink: Send + Sync {
    /// Name of the back-end, for example `x11`, `wayland-portal`, or `windows`.
    fn name(&self) -> &'static str;

    /// What the OS allows now, without asking the user anything.
    fn permission_status(&self) -> PermStatus;

    /// Get ready for a session: connect, and where the OS asks the user (the
    /// Wayland portal dialog, the macOS Accessibility prompt), ask. Returns
    /// what is possible afterwards.
    fn prepare(&self, grant: &Grant) -> PermStatus {
        let _ = grant;
        self.permission_status()
    }

    /// Move the pointer, press a button, or turn the wheel.
    fn pointer(&self, grant: &Grant, event: &PointerEvent) -> Result<()>;

    /// Press or release one key.
    fn key(&self, grant: &Grant, event: &KeyEvent) -> Result<()>;

    /// Type Unicode text as text, not as key codes (R10.4).
    fn text(&self, grant: &Grant, text: &str) -> Result<()>;

    /// Run a system navigation action: one of [`nav`](urdevice_proto::features::input::nav).
    /// Only Android hosts have them; the default says so.
    fn nav(&self, grant: &Grant, action: u32) -> Result<()> {
        let _ = (grant, action);
        Err(CoreError::Unsupported {
            feature: "navigation actions",
            platform: self.name(),
        })
    }

    /// What typing needs, apart from the pointer. Android: the UrDevice
    /// keyboard. A desktop types with the same permission as the pointer.
    fn text_status(&self) -> PermStatus {
        self.permission_status()
    }

    /// The primary display in pixels, when the host knows it. The controller
    /// shapes its remote-control window like it.
    fn display_size(&self) -> Option<(u32, u32)> {
        None
    }

    /// Drop what the adapter is building for this activity, before the core
    /// releases the held keys and buttons: Stop must not finish a swipe the
    /// user had only started.
    fn cancel(&self, grant: &Grant) {
        let _ = grant;
    }
}

/// The presenter's laser dot on the host (design §9.3). Desktop draws a
/// click-through, always-on-top window, or moves the cursor where that is
/// not possible.
pub trait Overlay: Send + Sync {
    /// Show the dot at `pos` (0.0 to 1.0 across and down the primary
    /// display), or hide it with `None`.
    fn laser(&self, grant: &Grant, pos: Option<(f32, f32)>) -> Result<()>;
}

/// What the host reported with [`WireStatus`], sanitized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostStatus {
    /// What works on the host.
    pub status: PermStatus,
    /// The host's back-end name, display only.
    pub backend: String,
    /// What typing lacks while the pointer works, when the host says so
    /// (Android: the UrDevice keyboard is off).
    pub text_missing: Option<Missing>,
    /// The host's primary display in pixels, when it reported it.
    pub display: Option<(u32, u32)>,
}

/// What the UI listens to. Called from core threads: hand work to the UI
/// thread.
pub trait InputEvents: Send + Sync {
    /// Controller side: a remote-control session with `peer` started
    /// (`true`) or ended for any reason (`false`).
    fn session(&self, peer: DeviceId, active: bool) {
        let _ = (peer, active);
    }

    /// Controller side: `peer` reported what its input adapter can do.
    fn host_status(&self, peer: DeviceId, status: &HostStatus) {
        let _ = (peer, status);
    }

    /// Controller side: `peer` refused: `denied`, `declined`, `timeout`,
    /// `unsupported`, `offline`, `ask`, or `no_activity`.
    fn refused(&self, peer: DeviceId, reason: &str) {
        let _ = (peer, reason);
    }

    /// Host side: `peer` controls this device, but the OS first needs a
    /// permission. The UI explains it and offers to open the settings.
    fn missing_permission(&self, peer: DeviceId, missing: Missing) {
        let _ = (peer, missing);
    }
}

/// What a host holds for one activity, so Stop can let go.
struct Held {
    grant: Grant,
    keys: BTreeSet<u32>,
    buttons: BTreeSet<u32>,
    /// For a key pressed with modifiers: the modifier keys it pressed.
    implied: HashMap<u32, Vec<u32>>,
    laser: bool,
}

impl Held {
    fn new(grant: &Grant) -> Self {
        Self {
            grant: grant.clone(),
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            implied: HashMap::new(),
            laser: false,
        }
    }
}

/// Work for the worker thread, applied in order.
enum Job {
    Events {
        grant: Grant,
        events: Vec<Event>,
    },
    Hello {
        grant: Grant,
    },
    /// Let go of everything an activity holds (`Some`) or everything a peer
    /// holds (`None`).
    Release {
        peer: DeviceId,
        activity: Option<u64>,
    },
}

/// A token bucket per peer.
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// One controller-side session with a host.
struct Session {
    activity: u64,
    /// Lossy events waiting for the next flush.
    pending: Vec<Event>,
}

#[derive(Default)]
struct Controller {
    sessions: HashMap<DeviceId, Session>,
    requesting: HashSet<DeviceId>,
}

struct Shared {
    ctx: OnceLock<ServiceCtx>,
    sink: Slot<dyn InputSink>,
    overlay: Slot<dyn Overlay>,
    events: Slot<dyn InputEvents>,
    jobs: SyncSender<Job>,
    queue: Mutex<Option<mpsc::Receiver<Job>>>,
    held: Mutex<HashMap<(DeviceId, u64), Held>>,
    buckets: Mutex<HashMap<DeviceId, Bucket>>,
    ctrl: Mutex<Controller>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Default for Shared {
    fn default() -> Self {
        let (jobs, rx) = mpsc::sync_channel(MAX_QUEUED_JOBS);
        Self {
            ctx: OnceLock::new(),
            sink: Slot::default(),
            overlay: Slot::default(),
            events: Slot::default(),
            jobs,
            queue: Mutex::new(Some(rx)),
            held: Mutex::default(),
            buckets: Mutex::default(),
            ctrl: Mutex::default(),
        }
    }
}

/// The remote-control service.
#[derive(Default)]
pub struct Input {
    shared: Arc<Shared>,
}

// ---------------------------------------------------------------------------
// Public API: adapters, listener, controller calls.
// ---------------------------------------------------------------------------

impl Input {
    /// Give the service this device's input injection (host side).
    pub fn set_sink(&self, sink: Arc<dyn InputSink>) {
        self.shared.sink.set(sink);
    }

    /// Give the service the laser overlay (host side).
    pub fn set_overlay(&self, overlay: Arc<dyn Overlay>) {
        self.shared.overlay.set(overlay);
    }

    /// Set the UI listener.
    pub fn set_events(&self, events: Arc<dyn InputEvents>) {
        self.shared.events.set(events);
    }

    fn ctx(&self) -> Result<&ServiceCtx> {
        self.shared.ctx.get().ok_or(CoreError::Stopped)
    }

    /// Start a remote-control session with `host`: ask it for an activity
    /// (its user approves under Always Ask), then send `Hello`. Returns at
    /// once; [`InputEvents::session`] or [`InputEvents::refused`] follows.
    pub fn connect(&self, host: DeviceId) -> Result<()> {
        let ctx = self.ctx()?.clone();
        if !ctx.peer_offers(host) {
            return Err(CoreError::Refused("unsupported".into()));
        }
        {
            let mut c = lock(&self.shared.ctrl);
            if c.sessions.contains_key(&host) || !c.requesting.insert(host) {
                return Ok(());
            }
        }
        let shared = self.shared.clone();
        let task_ctx = ctx.clone();
        ctx.spawn(async move {
            let r = task_ctx.request(host).await;
            match r {
                Ok(activity) => {
                    {
                        let mut c = lock(&shared.ctrl);
                        c.requesting.remove(&host);
                        c.sessions.insert(
                            host,
                            Session {
                                activity,
                                pending: Vec::new(),
                            },
                        );
                    }
                    let hello = WireInput::from(Msg::Hello(Hello {}));
                    if task_ctx
                        .send(host, Direction::ToHost, activity, &hello)
                        .is_err()
                    {
                        lock(&shared.ctrl).sessions.remove(&host);
                        notify_refused(&shared, host, "offline");
                        return;
                    }
                    if let Some(e) = shared.events.get() {
                        e.session(host, true);
                    }
                    flush_loop(shared, task_ctx, host, activity).await;
                }
                Err(e) => {
                    lock(&shared.ctrl).requesting.remove(&host);
                    let why = match e {
                        CoreError::Refused(why) => why,
                        _ => "offline".into(),
                    };
                    notify_refused(&shared, host, &why);
                }
            }
        });
        Ok(())
    }

    /// True while a session with `host` is running.
    pub fn is_active(&self, host: DeviceId) -> bool {
        lock(&self.shared.ctrl).sessions.contains_key(&host)
    }

    /// True while a session with `host` is being requested.
    pub fn is_connecting(&self, host: DeviceId) -> bool {
        lock(&self.shared.ctrl).requesting.contains(&host)
    }

    /// End the session with `host`: hides the laser, lets go of keys and
    /// buttons on the host, and ends the activity on both devices.
    pub fn end(&self, host: DeviceId) -> Result<()> {
        let ctx = self.ctx()?;
        let Some(session) = lock(&self.shared.ctrl).sessions.remove(&host) else {
            return Ok(());
        };
        // The host releases everything held when the activity ends; the laser
        // goes first so the dot never outlives the button.
        let off = WireInput::from(Msg::Batch(Batch {
            events: vec![Event::from(Kind::Laser(wire::Laser {
                x: 0,
                y: 0,
                on: false,
            }))],
        }));
        let _ = ctx.send(host, Direction::ToHost, session.activity, &off);
        ctx.stop(host, session.activity);
        if let Some(e) = self.shared.events.get() {
            e.session(host, false);
        }
        Ok(())
    }

    /// Move the host's pointer by `dx`, `dy` pixels (R9.1).
    pub fn pointer_move(&self, host: DeviceId, dx: i32, dy: i32) -> Result<()> {
        self.queue(
            host,
            Kind::Move(wire::Move {
                dx: dx.clamp(-MAX_DELTA, MAX_DELTA),
                dy: dy.clamp(-MAX_DELTA, MAX_DELTA),
            }),
        )
    }

    /// Put the host's pointer at a place on one of its displays.
    pub fn pointer_abs(&self, host: DeviceId, x: u32, y: u32, display: u32) -> Result<()> {
        self.queue(host, Kind::Abs(wire::Abs { x, y, display }))
    }

    /// Press or release a mouse button (one of [`button`]).
    pub fn button(&self, host: DeviceId, button: u32, down: bool) -> Result<()> {
        self.queue(host, Kind::Button(wire::Button { button, down }))
    }

    /// Turn the host's wheel, in [`SCROLL_NOTCH`] units (down and right positive).
    pub fn scroll(&self, host: DeviceId, dx: i32, dy: i32) -> Result<()> {
        self.queue(
            host,
            Kind::Scroll(wire::Scroll {
                dx: dx.clamp(-MAX_DELTA, MAX_DELTA),
                dy: dy.clamp(-MAX_DELTA, MAX_DELTA),
            }),
        )
    }

    /// Press or release a key. `usage` is a USB HID usage (page 0x07);
    /// `modifiers` are [`modifier`] bits the host holds around the key.
    pub fn key(&self, host: DeviceId, usage: u32, down: bool, modifiers: u32) -> Result<()> {
        if !wire::is_valid_usage(usage) {
            return Err(CoreError::InvalidInput("unknown key".into()));
        }
        self.queue(
            host,
            Kind::Key(wire::Key {
                usage,
                down,
                modifiers: modifiers & modifier::ALL,
            }),
        )
    }

    /// Press and release a key, with modifiers (a shortcut, a special key).
    pub fn tap_key(&self, host: DeviceId, usage: u32, modifiers: u32) -> Result<()> {
        self.key(host, usage, true, modifiers)?;
        self.key(host, usage, false, modifiers)
    }

    /// Type Unicode text on the host (R10.4). Control characters other than
    /// newline and tab are dropped; long text is split.
    pub fn text(&self, host: DeviceId, text: &str) -> Result<()> {
        let clean: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect();
        for chunk in split_text(&clean, MAX_TEXT_BYTES) {
            self.queue(
                host,
                Kind::Text(wire::Text {
                    text: chunk.to_owned(),
                }),
            )?;
        }
        Ok(())
    }

    /// Run a system navigation action on the host: one of
    /// [`nav`](urdevice_proto::features::input::nav) (Back, Home, Recents).
    /// Android hosts only; a desktop host ignores it (R9.3).
    pub fn nav(&self, host: DeviceId, action: u32) -> Result<()> {
        if action >= wire::nav::COUNT {
            return Err(CoreError::InvalidInput("unknown navigation action".into()));
        }
        self.queue(host, Kind::Nav(wire::Nav { action }))
    }

    /// Show the presenter's laser at `x`, `y` (0.0 to 1.0 across and down the
    /// host's primary display), or hide it with `on = false`.
    pub fn laser(&self, host: DeviceId, x: f32, y: f32, on: bool) -> Result<()> {
        let to = |v: f32| (v.clamp(0.0, 1.0) * COORD_MAX as f32).round() as u32;
        self.queue(
            host,
            Kind::Laser(wire::Laser {
                x: to(x),
                y: to(y),
                on,
            }),
        )
    }

    /// Queue one event for `host`. Lossy events wait for the next flush;
    /// the others go out now, after what is already waiting.
    fn queue(&self, host: DeviceId, kind: Kind) -> Result<()> {
        let ctx = self.ctx()?;
        let event = Event::from(kind);
        let mut c = lock(&self.shared.ctrl);
        let session = c
            .sessions
            .get_mut(&host)
            .ok_or_else(|| CoreError::InvalidInput("no remote-control session".into()))?;
        if event.is_lossy() {
            coalesce(&mut session.pending, event);
            return Ok(());
        }
        let activity = session.activity;
        let pending = std::mem::take(&mut session.pending);
        // Keep the order: the moves before a click are sent before it, and
        // the lock is held so a flush cannot slip in between.
        send_events(ctx, host, activity, pending);
        send_reliable(ctx, host, activity, vec![event]);
        Ok(())
    }
}

/// Add a lossy event, merging it into the one before where that loses nothing.
fn coalesce(pending: &mut Vec<Event>, event: Event) {
    match (
        pending.last_mut().and_then(|e| e.kind.as_mut()),
        &event.kind,
    ) {
        (Some(Kind::Move(a)), Some(Kind::Move(b))) => {
            a.dx = a.dx.saturating_add(b.dx).clamp(-MAX_DELTA, MAX_DELTA);
            a.dy = a.dy.saturating_add(b.dy).clamp(-MAX_DELTA, MAX_DELTA);
        }
        (Some(Kind::Scroll(a)), Some(Kind::Scroll(b))) => {
            a.dx = a.dx.saturating_add(b.dx).clamp(-MAX_DELTA, MAX_DELTA);
            a.dy = a.dy.saturating_add(b.dy).clamp(-MAX_DELTA, MAX_DELTA);
        }
        (Some(Kind::Laser(a)), Some(Kind::Laser(b))) => *a = b.clone(),
        (Some(Kind::Abs(a)), Some(Kind::Abs(b))) => *a = b.clone(),
        _ => {
            // A runaway queue (no flush for a long time) keeps the newest.
            if pending.len() >= MAX_EVENTS * 8 {
                pending.remove(0);
            }
            pending.push(event);
        }
    }
}

/// `text` in pieces of at most `max` bytes, cut at character boundaries.
fn split_text(text: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if rest.len() <= max {
            out.push(rest);
            break;
        }
        let mut end = max;
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (a, b) = rest.split_at(end);
        out.push(a);
        rest = b;
    }
    out
}

/// Send lossy events: datagrams when the path takes them, else the control
/// stream.
fn send_events(ctx: &ServiceCtx, host: DeviceId, activity: u64, events: Vec<Event>) {
    for chunk in events.chunks(MAX_EVENTS / 2) {
        let batch = Batch {
            events: chunk.to_vec(),
        };
        let bytes = prost::Message::encode_to_vec(&batch);
        let fits = ctx
            .max_datagram_payload(host)
            .is_some_and(|max| bytes.len() <= max);
        if fits
            && ctx
                .send_datagram(host, Direction::ToHost, activity, &bytes)
                .is_ok()
        {
            continue;
        }
        send_reliable(ctx, host, activity, chunk.to_vec());
    }
}

/// Send events on the control stream, which is ordered and not lossy.
fn send_reliable(ctx: &ServiceCtx, host: DeviceId, activity: u64, events: Vec<Event>) {
    if events.is_empty() {
        return;
    }
    let m = WireInput::from(Msg::Batch(Batch { events }));
    if let Err(e) = ctx.send(host, Direction::ToHost, activity, &m) {
        tracing::debug!(error = %e, "input events not sent");
    }
}

/// Send what the user did, every [`FLUSH_EVERY`], until the session ends.
async fn flush_loop(shared: Arc<Shared>, ctx: ServiceCtx, host: DeviceId, activity: u64) {
    let mut tick = tokio::time::interval(FLUSH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let mut c = lock(&shared.ctrl);
        let Some(s) = c.sessions.get_mut(&host).filter(|s| s.activity == activity) else {
            return;
        };
        if s.pending.is_empty() {
            continue;
        }
        let pending = std::mem::take(&mut s.pending);
        // Held across the send so a click queued now goes out after these.
        send_events(&ctx, host, activity, pending);
    }
}

fn notify_refused(shared: &Shared, peer: DeviceId, reason: &str) {
    if let Some(e) = shared.events.get() {
        e.refused(peer, reason);
    }
}

// ---------------------------------------------------------------------------
// Host side: the worker thread.
// ---------------------------------------------------------------------------

/// The expansions of [`modifier`] bits into the left-hand modifier keys.
const MODIFIER_KEYS: [(u32, u32); 4] = [
    (modifier::CTRL, usage::LEFT_CTRL),
    (modifier::SHIFT, usage::LEFT_SHIFT),
    (modifier::ALT, usage::LEFT_ALT),
    (modifier::META, usage::LEFT_META),
];

/// True for a modifier key (left or right).
fn is_modifier_key(u: u32) -> bool {
    (usage::LEFT_CTRL..=usage::RIGHT_META).contains(&u)
}

/// The group a modifier key belongs to: `usage` and its left or right twin.
fn modifier_twin(u: u32) -> u32 {
    if u >= usage::RIGHT_CTRL { u - 4 } else { u + 4 }
}

fn worker(shared: &Weak<Shared>, rx: &mpsc::Receiver<Job>) {
    while let Ok(job) = rx.recv() {
        let Some(shared) = shared.upgrade() else {
            return;
        };
        match job {
            Job::Events { grant, events } => apply_events(&shared, &grant, events),
            Job::Hello { grant } => hello(&shared, &grant),
            Job::Release { peer, activity } => release(&shared, peer, activity),
        }
    }
}

fn hello(shared: &Shared, grant: &Grant) {
    let Some(ctx) = shared.ctx.get() else { return };
    let (status, backend, text, size) = match shared.sink.get() {
        Some(sink) => (
            sink.prepare(grant),
            sink.name(),
            sink.text_status(),
            sink.display_size(),
        ),
        None => (PermStatus::Unavailable, "", PermStatus::Ready, None),
    };
    let text_missing = match (status, text) {
        // A host that is not ready at all says so with `status`.
        (PermStatus::Ready, PermStatus::Missing(m)) => Some(m),
        _ => None,
    };
    let (state, permission) = match status {
        PermStatus::Ready => (wire::state::READY, ""),
        PermStatus::Missing(m) => (wire::state::MISSING_PERMISSION, m.as_str()),
        PermStatus::Unavailable => (wire::state::UNAVAILABLE, ""),
    };
    let m = WireInput::from(Msg::Status(WireStatus {
        state,
        permission: permission.into(),
        backend: backend.into(),
        width: size.map_or(0, |s| s.0),
        height: size.map_or(0, |s| s.1),
        text_permission: text_missing.map_or("", Missing::as_str).into(),
    }));
    let _ = ctx.send(grant.peer(), Direction::ToController, grant.activity(), &m);
    if let Some(e) = shared.events.get() {
        if let PermStatus::Missing(missing) = status {
            e.missing_permission(grant.peer(), missing);
        }
        if let Some(missing) = text_missing {
            e.missing_permission(grant.peer(), missing);
        }
    }
}

fn apply_events(shared: &Shared, grant: &Grant, events: Vec<Event>) {
    let sink = shared.sink.get();
    for event in events {
        let Some(kind) = event.kind else { continue };
        let key = (grant.peer(), grant.activity());
        match kind {
            Kind::Move(m) => {
                pointer(&sink, grant, PointerEvent::Move { dx: m.dx, dy: m.dy });
            }
            Kind::Abs(a) => {
                pointer(
                    &sink,
                    grant,
                    PointerEvent::Abs {
                        x: a.x,
                        y: a.y,
                        display: a.display,
                    },
                );
            }
            Kind::Scroll(s) => {
                pointer(&sink, grant, PointerEvent::Scroll { dx: s.dx, dy: s.dy });
            }
            Kind::Button(b) => {
                {
                    let mut held = lock(&shared.held);
                    let h = held.entry(key).or_insert_with(|| Held::new(grant));
                    if b.down {
                        h.buttons.insert(b.button);
                    } else {
                        h.buttons.remove(&b.button);
                    }
                }
                pointer(
                    &sink,
                    grant,
                    PointerEvent::Button {
                        button: b.button,
                        down: b.down,
                    },
                );
            }
            Kind::Key(k) => {
                let steps = {
                    let mut held = lock(&shared.held);
                    let h = held.entry(key).or_insert_with(|| Held::new(grant));
                    key_steps(h, k.usage, k.down, k.modifiers)
                };
                for (u, down) in steps {
                    if let Some(s) = &sink
                        && let Err(e) = s.key(grant, &KeyEvent { usage: u, down })
                    {
                        tracing::debug!(error = %e, "key not injected");
                    }
                }
            }
            Kind::Text(t) => {
                if let Some(s) = &sink
                    && let Err(e) = s.text(grant, &t.text)
                {
                    tracing::debug!(error = %e, len = t.text.len(), "text not injected");
                }
            }
            Kind::Nav(n) => {
                if let Some(s) = &sink
                    && let Err(e) = s.nav(grant, n.action)
                {
                    tracing::debug!(error = %e, "navigation action not run");
                }
            }
            Kind::Laser(l) => {
                lock(&shared.held)
                    .entry(key)
                    .or_insert_with(|| Held::new(grant))
                    .laser = l.on;
                if let Some(o) = shared.overlay.get() {
                    let pos = l
                        .on
                        .then(|| (l.x as f32 / COORD_MAX as f32, l.y as f32 / COORD_MAX as f32));
                    if let Err(e) = o.laser(grant, pos) {
                        tracing::debug!(error = %e, "laser not shown");
                    }
                }
            }
        }
    }
}

fn pointer(sink: &Option<Arc<dyn InputSink>>, grant: &Grant, e: PointerEvent) {
    if let Some(s) = sink
        && let Err(err) = s.pointer(grant, &e)
    {
        tracing::debug!(error = %err, "pointer event not injected");
    }
}

/// The plain key events that carry out `Key { usage, down, modifiers }`, and
/// the held state afterwards. Modifiers go down before the key and come up
/// after it, but only the ones this key put down.
fn key_steps(h: &mut Held, usage: u32, down: bool, modifiers: u32) -> Vec<(u32, bool)> {
    let mut steps = Vec::new();
    if down {
        let mut implied = Vec::new();
        if !is_modifier_key(usage) {
            for (bit, key) in MODIFIER_KEYS {
                if modifiers & bit != 0
                    && !h.keys.contains(&key)
                    && !h.keys.contains(&modifier_twin(key))
                {
                    h.keys.insert(key);
                    steps.push((key, true));
                    implied.push(key);
                }
            }
        }
        if !implied.is_empty() {
            h.implied.entry(usage).or_default().extend(implied);
        }
        h.keys.insert(usage);
        steps.push((usage, true));
    } else {
        if h.keys.remove(&usage) {
            steps.push((usage, false));
        }
        for m in h.implied.remove(&usage).unwrap_or_default() {
            if h.keys.remove(&m) {
                steps.push((m, false));
            }
        }
    }
    steps
}

/// Let go of what an activity (or a whole peer) holds.
fn release(shared: &Shared, peer: DeviceId, activity: Option<u64>) {
    let taken: Vec<Held> = {
        let mut held = lock(&shared.held);
        let keys: Vec<_> = held
            .keys()
            .filter(|(p, a)| *p == peer && activity.is_none_or(|x| x == *a))
            .copied()
            .collect();
        keys.iter().filter_map(|k| held.remove(k)).collect()
    };
    if taken.is_empty() {
        return;
    }
    let sink = shared.sink.get();
    for h in taken {
        if let Some(s) = &sink {
            s.cancel(&h.grant);
            for k in h.keys.iter().rev() {
                let _ = s.key(
                    &h.grant,
                    &KeyEvent {
                        usage: *k,
                        down: false,
                    },
                );
            }
            for b in &h.buttons {
                let _ = s.pointer(
                    &h.grant,
                    &PointerEvent::Button {
                        button: *b,
                        down: false,
                    },
                );
            }
        }
        if h.laser
            && let Some(o) = shared.overlay.get()
        {
            let _ = o.laser(&h.grant, None);
        }
    }
}

impl Shared {
    /// Take `n` events from a peer's allowance, or none when it is spent.
    fn allow(&self, peer: DeviceId, n: usize) -> bool {
        let now = Instant::now();
        let mut b = lock(&self.buckets);
        let bucket = b.entry(peer).or_insert(Bucket {
            tokens: EVENT_BURST,
            at: now,
        });
        let gained = now.duration_since(bucket.at).as_secs_f64() * EVENTS_PER_SEC;
        bucket.tokens = (bucket.tokens + gained).min(EVENT_BURST);
        bucket.at = now;
        let n = n as f64;
        if bucket.tokens >= n {
            bucket.tokens -= n;
            true
        } else {
            false
        }
    }

    /// Queue a job; a full queue drops it (a flooding controller loses events).
    fn enqueue(&self, job: Job) {
        match self.jobs.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => tracing::warn!("input queue full; events dropped"),
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    fn host_batch(&self, grant: Grant, batch: &Batch) {
        // Watched capability: the node insists on an activity, and so do we.
        if grant.activity() == 0 {
            return;
        }
        let events = batch.sanitize();
        if events.is_empty() || !self.allow(grant.peer(), events.len()) {
            return;
        }
        self.enqueue(Job::Events { grant, events });
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

impl Service for Input {
    fn id(&self) -> ServiceId {
        ServiceId::INPUT
    }

    fn capability(&self) -> Capability {
        Capability::Input
    }

    fn start(&self, ctx: ServiceCtx) {
        if self.shared.ctx.set(ctx).is_ok()
            && let Some(rx) = lock(&self.shared.queue).take()
        {
            let weak = Arc::downgrade(&self.shared);
            let spawned = std::thread::Builder::new()
                .name("urdevice-input".into())
                .spawn(move || worker(&weak, &rx));
            if let Err(e) = spawned {
                tracing::error!(error = %e, "input worker did not start");
            }
        }
    }

    fn on_message(&self, _ctx: &ServiceCtx, msg: Message) -> Result<()> {
        // Untrusted: a body that does not decode closes the session.
        let m: WireInput = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad input message".into()))?;
        let Some(inner) = m.msg else { return Ok(()) };
        match (msg.access, inner) {
            (Access::Host(grant), Msg::Batch(b)) => self.shared.host_batch(grant, &b),
            (Access::Host(grant), Msg::Hello(_)) if grant.activity() != 0 => {
                self.shared.enqueue(Job::Hello { grant });
            }
            (Access::Controller { .. }, Msg::Status(s)) => self.on_status(msg.peer, s),
            // Status sent to a host, or events sent to a controller: never act.
            _ => {}
        }
        Ok(())
    }

    fn on_datagram(&self, _ctx: &ServiceCtx, d: Datagram) {
        let Access::Host(grant) = d.access else {
            return;
        };
        if let Ok(batch) = <Batch as prost::Message>::decode(d.payload.as_slice()) {
            self.shared.host_batch(grant, &batch);
        }
    }

    fn on_refused(&self, _ctx: &ServiceCtx, peer: DeviceId, _activity: u64, reason: &str) {
        let ended = lock(&self.shared.ctrl).sessions.remove(&peer).is_some();
        if ended && let Some(e) = self.shared.events.get() {
            e.session(peer, false);
        }
        notify_refused(&self.shared, peer, reason);
    }

    fn on_activity_end(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64) {
        self.shared.enqueue(Job::Release {
            peer,
            activity: Some(activity),
        });
        let ended = {
            let mut c = lock(&self.shared.ctrl);
            if c.sessions
                .get(&peer)
                .is_some_and(|s| s.activity == activity)
            {
                c.sessions.remove(&peer);
                true
            } else {
                false
            }
        };
        if ended && let Some(e) = self.shared.events.get() {
            e.session(peer, false);
        }
    }

    fn on_peer(&self, _ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        if connected {
            return;
        }
        lock(&self.shared.buckets).remove(&peer);
        self.shared.enqueue(Job::Release {
            peer,
            activity: None,
        });
        let ended = {
            let mut c = lock(&self.shared.ctrl);
            c.requesting.remove(&peer);
            c.sessions.remove(&peer).is_some()
        };
        if ended && let Some(e) = self.shared.events.get() {
            e.session(peer, false);
        }
    }
}

impl Input {
    fn on_status(&self, peer: DeviceId, s: WireStatus) {
        // Only a host this device controls may tell it what works.
        if !lock(&self.shared.ctrl).sessions.contains_key(&peer) {
            return;
        }
        let s = s.sanitize();
        let status = match s.state {
            wire::state::READY => PermStatus::Ready,
            wire::state::MISSING_PERMISSION => {
                Missing::parse(&s.permission).map_or(PermStatus::Unavailable, PermStatus::Missing)
            }
            _ => PermStatus::Unavailable,
        };
        if let Some(e) = self.shared.events.get() {
            let display = (s.width > 0 && s.height > 0).then_some((s.width, s.height));
            e.host_status(
                peer,
                &HostStatus {
                    status,
                    backend: s.backend,
                    text_missing: Missing::parse(&s.text_permission),
                    display,
                },
            );
        }
    }
}

pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Arc::new(Input::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held() -> Held {
        let grant = Grant {
            peer: DeviceId::from_hex(&"ab".repeat(32)).unwrap(),
            capability: Capability::Input,
            activity: 7,
        };
        Held::new(&grant)
    }

    #[test]
    fn a_shortcut_presses_modifiers_around_the_key() {
        let mut h = held();
        let down = key_steps(&mut h, usage::A + 2, true, modifier::CTRL | modifier::SHIFT);
        assert_eq!(
            down,
            vec![
                (usage::LEFT_CTRL, true),
                (usage::LEFT_SHIFT, true),
                (usage::A + 2, true)
            ]
        );
        let up = key_steps(
            &mut h,
            usage::A + 2,
            false,
            modifier::CTRL | modifier::SHIFT,
        );
        assert_eq!(
            up,
            vec![
                (usage::A + 2, false),
                (usage::LEFT_CTRL, false),
                (usage::LEFT_SHIFT, false)
            ]
        );
        assert!(h.keys.is_empty() && h.implied.is_empty());
    }

    #[test]
    fn a_held_modifier_is_not_pressed_twice_nor_released_early() {
        let mut h = held();
        key_steps(&mut h, usage::LEFT_CTRL, true, 0);
        let down = key_steps(&mut h, usage::A, true, modifier::CTRL);
        assert_eq!(down, vec![(usage::A, true)]);
        let up = key_steps(&mut h, usage::A, false, modifier::CTRL);
        assert_eq!(up, vec![(usage::A, false)]);
        assert!(h.keys.contains(&usage::LEFT_CTRL));
        // The right Control key counts as Control too.
        let mut h = held();
        key_steps(&mut h, usage::RIGHT_CTRL, true, 0);
        assert_eq!(
            key_steps(&mut h, usage::B, true, modifier::CTRL),
            vec![(usage::B, true)]
        );
    }

    #[test]
    fn releasing_a_key_that_is_not_held_does_nothing() {
        let mut h = held();
        assert!(key_steps(&mut h, usage::A, false, 0).is_empty());
    }

    #[test]
    fn moves_and_scrolls_merge_and_the_last_laser_wins() {
        let mv = |dx, dy| Event::from(Kind::Move(wire::Move { dx, dy }));
        let mut p = Vec::new();
        coalesce(&mut p, mv(1, 2));
        coalesce(&mut p, mv(3, -5));
        assert_eq!(p, vec![mv(4, -3)]);
        coalesce(
            &mut p,
            Event::from(Kind::Scroll(wire::Scroll { dx: 0, dy: 10 })),
        );
        coalesce(&mut p, mv(1, 1));
        assert_eq!(p.len(), 3);
        let laser = |x| Event::from(Kind::Laser(wire::Laser { x, y: 0, on: true }));
        coalesce(&mut p, laser(1));
        coalesce(&mut p, laser(2));
        assert_eq!(p.len(), 4);
        assert_eq!(p[3], laser(2));
    }

    #[test]
    fn a_runaway_queue_stays_bounded() {
        let mut p = Vec::new();
        for i in 0..MAX_EVENTS * 20 {
            let kind = if i % 2 == 0 {
                Kind::Move(wire::Move { dx: 1, dy: 1 })
            } else {
                Kind::Scroll(wire::Scroll { dx: 0, dy: 1 })
            };
            coalesce(&mut p, Event::from(kind));
        }
        assert!(p.len() <= MAX_EVENTS * 8);
    }

    #[test]
    fn text_splits_at_character_boundaries() {
        let s = "é".repeat(10);
        for max in [3, 4, 5, 20] {
            let parts = split_text(&s, max);
            assert!(parts.iter().all(|p| p.len() <= max));
            assert_eq!(parts.concat(), s);
        }
        assert!(split_text("", 4).is_empty());
    }

    #[test]
    fn the_rate_limit_stops_a_flood() {
        let shared = Shared::default();
        let peer = DeviceId::from_hex(&"cd".repeat(32)).unwrap();
        assert!(shared.allow(peer, 1000));
        assert!(!shared.allow(peer, 1000));
        assert!(shared.allow(peer, 100));
    }

    #[test]
    fn missing_permissions_round_trip_their_names() {
        for m in [Missing::Accessibility, Missing::Uinput, Missing::Portal] {
            assert_eq!(Missing::parse(m.as_str()), Some(m));
        }
        assert_eq!(Missing::parse("x"), None);
    }
}
