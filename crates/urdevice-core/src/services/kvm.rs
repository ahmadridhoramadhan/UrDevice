//! Shared mouse and keyboard across desktops (#30, design §18.3).
//!
//! Service ids: `ServiceId::KVM`.
//!
//! Slot for the kvm feature. Implement [`super::Service`] here and add it
//! in [`register`]; see `docs/dev/adding-a-feature.md`. Wire messages go in
//! `urdevice_proto::features::kvm`.

use super::Registry;
use crate::permission::Capability;

/// Release 2 capabilities this module ships. Add each one when the feature
/// is done: the capability then shows in the settings and the handshake.
pub(crate) const SHIPPED: &[Capability] = &[];

/// Add this feature's services to a starting node. `platform` is `linux`,
/// `windows`, `macos`, or `android`.
pub(crate) fn register(_r: &mut Registry, _platform: &str) {}
