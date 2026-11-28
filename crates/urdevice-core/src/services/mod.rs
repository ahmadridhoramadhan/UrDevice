//! Feature services (design §9).
//!
//! Every feature (clipboard, files, input, ...) is a [`Service`] in its own
//! module below. The node owns a [`Registry`] of them and routes feature
//! traffic to them: control messages ([`urdevice_proto::Feature`]), extra
//! QUIC streams ([`urdevice_proto::StreamOpen`]), and datagrams.
//!
//! The permission check happens in the node, before a service sees anything
//! sent to the host side ([`Direction::ToHost`]). A service gets a [`Grant`]
//! as proof; it cannot make one itself (security.md: check permissions in the
//! core). Traffic for the controller side ([`Direction::ToController`]) is not
//! checked, so a service must never act on this device because of it: it may
//! only show data to the local user.
//!
//! Adding a feature means filling in its module and its `register` function.
//! `docs/dev/adding-a-feature.md` walks through it.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub use urdevice_proto::{Direction, ServiceId};

use crate::identity::DeviceId;
pub use crate::node::ServiceCtx;
use crate::permission::Capability;

pub mod adb;
pub mod automations;
pub mod bluetooth;
pub mod clip_history;
pub mod clipboard;
pub mod display;
pub mod file_browser;
pub mod files;
pub mod gallery;
pub mod input;
pub mod inspector;
pub mod kvm;
pub mod media;
pub mod mic;
pub mod notify;
pub mod port_profiles;
pub mod proximity_lock;
pub mod ring;
pub mod screen;
pub mod speaker;
pub mod telephony;
pub mod terminal;
pub mod tunnel;
pub mod webcam;

/// Ask every feature module to register its services. One line per module;
/// the modules decide for themselves whether they have anything to add.
pub(crate) fn register_all(r: &mut Registry, platform: &str) {
    clipboard::register(r, platform);
    files::register(r, platform);
    gallery::register(r, platform);
    input::register(r, platform);
    media::register(r, platform);
    screen::register(r, platform);
    tunnel::register(r, platform);
    bluetooth::register(r, platform);
    notify::register(r, platform);
    ring::register(r, platform);
    webcam::register(r, platform);
    telephony::register(r, platform);
    kvm::register(r, platform);
    display::register(r, platform);
    terminal::register(r, platform);
    port_profiles::register(r, platform);
    inspector::register(r, platform);
    adb::register(r, platform);
    file_browser::register(r, platform);
    clip_history::register(r, platform);
    proximity_lock::register(r, platform);
    automations::register(r, platform);
    speaker::register(r, platform);
}

/// True when a Release 2 capability's module lists it as shipped.
pub(crate) fn shipped(c: Capability) -> bool {
    [
        notify::SHIPPED,
        ring::SHIPPED,
        webcam::SHIPPED,
        telephony::SHIPPED,
        kvm::SHIPPED,
        display::SHIPPED,
        terminal::SHIPPED,
        port_profiles::SHIPPED,
        inspector::SHIPPED,
        adb::SHIPPED,
        file_browser::SHIPPED,
        clip_history::SHIPPED,
        proximity_lock::SHIPPED,
        automations::SHIPPED,
        speaker::SHIPPED,
        clipboard::SHIPPED,
        files::SHIPPED,
        gallery::SHIPPED,
        input::SHIPPED,
        media::SHIPPED,
        screen::SHIPPED,
        tunnel::SHIPPED,
        bluetooth::SHIPPED,
    ]
    .iter()
    .any(|list| list.contains(&c))
}

/// QUIC error codes a receiver uses to refuse a feature stream.
pub mod stream_code {
    /// The permission check failed.
    pub const REFUSED: u32 = 1;
    /// No such service, or the capability is not offered.
    pub const UNSUPPORTED: u32 = 2;
    /// The service ended the stream early, for example on Stop.
    pub const CANCELLED: u32 = 3;
}

/// Proof that the permission engine allowed something sent to this device as
/// the host. Only the node creates one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub(crate) peer: DeviceId,
    pub(crate) capability: Capability,
    pub(crate) activity: u64,
}

impl Grant {
    /// The device that asked.
    pub fn peer(&self) -> DeviceId {
        self.peer
    }

    /// The capability that was checked.
    pub fn capability(&self) -> Capability {
        self.capability
    }

    /// The activity the traffic belongs to, or 0 when the setting is Allow
    /// and the capability needs no activity.
    pub fn activity(&self) -> u64 {
        self.activity
    }
}

/// Who an incoming item is for, and what the node checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// This device is the host and the permission engine allowed it.
    Host(Grant),
    /// This device is the controller: a reply or pushed data for something
    /// it uses on the peer. Not permission-checked; never act on it beyond
    /// showing it to the local user.
    Controller {
        /// The activity the sender named, or 0. Not verified.
        activity: u64,
    },
}

/// A control message for a service.
#[derive(Debug)]
pub struct Message {
    /// The sender.
    pub peer: DeviceId,
    /// Host or controller side.
    pub access: Access,
    /// The service's own protobuf message. Untrusted: decode with size limits.
    pub body: Vec<u8>,
}

/// A QUIC stream a peer opened for a service. Read and write it from a task
/// started with [`ServiceCtx::spawn`]; dropping it closes it.
#[derive(Debug)]
pub struct Stream {
    /// The sender.
    pub peer: DeviceId,
    /// Host or controller side.
    pub access: Access,
    /// Stream kind, defined by the service.
    pub kind: u32,
    /// Extra data from the header. Untrusted.
    pub meta: Vec<u8>,
    /// The sending half for a bidirectional stream, `None` for a
    /// unidirectional one.
    pub send: Option<quinn::SendStream>,
    /// The receiving half.
    pub recv: quinn::RecvStream,
}

/// A datagram for a service (design §6.2: input events).
#[derive(Debug)]
pub struct Datagram {
    /// The sender.
    pub peer: DeviceId,
    /// Host or controller side.
    pub access: Access,
    /// The bytes after the datagram header. Untrusted.
    pub payload: Vec<u8>,
}

/// One feature service. The node calls these methods from its own threads;
/// they must return quickly and hand longer work to [`ServiceCtx::spawn`].
/// Every method has a default, so a service implements only what it uses.
pub trait Service: Any + Send + Sync {
    /// The id from [`ServiceId`]; unique per node.
    fn id(&self) -> ServiceId;

    /// The capability the permission engine checks for host-side traffic.
    fn capability(&self) -> Capability;

    /// Called once when the node has started. Keep `ctx`: it is how the
    /// service sends messages, opens streams, and asks for activities.
    fn start(&self, ctx: ServiceCtx) {
        let _ = ctx;
    }

    /// A control message arrived. Return [`crate::CoreError::Protocol`] for a
    /// malformed body: the node then closes the session and counts a
    /// violation (design §14).
    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> crate::Result<()> {
        let _ = (ctx, msg);
        Ok(())
    }

    /// A peer opened a stream for this service. The default drops it.
    fn on_stream(&self, ctx: &ServiceCtx, stream: Stream) {
        let _ = (ctx, stream);
    }

    /// A datagram arrived. The default drops it.
    fn on_datagram(&self, ctx: &ServiceCtx, datagram: Datagram) {
        let _ = (ctx, datagram);
    }

    /// The peer refused a message this service sent: `denied`, `ask`,
    /// `no_activity`, or `unsupported` (see [`urdevice_proto::FeatureRefused`]).
    fn on_refused(&self, ctx: &ServiceCtx, peer: DeviceId, activity: u64, reason: &str) {
        let _ = (ctx, peer, activity, reason);
    }

    /// An activity of this service's capability ended: Stop on either
    /// device, the setting changed to Deny (R5.5), or the session closed.
    /// Cancel its tasks and close its streams here.
    fn on_activity_end(&self, ctx: &ServiceCtx, peer: DeviceId, activity: u64) {
        let _ = (ctx, peer, activity);
    }

    /// A session with `peer` opened (`true`) or closed (`false`).
    fn on_peer(&self, ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        let _ = (ctx, peer, connected);
    }
}

/// The services of one node, built once at start.
#[derive(Default)]
pub struct Registry {
    list: Vec<Arc<dyn Service>>,
    typed: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.list.iter().map(|s| s.id()))
            .finish()
    }
}

impl Registry {
    /// Add a service. A second service with the same id is refused: that is
    /// a bug in the service table.
    pub fn add<S: Service>(&mut self, service: Arc<S>) {
        if let Err(e) = self.try_add(service) {
            tracing::error!(error = %e, "service not registered");
            debug_assert!(false, "{e}");
        }
    }

    /// Like [`Registry::add`], but returns the error.
    pub fn try_add<S: Service>(&mut self, service: Arc<S>) -> crate::Result<()> {
        let id = service.id();
        if self.list.iter().any(|s| s.id() == id) {
            return Err(crate::CoreError::InvalidInput(format!(
                "{id} is registered twice"
            )));
        }
        self.typed
            .entry(TypeId::of::<S>())
            .or_insert_with(|| service.clone() as Arc<dyn Any + Send + Sync>);
        self.list.push(service);
        Ok(())
    }

    /// The service with this id.
    pub fn get(&self, id: ServiceId) -> Option<&Arc<dyn Service>> {
        self.list.iter().find(|s| s.id() == id)
    }

    /// The first service of type `S`.
    pub fn typed<S: Service>(&self) -> Option<Arc<S>> {
        self.typed
            .get(&TypeId::of::<S>())
            .cloned()
            .and_then(|a| a.downcast::<S>().ok())
    }

    /// Every service, in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Service>> {
        self.list.iter()
    }

    /// The services that use `cap`.
    pub fn for_capability(&self, cap: Capability) -> impl Iterator<Item = &Arc<dyn Service>> {
        self.list.iter().filter(move |s| s.capability() == cap)
    }
}

/// A settable slot for something the platform or UI provides after the node
/// started: a host adapter (clipboard, input injection, ...) or a listener
/// that forwards a feature's events to the UI.
pub struct Slot<T: ?Sized>(RwLock<Option<Arc<T>>>);

impl<T: ?Sized> Default for Slot<T> {
    fn default() -> Self {
        Self(RwLock::new(None))
    }
}

impl<T: ?Sized> std::fmt::Debug for Slot<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.get().is_some() {
            "Slot(set)"
        } else {
            "Slot(empty)"
        })
    }
}

impl<T: ?Sized> Slot<T> {
    /// Put a value in, replacing the old one.
    pub fn set(&self, value: Arc<T>) {
        match self.0.write() {
            Ok(mut g) => *g = Some(value),
            Err(p) => *p.into_inner() = Some(value),
        }
    }

    /// Take the value out.
    pub fn clear(&self) {
        match self.0.write() {
            Ok(mut g) => *g = None,
            Err(p) => *p.into_inner() = None,
        }
    }

    /// The current value.
    pub fn get(&self) -> Option<Arc<T>> {
        match self.0.read() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    impl Service for A {
        fn id(&self) -> ServiceId {
            ServiceId::TEST_FIRST
        }
        fn capability(&self) -> Capability {
            Capability::Clipboard
        }
    }

    #[test]
    fn registry_refuses_duplicates_and_finds_by_type() {
        let mut r = Registry::default();
        r.try_add(Arc::new(A)).unwrap();
        assert!(r.try_add(Arc::new(A)).is_err());
        assert!(r.typed::<A>().is_some());
        assert!(r.get(ServiceId::TEST_FIRST).is_some());
        assert_eq!(r.for_capability(Capability::Clipboard).count(), 1);
        assert_eq!(r.for_capability(Capability::Input).count(), 0);
    }

    #[test]
    fn every_platform_registers_without_duplicate_ids() {
        // `Registry::add` asserts on a duplicate id in debug builds.
        for p in ["linux", "windows", "macos", "android"] {
            let mut r = Registry::default();
            register_all(&mut r, p);
            assert!(r.iter().all(|s| s.id() < ServiceId::TEST_FIRST));
        }
    }

    #[test]
    fn slot_set_get_clear() {
        let s: Slot<str> = Slot::default();
        assert!(s.get().is_none());
        s.set(Arc::from("x"));
        assert_eq!(s.get().as_deref(), Some("x"));
        s.clear();
        assert!(s.get().is_none());
    }
}
