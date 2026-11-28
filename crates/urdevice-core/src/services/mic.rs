//! Phone as microphone (#28, T11.5, design §18.1, R17.4).
//!
//! Service id: [`ServiceId::MIC`], capability `mic.v1` (Release 2, default Always Ask, watched:
//! every session is an activity, so the host shows the indicator and Stop, R5.4). Only Android
//! hosts offer it (`Capability::android_only`).
//!
//! **Listener (controller).** [`Mic::request_listen`] asks the host for an activity (the host
//! allows, prompts, or refuses) and then sends [`Start`](wire::Start). The host opens one
//! unidirectional audio stream; the service reads it with size limits and hands each PCM chunk to
//! [`MicEvents::chunk`]. The UI plays it (desktop: a virtual microphone) and never acts on it.
//! [`Mic::stop`] ends the activity.
//!
//! **Host.** A `Start` that passed the permission check reaches the [`MicHost`] with its
//! [`Grant`]. The adapter records (Android: `AudioRecord` in a microphone foreground service) and
//! pushes 16-bit PCM chunks into a [`MicSink`]. The sink keeps at most [`MAX_QUEUED_CHUNKS`]
//! waiting for the network: a chunk that does not fit is dropped, a short glitch instead of a
//! growing delay.
//!
//! Audio is never stored or logged; tracing carries ids and sizes only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use tokio::sync::{Notify, mpsc};
use urdevice_proto::features::webcam::{
    self as wire, AudioInfo, CHUNK_PREFIX, Mic as WireMic, MicError, STREAM_AUDIO, mic::Msg,
};

use super::{
    Access, Direction, Grant, Message, Registry, Service, ServiceCtx, ServiceId, Slot, Stream,
    stream_code,
};
use crate::identity::DeviceId;
use crate::permission::Capability;
use crate::{CoreError, Result};

/// Most chunks of audio waiting for the network per session (about 10 x 20 ms).
pub const MAX_QUEUED_CHUNKS: usize = 10;

/// Most audio sessions a host runs at once, over all listeners.
pub const MAX_HOST_SESSIONS: usize = 2;

/// Where a host adapter puts recorded audio. Made by the service.
pub trait MicSink: Send + Sync {
    /// False while the network is behind: the adapter may skip this chunk.
    fn ready(&self) -> bool;
    /// Hand over one chunk of 16-bit little-endian PCM at [`wire::SAMPLE_RATE`], mono. Never
    /// blocks; drops when full. A chunk that is empty, odd, or too long is ignored.
    fn push(&self, pcm: Vec<u8>);
    /// Recording stopped on its own (the OS revoked the permission). `reason` is
    /// `capture_failed` or `stopped`.
    fn failed(&self, reason: &str);
}

/// A running recording, returned by [`MicHost::start`].
pub trait MicCapture: Send + Sync {
    /// Stop recording and release the microphone. Called once.
    fn stop(&self);
}

/// What the host OS provides. Android: Kotlin through the FFI. The method may block (a permission
/// dialog); the service calls it on the blocking pool.
pub trait MicHost: Send + Sync {
    /// Start recording. `grant` is proof the permission engine allowed this peer; an adapter never
    /// records without one. Errors use [`CoreError::Refused`] with `consent_declined` or
    /// `capture_failed`.
    fn start(&self, grant: &Grant, sink: Arc<dyn MicSink>) -> Result<Arc<dyn MicCapture>>;
}

/// What the listener UI hears. Called from core threads; hand work to the UI thread. Never act on
/// the device because of these.
pub trait MicEvents: Send + Sync {
    /// [`Mic::request_listen`] got an activity: the host allowed it.
    fn opened(&self, peer: DeviceId, activity: u64);
    /// An audio stream began: reset the player.
    fn stream_started(&self, peer: DeviceId, activity: u64);
    /// One chunk of 16-bit little-endian PCM, mono, at [`wire::SAMPLE_RATE`].
    fn chunk(&self, peer: DeviceId, activity: u64, pcm: Vec<u8>);
    /// The host reported an error, or refused a message. Activity 0 is a refused request.
    fn error(&self, peer: DeviceId, activity: u64, reason: &str);
    /// The session ended (Stop on either device, Deny, disconnect).
    fn ended(&self, peer: DeviceId, activity: u64);
}

// ---------------------------------------------------------------------------
// Host side
// ---------------------------------------------------------------------------

struct Sink {
    tx: mpsc::Sender<Vec<u8>>,
    closed: AtomicBool,
    dropped: AtomicU64,
    /// Wakes the session task: recording failed.
    kick: Notify,
    failure: Mutex<Option<String>>,
}

impl MicSink for Sink {
    fn ready(&self) -> bool {
        !self.closed.load(Ordering::Relaxed) && self.tx.capacity() > 0
    }

    fn push(&self, pcm: Vec<u8>) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        if wire::chunk_len(u32::try_from(pcm.len()).unwrap_or(u32::MAX).to_be_bytes()).is_err() {
            return;
        }
        if self.tx.try_send(pcm).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn failed(&self, reason: &str) {
        let reason = if reason == "stopped" {
            "stopped"
        } else {
            "capture_failed"
        };
        *lock(&self.failure) = Some(reason.into());
        self.kick.notify_one();
    }
}

struct HostSession {
    /// Distinguishes a new `Start` from the session it replaced.
    generation: u64,
    stop: Arc<Notify>,
}

#[derive(Default)]
struct State {
    host: HashMap<(DeviceId, u64), HostSession>,
    next_generation: u64,
    /// Sessions this device listens to, with the generation of their newest stream.
    listening: HashMap<(DeviceId, u64), u64>,
}

/// The microphone service. UI code reaches it with `node.service::<Mic>()`.
#[derive(Default)]
pub struct Mic {
    ctx: OnceLock<ServiceCtx>,
    /// The recording adapter, on devices that can be listened to.
    pub host: Slot<dyn MicHost>,
    /// The listener UI.
    pub events: Slot<dyn MicEvents>,
    state: Arc<Mutex<State>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Mic {
    fn ctx(&self) -> Result<&ServiceCtx> {
        self.ctx.get().ok_or(CoreError::Stopped)
    }

    fn events(&self) -> Option<Arc<dyn MicEvents>> {
        self.events.get()
    }

    /// Ask `peer` for its microphone. The answer arrives in [`MicEvents::opened`] (the stream
    /// starts at once) or in [`MicEvents::error`] with activity 0 and `denied`, `declined`,
    /// `timeout`, `unsupported`, or `offline`.
    pub fn request_listen(self: &Arc<Self>, peer: DeviceId) -> Result<()> {
        let ctx = self.ctx()?.clone();
        let this = self.clone();
        ctx.clone().spawn(async move {
            let r = ctx.request(peer).await;
            let Some(e) = this.events() else { return };
            match r {
                Ok(activity) => {
                    lock(&this.state).listening.insert((peer, activity), 0);
                    e.opened(peer, activity);
                    let _ = ctx.send(
                        peer,
                        Direction::ToHost,
                        activity,
                        &WireMic::from(Msg::Start(wire::Start {})),
                    );
                }
                Err(CoreError::Refused(reason)) => e.error(peer, 0, &reason),
                Err(_) => e.error(peer, 0, "offline"),
            }
        });
        Ok(())
    }

    /// End the session on both devices.
    pub fn stop(&self, peer: DeviceId, activity: u64) {
        if let Ok(ctx) = self.ctx() {
            ctx.stop(peer, activity);
        }
    }

    /// True while this device records for anyone.
    pub fn is_recording(&self) -> bool {
        !lock(&self.state).host.is_empty()
    }

    fn reply_error(ctx: &ServiceCtx, peer: DeviceId, activity: u64, reason: &str) {
        let msg = WireMic::from(Msg::Error(MicError {
            reason: reason.into(),
        }));
        let _ = ctx.send(peer, Direction::ToController, activity, &msg);
    }

    fn host_start(&self, ctx: &ServiceCtx, grant: Grant) {
        let (peer, activity) = (grant.peer(), grant.activity());
        let Some(host) = self.host.get() else {
            Self::reply_error(ctx, peer, activity, "no_host");
            return;
        };
        let (generation, stop) = {
            let mut st = lock(&self.state);
            if let Some(old) = st.host.remove(&(peer, activity)) {
                old.stop.notify_one();
            }
            if st.host.len() >= MAX_HOST_SESSIONS {
                drop(st);
                Self::reply_error(ctx, peer, activity, "capture_failed");
                return;
            }
            st.next_generation += 1;
            let generation = st.next_generation;
            let stop = Arc::new(Notify::new());
            st.host.insert(
                (peer, activity),
                HostSession {
                    generation,
                    stop: stop.clone(),
                },
            );
            (generation, stop)
        };
        tracing::info!(activity, "microphone session starting");
        ctx.spawn(run_host_session(
            ctx.clone(),
            self.state.clone(),
            host,
            grant,
            generation,
            stop,
        ));
    }

    fn viewer_stream(&self, ctx: &ServiceCtx, mut stream: Stream, activity: u64) {
        let peer = stream.peer;
        // The one format this device plays; anything else is refused below.
        let standard = prost::Message::decode(stream.meta.as_slice())
            .is_ok_and(|i: AudioInfo| i.is_standard());
        let generation = {
            let mut st = lock(&self.state);
            match st.listening.get_mut(&(peer, activity)) {
                Some(g) if stream.kind == STREAM_AUDIO && standard => {
                    *g += 1;
                    Some(*g)
                }
                _ => None,
            }
        };
        let Some(generation) = generation else {
            // Not a stream this device asked for, or a format it cannot play.
            let _ = stream.recv.stop(stream_code::UNSUPPORTED.into());
            return;
        };
        let events = self.events();
        if let Some(e) = &events {
            e.stream_started(peer, activity);
        }
        let state = self.state.clone();
        ctx.spawn(async move {
            let mut recv = stream.recv;
            loop {
                let chunk = match read_chunk(&mut recv).await {
                    Ok(Some(c)) => c,
                    Ok(None) => break,
                    Err(e) => {
                        tracing::debug!(activity, error = %e, "audio stream ended");
                        let _ = recv.stop(stream_code::CANCELLED.into());
                        break;
                    }
                };
                let current = lock(&state).listening.get(&(peer, activity)) == Some(&generation);
                if !current {
                    let _ = recv.stop(stream_code::CANCELLED.into());
                    break;
                }
                if let Some(e) = &events {
                    e.chunk(peer, activity, chunk);
                }
            }
        });
    }
}

/// Read one length-prefixed chunk. `Ok(None)` at a clean end of stream.
async fn read_chunk(recv: &mut quinn::RecvStream) -> Result<Option<Vec<u8>>> {
    let mut prefix = [0u8; CHUNK_PREFIX];
    match recv.read_exact(&mut prefix).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(CoreError::Network(e.to_string())),
    }
    let len = wire::chunk_len(prefix).map_err(|e| CoreError::Protocol(e.to_string()))?;
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body)
        .await
        .map_err(|e| CoreError::Network(e.to_string()))?;
    Ok(Some(body))
}

fn remove_if_current(state: &Mutex<State>, peer: DeviceId, activity: u64, generation: u64) {
    let mut st = lock(state);
    if st
        .host
        .get(&(peer, activity))
        .is_some_and(|s| s.generation == generation)
    {
        st.host.remove(&(peer, activity));
    }
}

/// One host session: open the stream, start recording, write chunks, and clean up on Stop or any
/// failure.
async fn run_host_session(
    ctx: ServiceCtx,
    state: Arc<Mutex<State>>,
    host: Arc<dyn MicHost>,
    grant: Grant,
    generation: u64,
    stop: Arc<Notify>,
) {
    let (peer, activity) = (grant.peer(), grant.activity());
    let current = |st: &State| {
        st.host
            .get(&(peer, activity))
            .is_some_and(|s| s.generation == generation)
    };
    let meta = prost::Message::encode_to_vec(&AudioInfo::standard());
    let mut send = match ctx
        .open_uni(peer, Direction::ToController, activity, STREAM_AUDIO, meta)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(activity, error = %e, "audio stream did not open");
            remove_if_current(&state, peer, activity, generation);
            return;
        }
    };
    let (tx, mut rx) = mpsc::channel(MAX_QUEUED_CHUNKS);
    let sink = Arc::new(Sink {
        tx,
        closed: AtomicBool::new(false),
        dropped: AtomicU64::new(0),
        kick: Notify::new(),
        failure: Mutex::new(None),
    });
    let started = {
        let (host, grant, sink) = (host.clone(), grant.clone(), sink.clone());
        tokio::task::spawn_blocking(move || host.start(&grant, sink)).await
    };
    let capture = match started {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            let reason = match e {
                CoreError::Refused(r) => r,
                _ => "capture_failed".into(),
            };
            tracing::info!(activity, reason = %reason, "microphone did not start");
            Mic::reply_error(&ctx, peer, activity, &reason);
            let _ = send.finish();
            remove_if_current(&state, peer, activity, generation);
            return;
        }
        Err(_) => {
            Mic::reply_error(&ctx, peer, activity, "capture_failed");
            let _ = send.finish();
            remove_if_current(&state, peer, activity, generation);
            return;
        }
    };
    if !current(&lock(&state)) {
        // Stopped while a permission dialog was open.
        sink.closed.store(true, Ordering::Relaxed);
        capture.stop();
        let _ = send.finish();
        return;
    }
    tracing::info!(activity, "microphone session running");

    let mut written: u64 = 0;
    let end_reason: Option<String> = loop {
        tokio::select! {
            _ = stop.notified() => break None,
            chunk = rx.recv() => {
                let Some(pcm) = chunk else { break Some("capture_failed".into()) };
                let Some(bytes) = wire::encode_chunk(&pcm) else { continue };
                if let Err(e) = send.write_all(&bytes).await {
                    tracing::debug!(activity, error = %e, "listener closed the audio stream");
                    break None;
                }
                written += bytes.len() as u64;
            }
            _ = sink.kick.notified() => {
                if let Some(r) = lock(&sink.failure).take() {
                    break Some(r);
                }
            }
        }
    };
    sink.closed.store(true, Ordering::Relaxed);
    capture.stop();
    let _ = send.finish();
    tracing::info!(
        activity,
        bytes = written,
        dropped = sink.dropped.load(Ordering::Relaxed),
        "microphone session ended"
    );
    let still_current = current(&lock(&state));
    remove_if_current(&state, peer, activity, generation);
    if let Some(reason) = end_reason
        && still_current
    {
        Mic::reply_error(&ctx, peer, activity, &reason);
        // Recording died on its own: end the activity so the indicator goes.
        ctx.stop(peer, activity);
    }
}

impl Service for Mic {
    fn id(&self) -> ServiceId {
        ServiceId::MIC
    }

    fn capability(&self) -> Capability {
        Capability::Mic
    }

    fn start(&self, ctx: ServiceCtx) {
        let _ = self.ctx.set(ctx);
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> Result<()> {
        let body: WireMic = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad Mic message".into()))?;
        let Some(m) = body.msg else { return Ok(()) };
        match msg.access {
            Access::Host(grant) => {
                // Watched: the node only lets traffic with an activity through.
                if grant.activity() == 0 {
                    return Ok(());
                }
                if let Msg::Start(_) = m {
                    self.host_start(ctx, grant);
                }
            }
            Access::Controller { activity } => {
                let peer = msg.peer;
                if !lock(&self.state).listening.contains_key(&(peer, activity)) {
                    return Ok(());
                }
                if let (Msg::Error(mut err), Some(e)) = (m, self.events()) {
                    err.sanitize();
                    e.error(peer, activity, &err.reason);
                }
            }
        }
        Ok(())
    }

    fn on_stream(&self, ctx: &ServiceCtx, mut stream: Stream) {
        match stream.access.clone() {
            Access::Controller { activity } => self.viewer_stream(ctx, stream, activity),
            Access::Host(_) => {
                // Listeners never open streams to a host.
                let _ = stream.recv.stop(stream_code::UNSUPPORTED.into());
                if let Some(s) = stream.send.as_mut() {
                    let _ = s.reset(stream_code::UNSUPPORTED.into());
                }
            }
        }
    }

    fn on_refused(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64, reason: &str) {
        if let Some(e) = self.events() {
            e.error(peer, activity, reason);
        }
    }

    fn on_activity_end(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64) {
        let (host, listened) = {
            let mut st = lock(&self.state);
            (
                st.host.remove(&(peer, activity)),
                st.listening.remove(&(peer, activity)).is_some(),
            )
        };
        if let Some(h) = host {
            // The session task stops the recording and finishes the stream.
            h.stop.notify_one();
        }
        if listened && let Some(e) = self.events() {
            e.ended(peer, activity);
        }
    }

    fn on_peer(&self, ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        if connected {
            return;
        }
        let ids: Vec<u64> = {
            let st = lock(&self.state);
            st.host
                .keys()
                .chain(st.listening.keys())
                .filter(|(p, _)| *p == peer)
                .map(|(_, a)| *a)
                .collect()
        };
        for a in ids {
            self.on_activity_end(ctx, peer, a);
        }
    }
}

/// Add this feature's service to a starting node.
pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Arc::new(Mic::default()));
}
