//! Use another device as a speaker (#40): the desktop side.
//!
//! The view is `ui/features/speaker.slint` (global `SpeakerFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Speaker;

impl Feature for Speaker {
    fn id(&self) -> &'static str {
        "speaker"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::AudioOut,
            key: "audio.out.v1",
        }]
    }
}
