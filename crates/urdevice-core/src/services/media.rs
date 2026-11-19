//! Media control (#20, design §9.4).
//!
//! Service ids: `ServiceId::MEDIA`.
//!
//! Slot for the media feature. Implement [`super::Service`] here and add it
//! in [`register`]; see `docs/dev/adding-a-feature.md`. Wire messages go in
//! `urdevice_proto::features::media`.

use super::Registry;
use crate::permission::Capability;

/// Release 2 capabilities this module ships. Release 1 capabilities are
/// always listed, so this stays empty.
pub(crate) const SHIPPED: &[Capability] = &[];

/// Add this feature's services to a starting node. `platform` is `linux`,
/// `windows`, `macos`, or `android`.
pub(crate) fn register(_r: &mut Registry, _platform: &str) {}
