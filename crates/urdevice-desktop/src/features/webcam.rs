//! Phone as webcam and microphone (#28): the desktop side.
//!
//! The view is `ui/features/webcam.slint` (global `WebcamFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Webcam;

impl Feature for Webcam {
    fn id(&self) -> &'static str {
        "webcam"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[
            TileSpec {
                cap: Capability::Camera,
                key: "camera.v1",
            },
            TileSpec {
                cap: Capability::Mic,
                key: "mic.v1",
            },
        ]
    }
}
