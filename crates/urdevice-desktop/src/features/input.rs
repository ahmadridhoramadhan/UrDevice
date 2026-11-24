//! Remote control: pointer, keyboard, text, and presenter (#18, #19): the desktop side.
//!
//! This desktop is the **host** here: another device (the phone) controls it.
//! The OS back-end is `urdevice_platform::features::input`; the logic, the
//! permission check, and the indicator with Stop are the core's `Input`
//! service and the main window's activity list. This file connects them and
//! adds the two pieces that need the UI:
//!
//! - the **laser overlay**, a transparent click-through window that stays on
//!   top ([`Laser`]), with a cursor-move fallback where no window can be made;
//! - the **missing permission** dialog (R9.4): it says which OS permission is
//!   needed and has a button for the settings screen (macOS Accessibility, the
//!   Wayland approval) or for the one-time `uinput` rule.
//!
//! Controlling another device from this desktop (#19) is the remote-control
//! window in [`remote`]: it captures the pointer and keyboard, with
//! Ctrl+Alt+G to release them.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use slint::ComponentHandle as _;
use urdevice_core::Capability;
use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::input::{
    HostStatus, Input as Service, InputEvents, InputSink, Missing, Overlay, PointerEvent,
};
use urdevice_platform::features::input::{
    DesktopInput, install_uinput_rule, open_permission_settings, sink,
};

use urdevice_core::node::PeerView;

use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::InputFeature;
#[cfg(not(windows))]
use crate::ui::LaserWindow;

#[cfg(windows)]
mod laser_win;
mod remote;

pub(crate) struct Input;

thread_local! {
    /// The name of the device the view was opened for, for the window title.
    static LAST_NAME: RefCell<String> = const { RefCell::new(String::new()) };

    /// The laser window. Slint windows live on the UI thread.
    #[cfg(not(windows))]
    static LASER: RefCell<Option<LaserWindow>> = const { RefCell::new(None) };

    /// The laser dot on Windows: a small layered window, not a Slint one (see `laser_win`).
    #[cfg(windows)]
    static DOT: RefCell<Option<laser_win::Dot>> = const { RefCell::new(None) };

    /// Keeps the display size up to date for the input back-end.
    static SIZE_TIMER: slint::Timer = slint::Timer::default();
}

/// Tell the input back-end how big the display is, in the pixels the pointer
/// moves in (logical pixels). Wayland cannot place the pointer, so the back-end
/// works out a position from this. Runs on the UI thread.
fn report_display_size(window: &slint::Weak<crate::ui::MainWindow>, input: &DesktopInput) {
    use slint::winit_030::WinitWindowAccessor;
    let Some(w) = window.upgrade() else { return };
    w.window().with_winit_window(|win| {
        if let Some(m) = win.current_monitor().or_else(|| win.primary_monitor()) {
            let scale = m.scale_factor() as f32;
            let size = m.size();
            if scale > 0.0 {
                input.set_display_size(size.width as f32 / scale, size.height as f32 / scale);
            }
        }
    });
}

/// Draws the presenter's laser dot (R9b.3).
struct Laser {
    /// Moves the cursor when no overlay window can be made.
    fallback: Arc<DesktopInput>,
}

impl Overlay for Laser {
    fn laser(&self, grant: &Grant, pos: Option<(f32, f32)>) -> urdevice_core::Result<()> {
        let fallback = self.fallback.clone();
        let grant = grant.clone();
        slint::invoke_from_event_loop(move || {
            if let Err(why) = draw(pos) {
                tracing::debug!(why, "no laser window; moving the cursor instead");
                if let Some((x, y)) = pos {
                    let to = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u32;
                    let _ = fallback.pointer(
                        &grant,
                        &PointerEvent::Abs {
                            x: to(x),
                            y: to(y),
                            display: 0,
                        },
                    );
                }
            }
        })
        .map_err(|_| urdevice_core::CoreError::Stopped)
    }
}

/// Show the dot at `pos` (0.0 to 1.0 of the primary display), or hide it.
/// Runs on the UI thread.
#[cfg(windows)]
fn draw(pos: Option<(f32, f32)>) -> Result<(), String> {
    DOT.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some((x, y)) = pos else {
            if let Some(dot) = slot.as_mut() {
                dot.hide();
            }
            return Ok(());
        };
        if slot.is_none() {
            *slot = Some(laser_win::Dot::new()?);
        }
        match slot.as_mut() {
            Some(dot) => dot.show_at(x, y),
            None => Ok(()),
        }
    })
}

/// Show the dot at `pos` (0.0 to 1.0 of the primary display), or hide it.
/// Runs on the UI thread.
#[cfg(not(windows))]
fn draw(pos: Option<(f32, f32)>) -> Result<(), String> {
    LASER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some((x, y)) = pos else {
            if let Some(w) = slot.as_ref() {
                w.set_shown(false);
                let _ = w.hide();
            }
            return Ok(());
        };
        if slot.is_none() {
            *slot = Some(LaserWindow::new().map_err(|e| e.to_string())?);
        }
        let Some(w) = slot.as_ref() else {
            return Ok(());
        };
        w.show().map_err(|e| e.to_string())?;
        cover_display(w);
        let size = w.window().size().to_logical(w.window().scale_factor());
        w.set_dot_x(x * size.width);
        w.set_dot_y(y * size.height);
        w.set_shown(true);
        Ok(())
    })
}

/// Make the window fill the primary display and let clicks through. Done on
/// every show: cheap, and it follows a changed monitor setup.
#[cfg(not(windows))]
fn cover_display(w: &LaserWindow) {
    use slint::winit_030::WinitWindowAccessor;
    w.window().with_winit_window(|win| {
        // Clicks go to the window below. Not every system supports it (some
        // Wayland compositors); the dot is then merely in the way.
        let _ = win.set_cursor_hittest(false);
        if let Some(m) = win.primary_monitor().or_else(|| win.current_monitor()) {
            let (p, s) = (m.position(), m.size());
            w.window()
                .set_position(slint::PhysicalPosition::new(p.x, p.y));
            w.window()
                .set_size(slint::PhysicalSize::new(s.width, s.height));
        }
    });
}

/// Tells the user what the OS needs (R9.4).
struct Events {
    /// Unix seconds of the last dialog, so one problem asks once a minute.
    last: AtomicU64,
}

impl InputEvents for Events {
    fn session(&self, peer: DeviceId, active: bool) {
        let _ = slint::invoke_from_event_loop(move || remote::session(peer, active));
    }

    fn host_status(&self, peer: DeviceId, status: &HostStatus) {
        let status = status.clone();
        let _ = slint::invoke_from_event_loop(move || remote::host_status(peer, status));
    }

    fn refused(&self, peer: DeviceId, reason: &str) {
        let reason = reason.to_owned();
        let _ = slint::invoke_from_event_loop(move || remote::refused(peer, &reason));
    }

    fn missing_permission(&self, _peer: DeviceId, missing: Missing) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let before = self.last.swap(now, Ordering::Relaxed);
        if now.saturating_sub(before) < 60 {
            return;
        }
        let _ = slint::invoke_from_event_loop(move || ask(missing));
    }
}

/// What to tell the user, and what the button does.
fn explain(missing: Missing) -> (&'static str, &'static str) {
    match missing {
        Missing::Accessibility => (
            "Another device wants to control this Mac, but macOS has not allowed UrDevice to do that. Turn on UrDevice in System Settings > Privacy & Security > Accessibility, then start remote control again.",
            "Open Accessibility settings",
        ),
        Missing::Uinput => (
            "Another device wants to control this computer, but this desktop has no remote-control portal and UrDevice cannot write to /dev/uinput. A one-time setup adds a udev rule that gives your user access. It needs your administrator password.",
            "Set up uinput",
        ),
        Missing::Portal => (
            "Another device wants to control this computer, but the desktop's remote-control approval was not given. Approve it when your desktop asks, or review the saved approval in your desktop's privacy settings, then start remote control again.",
            "Open privacy settings",
        ),
        // Android settings; a desktop host never reports them.
        Missing::AccessibilityService | Missing::InputMethod => (
            "Another device wants to control this computer, but the system needs a permission first.",
            "Open settings",
        ),
    }
}

/// Show the explanation with its settings button. Runs on the UI thread.
fn ask(missing: Missing) {
    let (text, action) = explain(missing);
    let answer = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("UrDevice needs a permission")
        .set_description(text)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            action.to_owned(),
            "Not now".to_owned(),
        ))
        .show();
    if !matches!(answer, rfd::MessageDialogResult::Custom(ref s) if s == action) {
        return;
    }
    // The administrator prompt waits for the user: keep it off the UI thread.
    std::thread::spawn(move || {
        let r = match missing {
            Missing::Uinput => install_uinput_rule(),
            other => open_permission_settings(other),
        };
        if let Err(e) = r {
            tracing::warn!(error = %e, "could not open the permission step");
        }
    });
}

impl Feature for Input {
    fn id(&self) -> &'static str {
        "input"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[
            TileSpec {
                cap: Capability::Input,
                key: "input.v1",
            },
            TileSpec {
                cap: Capability::Input,
                key: "keyboard",
            },
        ]
    }

    fn has_view(&self) -> bool {
        true
    }

    fn open(&self, ctx: &FeatureCtx, peer: &PeerView, _tile: &str) {
        let (Some(w), Some(svc), Some(id)) = (
            ctx.window.upgrade(),
            ctx.node.service::<Service>(),
            DeviceId::from_hex(&peer.id),
        ) else {
            return;
        };
        w.global::<InputFeature>()
            .set_peer_id(peer.id.as_str().into());
        LAST_NAME.with(|n| *n.borrow_mut() = peer.name.clone());
        remote::open(svc, id, &peer.name);
    }

    fn start(&self, ctx: &FeatureCtx) {
        let Some(svc) = ctx.node.service::<Service>() else {
            return;
        };
        if let Some(w) = ctx.window.upgrade() {
            let (window, svc) = (ctx.window.clone(), svc.clone());
            w.global::<InputFeature>().on_open_window(move || {
                let Some(w) = window.upgrade() else { return };
                let Some(id) = DeviceId::from_hex(&w.global::<InputFeature>().get_peer_id()) else {
                    return;
                };
                let name = LAST_NAME.with(|n| n.borrow().clone());
                remote::open(svc.clone(), id, &name);
            });
        }
        let secrets = Arc::new(urdevice_platform::DesktopSecretStore::new(
            &urdevice_platform::data_dir(),
        ));
        let input = sink(secrets);
        svc.set_sink(input.clone());
        {
            let (window, input) = (ctx.window.clone(), input.clone());
            report_display_size(&window, &input);
            SIZE_TIMER.with(|t| {
                t.start(
                    slint::TimerMode::Repeated,
                    std::time::Duration::from_secs(3),
                    move || report_display_size(&window, &input),
                );
            });
        }
        svc.set_overlay(Arc::new(Laser { fallback: input }));
        svc.set_events(Arc::new(Events {
            last: AtomicU64::new(0),
        }));
    }
}
