//! Use another device as a speaker (#40, design §18.15).
//!
//! Service ids: `ServiceId::SPEAKER`.
//!
//! Slot for the speaker feature. Implement [`super::Service`] here and add it
//! in [`register`]; see `docs/dev/adding-a-feature.md`. Wire messages go in
//! `urdevice_proto::features::speaker`.

use super::Registry;
use crate::permission::Capability;

/// Release 2 capabilities this module ships. Add each one when the feature
/// is done: the capability then shows in the settings and the handshake.
pub(crate) const SHIPPED: &[Capability] = &[];

/// Add this feature's services to a starting node. `platform` is `linux`,
/// `windows`, `macos`, or `android`.
pub(crate) fn register(_r: &mut Registry, _platform: &str) {}
