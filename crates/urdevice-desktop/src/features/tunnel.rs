//! Localhost sharing and Expose on LAN (#22, R13): the desktop side.
//!
//! The view is `ui/features/tunnel.slint` (global `TunnelFeature`): the open
//! device's shared ports with "Open in browser", this device's shared ports
//! on the "This device" page, and the LAN banner. See
//! `docs/dev/adding-a-feature.md`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use urdevice_core::identity::DeviceId;
use urdevice_core::node::{PeerView, Snapshot};
use urdevice_core::services::tunnel::{LanMode, Tunnel as TunnelSvc, TunnelStatus, TunnelView};
use urdevice_core::{Capability, Mode};

use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::{IncomingTunnelItem, SharedPortItem, TunnelFeature, TunnelPortItem};

pub(crate) struct Tunnel;

thread_local! {
    /// Ports to open in the browser once their tunnel listens: (peer, port).
    static OPEN_WHEN_READY: RefCell<Vec<(DeviceId, u16)>> = const { RefCell::new(Vec::new()) };
}

fn mode_index(m: Option<Mode>) -> i32 {
    match m {
        Some(Mode::Allow) => 0,
        Some(Mode::Ask) => 1,
        _ => 2,
    }
}

/// "1.2 MB" style sizes.
pub(crate) fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn stats(g: &TunnelFeature<'_>, t: &TunnelView) -> SharedString {
    g.invoke_stats_text(
        i32::try_from(t.connections).unwrap_or(i32::MAX),
        bytes(t.bytes_up + t.bytes_down).into(),
    )
}

fn until_text(ts: u64) -> String {
    i64::try_from(ts)
        .ok()
        .and_then(|t| jiff::Timestamp::from_second(t).ok())
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn svc(ctx: &FeatureCtx) -> Option<Arc<TunnelSvc>> {
    ctx.node.service::<TunnelSvc>()
}

fn peer_of(w: &crate::ui::MainWindow) -> Option<DeviceId> {
    DeviceId::from_hex(&w.global::<TunnelFeature>().get_peer_id())
}

fn set_error(w: &crate::ui::MainWindow, e: impl std::fmt::Display) {
    w.global::<TunnelFeature>()
        .set_error(SharedString::from(e.to_string()));
}

fn open_browser(w: &crate::ui::MainWindow, port: u16) {
    if let Err(e) = urdevice_platform::features::tunnel::open_in_browser(port) {
        let g = w.global::<TunnelFeature>();
        g.set_error(g.invoke_no_browser(e.to_string().into()));
    }
}

impl Tunnel {
    fn draw(ctx: &FeatureCtx, snap: &Snapshot) {
        let (Some(w), Some(s)) = (ctx.window.upgrade(), svc(ctx)) else {
            return;
        };
        let g = w.global::<TunnelFeature>();
        let tunnels = s.tunnels();

        // This device as the host.
        let local_mode = snap
            .permissions
            .iter()
            .find(|p| p.capability == Capability::Tunnel)
            .map(|p| p.mode);
        g.set_local_mode(mode_index(local_mode));
        let shared: Vec<SharedPortItem> = s
            .shared()
            .into_iter()
            .map(|p| SharedPortItem {
                port: i32::from(p.port),
                label: p.label.into(),
                lan: match p.lan {
                    LanMode::Off => 0,
                    LanMode::OneHour => 1,
                    LanMode::Always => 2,
                },
                lan_text: match (p.lan, p.lan_listening) {
                    (LanMode::Off, _) => SharedString::new(),
                    (_, false) => g.invoke_lan_waiting(),
                    (LanMode::OneHour, true) => g.invoke_lan_until(until_text(p.lan_until).into()),
                    (LanMode::Always, true) => g.invoke_lan_always(),
                },
            })
            .collect();
        g.set_shared(ModelRc::from(Rc::new(VecModel::from(shared))));
        let lan = s.lan_exposed();
        g.set_lan_ports(
            lan.iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
                .into(),
        );
        let incoming: Vec<IncomingTunnelItem> = tunnels
            .iter()
            .filter(|t| t.incoming)
            .map(|t| IncomingTunnelItem {
                tunnel_id: i32::try_from(t.id).unwrap_or(0),
                text: g.invoke_incoming_text(
                    t.peer_name.clone().into(),
                    i32::from(t.remote_port),
                    stats(&g, t),
                ),
            })
            .collect();
        g.set_incoming(ModelRc::from(Rc::new(VecModel::from(incoming))));

        // The open paired device as the host.
        let Some(peer) = peer_of(&w) else { return };
        let peer_view = snap.peers.iter().find(|p| p.id == peer.hex());
        let host = peer_view.map(|p| p.name.clone()).unwrap_or_default();
        let remote_mode = peer_view.and_then(|p| {
            p.remote_permissions
                .iter()
                .find(|r| r.capability == Capability::Tunnel)
                .map(|r| r.mode)
        });
        g.set_remote_mode(mode_index(remote_mode));
        let mine: Vec<&TunnelView> = tunnels
            .iter()
            .filter(|t| !t.incoming && t.peer == peer)
            .collect();
        let mut ready = Vec::new();
        let rows: Vec<TunnelPortItem> = s
            .remote_ports(peer)
            .into_iter()
            .map(|r| {
                let t = mine.iter().find(|t| t.remote_port == r.port);
                let (status, detail) = match t {
                    None => ("", SharedString::new()),
                    Some(t) => match &t.status {
                        TunnelStatus::Opening => ("opening", SharedString::new()),
                        TunnelStatus::Open => ("open", stats(&g, t)),
                        TunnelStatus::Failed(why) => {
                            ("failed", g.invoke_reason_text(why.into(), host.clone().into()))
                        }
                    },
                };
                if let Some(t) = t
                    && let Some(local) = t.local_port
                    && t.status == TunnelStatus::Open
                {
                    ready.push((r.port, local));
                }
                TunnelPortItem {
                    port: i32::from(r.port),
                    label: r.label.into(),
                    status: status.into(),
                    local_port: t.and_then(|t| t.local_port).map_or(0, i32::from),
                    detail,
                    tunnel_id: t.map_or(0, |t| i32::try_from(t.id).unwrap_or(0)),
                }
            })
            .collect();
        g.set_remote_ports(ModelRc::from(Rc::new(VecModel::from(rows))));

        // "Open in browser" pressed before the tunnel listened.
        let failed: Vec<u16> = mine
            .iter()
            .filter(|t| matches!(t.status, TunnelStatus::Failed(_)))
            .map(|t| t.remote_port)
            .collect();
        let to_open: Vec<u16> = OPEN_WHEN_READY.with(|o| {
            let mut o = o.borrow_mut();
            let mut out = Vec::new();
            o.retain(|(p, port)| {
                if *p != peer {
                    return true;
                }
                if let Some((_, local)) = ready.iter().find(|(r, _)| r == port) {
                    out.push(*local);
                    return false;
                }
                !failed.contains(port)
            });
            out
        });
        for local in to_open {
            open_browser(&w, local);
        }
    }
}

impl Feature for Tunnel {
    fn id(&self) -> &'static str {
        "tunnel"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Tunnel,
            key: "tunnel.v1",
        }]
    }

    fn has_view(&self) -> bool {
        true
    }

    fn start(&self, ctx: &FeatureCtx) {
        let (Some(w), Some(s)) = (ctx.window.upgrade(), svc(ctx)) else {
            return;
        };
        let g = w.global::<TunnelFeature>();
        let weak = ctx.window.clone();

        let (s2, wk) = (s.clone(), weak.clone());
        g.on_open(move |port| {
            let Some(w) = wk.upgrade() else { return };
            let (Some(peer), Ok(port)) = (peer_of(&w), u16::try_from(port)) else {
                return;
            };
            set_error(&w, "");
            if let Err(e) = s2.open(peer, port) {
                set_error(&w, e);
            }
        });

        let (s2, wk) = (s.clone(), weak.clone());
        g.on_open_browser(move |port| {
            let Some(w) = wk.upgrade() else { return };
            let (Some(peer), Ok(port)) = (peer_of(&w), u16::try_from(port)) else {
                return;
            };
            set_error(&w, "");
            let open = s2.tunnels().into_iter().find(|t| {
                !t.incoming
                    && t.peer == peer
                    && t.remote_port == port
                    && t.status == TunnelStatus::Open
            });
            match open.and_then(|t| t.local_port) {
                Some(local) => open_browser(&w, local),
                None => {
                    OPEN_WHEN_READY.with(|o| {
                        let mut o = o.borrow_mut();
                        if !o.contains(&(peer, port)) {
                            o.push((peer, port));
                        }
                    });
                    if let Err(e) = s2.open(peer, port) {
                        set_error(&w, e);
                    }
                }
            }
        });

        let s2 = s.clone();
        g.on_stop(move |id| {
            if let Ok(id) = u64::try_from(id) {
                s2.stop(id);
            }
        });

        let (s2, wk) = (s.clone(), weak.clone());
        g.on_share(move |port, label| {
            let Some(w) = wk.upgrade() else { return };
            match port.trim().parse::<u16>() {
                Ok(p) if p != 0 => match s2.share(p, &label) {
                    Ok(()) => set_error(&w, ""),
                    Err(e) => set_error(&w, e),
                },
                _ => {
                    let g = w.global::<TunnelFeature>();
                    g.set_error(g.invoke_bad_port());
                }
            }
        });

        let (s2, wk) = (s.clone(), weak.clone());
        g.on_unshare(move |port| {
            let Some(w) = wk.upgrade() else { return };
            if let Ok(p) = u16::try_from(port)
                && let Err(e) = s2.unshare(p)
            {
                set_error(&w, e);
            }
        });

        let (s2, wk) = (s, weak);
        g.on_set_lan(move |port, mode, confirmed| {
            let Some(w) = wk.upgrade() else { return };
            let Ok(p) = u16::try_from(port) else { return };
            let mode = match mode {
                1 => LanMode::OneHour,
                2 => LanMode::Always,
                _ => LanMode::Off,
            };
            match s2.set_lan(p, mode, confirmed) {
                Ok(()) => set_error(&w, ""),
                Err(e) => set_error(&w, e),
            }
        });
    }

    fn refresh(&self, ctx: &FeatureCtx, snap: &Snapshot) {
        Self::draw(ctx, snap);
    }

    fn open(&self, ctx: &FeatureCtx, peer: &PeerView, _tile: &str) {
        if let Some(w) = ctx.window.upgrade() {
            let g = w.global::<TunnelFeature>();
            g.set_peer_id(peer.id.clone().into());
            g.set_error(SharedString::new());
        }
        let snap = ctx.node.snapshot();
        Self::draw(ctx, &snap);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes() {
        assert_eq!(super::bytes(999), "999 B");
        assert_eq!(super::bytes(1_200_000), "1.2 MB");
    }
}
