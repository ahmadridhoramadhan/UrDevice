//! Clipboard sharing (#16): the desktop side.
//!
//! The view is `ui/features/clipboard.slint` (global `ClipboardFeature`). The
//! OS back-end is `urdevice_platform::features::clipboard`; the logic is the
//! core's `Clipboard` service. This file only connects the three and shows
//! what happened, never the content.

use std::sync::{Arc, Mutex, OnceLock};

use slint::ComponentHandle as _;
use urdevice_core::identity::DeviceId;
use urdevice_core::node::Snapshot;
use urdevice_core::services::clipboard::{
    ClipEvent, Clipboard as Service, ClipboardEvents, SkipReason,
};
use urdevice_core::{Capability, Node};
use urdevice_platform::features::clipboard::{DesktopClipboard, attach};

use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::{ClipboardFeature, MainWindow};

/// Keeps the OS adapter alive for as long as the app runs.
static ADAPTER: OnceLock<DesktopClipboard> = OnceLock::new();

pub(crate) struct Clipboard;

/// Shows core events in the view. Runs on core threads.
struct Events {
    window: Mutex<slint::Weak<MainWindow>>,
    node: Arc<Node>,
}

/// One short line for an event. No content, only names, types, and sizes.
fn line(node: &Node, e: &ClipEvent) -> Option<String> {
    let name = |peer: DeviceId| {
        let id = peer.hex();
        node.snapshot()
            .peers
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.name)
            .unwrap_or_default()
    };
    let kind = |m: &str| match m {
        "image/png" => "image",
        "text/html" => "rich text",
        _ => "text",
    };
    Some(match e {
        ClipEvent::Received { peer, mime, len } => {
            format!(
                "Received {} ({} bytes) from {}",
                kind(mime),
                len,
                name(*peer)
            )
        }
        ClipEvent::Sent { peer, mime, len } => {
            format!("Sent {} ({} bytes) to {}", kind(mime), len, name(*peer))
        }
        ClipEvent::Skipped(SkipReason::Sensitive) => {
            "Not sent: marked as sensitive by the app that copied it".into()
        }
        ClipEvent::Skipped(SkipReason::TooLarge) => "Not sent: over 10 MB".into(),
        ClipEvent::Skipped(SkipReason::Unsupported) => "Not sent: format not supported".into(),
        ClipEvent::Refused { peer, reason } => {
            format!("{} did not take it ({reason})", name(*peer))
        }
        ClipEvent::Skipped(SkipReason::Paused | SkipReason::NoDevice)
        | ClipEvent::SettingsChanged => {
            return None;
        }
    })
}

impl ClipboardEvents for Events {
    fn on_event(&self, event: &ClipEvent) {
        let Some(text) = line(&self.node, event) else {
            return;
        };
        let win = self
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = win.upgrade() {
                w.global::<ClipboardFeature>().set_status(text.into());
            }
        });
    }
}

fn service(node: &Node) -> Option<Arc<Service>> {
    node.service::<Service>()
}

impl Feature for Clipboard {
    fn id(&self) -> &'static str {
        "clipboard"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Clipboard,
            key: "clipboard.v1",
        }]
    }

    fn start(&self, ctx: &FeatureCtx) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        let Some(svc) = service(&ctx.node) else {
            return;
        };
        match attach(&svc) {
            Ok(a) => {
                let _ = ADAPTER.set(a);
            }
            // No usable clipboard interface (for example Wayland without
            // XWayland). The settings still work; nothing is shared.
            Err(e) => tracing::warn!(error = %e, "clipboard sharing is unavailable"),
        }
        svc.events.set(Arc::new(Events {
            window: Mutex::new(ctx.window.clone()),
            node: ctx.node.clone(),
        }));
        let g = w.global::<ClipboardFeature>();
        g.set_paused(svc.paused());
        let s = svc.clone();
        let win = ctx.window.clone();
        g.on_set_paused(move |v| {
            if let Err(e) = s.set_paused(v) {
                tracing::warn!(error = %e, "could not save the clipboard pause");
            }
            if let Some(w) = win.upgrade() {
                w.global::<ClipboardFeature>().set_paused(s.paused());
            }
        });
    }

    fn refresh(&self, ctx: &FeatureCtx, _snap: &Snapshot) {
        let (Some(w), Some(svc)) = (ctx.window.upgrade(), service(&ctx.node)) else {
            return;
        };
        let g = w.global::<ClipboardFeature>();
        g.set_paused(svc.paused());
    }

    fn switch(&self, ctx: &FeatureCtx, peer: &str) -> Option<bool> {
        Some(service(&ctx.node)?.peer_enabled(DeviceId::from_hex(peer)?))
    }

    fn set_switch(&self, ctx: &FeatureCtx, peer: &str, on: bool) {
        if let (Some(svc), Some(id)) = (service(&ctx.node), DeviceId::from_hex(peer))
            && let Err(e) = svc.set_peer_enabled(id, on)
        {
            tracing::warn!(error = %e, "could not save the clipboard switch");
        }
    }
}
