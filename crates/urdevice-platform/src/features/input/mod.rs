//! Remote control: pointer, keyboard, text, and presenter (#18, #19, design §9.3):
//! desktop OS adapters.
//!
//! [`sink`] returns the [`InputSink`] for the OS this process runs on:
//!
//! | OS | Injection |
//! |---|---|
//! | Windows | `SendInput`; text uses `KEYEVENTF_UNICODE` |
//! | macOS | `CGEventPost` (needs the Accessibility permission); text uses `CGEventKeyboardSetUnicodeString` |
//! | Linux X11 | XTest; text through a temporary keysym on a spare keycode |
//! | Linux Wayland | `org.freedesktop.portal.RemoteDesktop`, with the restore token saved in the keystore |
//! | Linux, no portal | a `uinput` virtual keyboard and mouse, after the one-time udev rule |
//!
//! A host adapter is built on first use ([`InputSink::prepare`], when a
//! remote-control session starts), because that is when the OS may ask the
//! user. Keys, text, and positions are never logged.

use std::sync::{Arc, Mutex, MutexGuard};

use urdevice_core::secret::SecretStore;
use urdevice_core::services::Grant;
use urdevice_core::services::input::{InputSink, KeyEvent, Missing, PermStatus, PointerEvent};
use urdevice_core::{CoreError, Result};

pub mod keymap;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// The secret that holds the Wayland RemoteDesktop restore token.
pub const PORTAL_TOKEN_SECRET: &str = "input-portal-restore-token";

/// What one OS back-end does. Called one at a time, in order.
pub(crate) trait Backend: Send {
    /// Name for the controller's status line.
    fn name(&self) -> &'static str;
    fn pointer(&mut self, e: &PointerEvent) -> Result<()>;
    fn key(&mut self, usage: u32, down: bool) -> Result<()>;
    fn text(&mut self, text: &str) -> Result<()>;
}

/// Why a back-end did not open.
#[derive(Debug)]
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub(crate) enum Fail {
    /// The OS needs a permission from the user.
    Missing(Missing),
    /// Input cannot work here.
    Unavailable(&'static str),
}

/// The desktop adapter for [`InputSink`].
pub struct DesktopInput {
    backend: Mutex<Option<Box<dyn Backend>>>,
    // Only the Linux portal back-end keeps a secret.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    secrets: Arc<dyn SecretStore>,
}

/// The input adapter for this OS. `secrets` keeps the Wayland portal's
/// restore token.
pub fn sink(secrets: Arc<dyn SecretStore>) -> Arc<DesktopInput> {
    Arc::new(DesktopInput {
        backend: Mutex::new(None),
        secrets,
    })
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn not_ready() -> CoreError {
    CoreError::PermissionDenied("input is not ready".into())
}

impl DesktopInput {
    #[cfg_attr(not(target_os = "linux"), allow(clippy::unused_self))]
    fn open(&self, prompt: bool) -> std::result::Result<Box<dyn Backend>, Fail> {
        #[cfg(target_os = "windows")]
        {
            let _ = prompt;
            Ok(Box::new(windows::Windows::new()))
        }
        #[cfg(target_os = "macos")]
        {
            macos::open(prompt)
        }
        #[cfg(target_os = "linux")]
        {
            linux::open(&self.secrets, prompt)
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            let _ = prompt;
            Err(Fail::Unavailable("unsupported"))
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut dyn Backend) -> Result<R>) -> Result<R> {
        let mut g = lock(&self.backend);
        let Some(b) = g.as_mut() else {
            return Err(not_ready());
        };
        let r = f(b.as_mut());
        // A back-end whose session ended (a revoked portal approval) is
        // dropped, so the next session opens a new one.
        if matches!(r, Err(CoreError::PermissionDenied(_))) {
            *g = None;
        }
        r
    }
}

impl InputSink for DesktopInput {
    fn name(&self) -> &'static str {
        lock(&self.backend)
            .as_ref()
            .map_or(std::env::consts::OS, |b| b.name())
    }

    fn permission_status(&self) -> PermStatus {
        if lock(&self.backend).is_some() {
            return PermStatus::Ready;
        }
        // Without prompting: is there something that could work?
        #[cfg(target_os = "macos")]
        {
            if macos::trusted() {
                PermStatus::Ready
            } else {
                PermStatus::Missing(Missing::Accessibility)
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            PermStatus::Ready
        }
    }

    fn prepare(&self, _grant: &Grant) -> PermStatus {
        let mut g = lock(&self.backend);
        if g.is_some() {
            return PermStatus::Ready;
        }
        match self.open(true) {
            Ok(b) => {
                tracing::info!(backend = b.name(), "input back-end ready");
                *g = Some(b);
                PermStatus::Ready
            }
            Err(Fail::Missing(m)) => PermStatus::Missing(m),
            Err(Fail::Unavailable(why)) => {
                tracing::warn!(why, "input is not available");
                PermStatus::Unavailable
            }
        }
    }

    fn pointer(&self, _grant: &Grant, e: &PointerEvent) -> Result<()> {
        self.with(|b| b.pointer(e))
    }

    fn key(&self, _grant: &Grant, e: &KeyEvent) -> Result<()> {
        self.with(|b| b.key(e.usage, e.down))
    }

    fn text(&self, _grant: &Grant, text: &str) -> Result<()> {
        self.with(|b| b.text(text))
    }
}

/// Collects wheel movement in 1/120 notches into whole notches, for back-ends
/// that scroll in steps (X11 buttons, the portal's discrete axis).
#[derive(Debug, Default)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct ScrollAccum {
    x: i32,
    y: i32,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl ScrollAccum {
    /// Add `dx`, `dy` (120 per notch) and return the whole notches now due.
    pub(crate) fn add(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        self.x = self.x.saturating_add(dx);
        self.y = self.y.saturating_add(dy);
        let (nx, ny) = (self.x / 120, self.y / 120);
        self.x -= nx * 120;
        self.y -= ny * 120;
        (nx, ny)
    }
}

/// Open the OS screen where the user grants the missing permission (R9.4).
/// On Linux, `uinput` has no settings screen: use [`install_uinput_rule`].
pub fn open_permission_settings(missing: Missing) -> Result<()> {
    let run = |cmd: &str, args: &[&str]| -> Result<()> {
        std::process::Command::new(cmd)
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(CoreError::from)
    };
    match missing {
        Missing::Accessibility => {
            #[cfg(target_os = "macos")]
            {
                run(
                    "open",
                    &[
                        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                    ],
                )
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = run;
                Err(CoreError::unsupported("input", std::env::consts::OS))
            }
        }
        Missing::Portal => {
            #[cfg(target_os = "linux")]
            {
                // GNOME keeps the saved approval under Settings > Privacy; other
                // desktops have their own place, so open the general settings.
                run("gnome-control-center", &["privacy"])
                    .or_else(|_| run("systemsettings", &[]))
                    .or_else(|_| run("xdg-open", &["settings:"]))
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = run;
                Err(CoreError::unsupported("input", std::env::consts::OS))
            }
        }
        Missing::Uinput => Err(CoreError::unsupported("input", "uinput-settings")),
        // Android settings; a desktop host never reports them.
        Missing::AccessibilityService | Missing::InputMethod => {
            Err(CoreError::unsupported("input", std::env::consts::OS))
        }
    }
}

/// Linux: install the udev rule that gives the logged-in user access to
/// `/dev/uinput`, with a one-time administrator prompt (`pkexec`), and load the
/// `uinput` module now and at boot (R9.2).
pub fn install_uinput_rule() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::install_uinput_rule()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(CoreError::unsupported("input", std::env::consts::OS))
    }
}

/// Delete the saved Wayland portal restore token (Delete all data, Reset
/// identity). It is a secret that restores remote control without a dialog.
pub fn forget_saved_tokens(secrets: &dyn SecretStore) {
    if let Err(e) = secrets.delete(PORTAL_TOKEN_SECRET) {
        tracing::warn!(error = %e, "could not delete the saved portal token");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_collects_small_steps() {
        let mut s = ScrollAccum::default();
        assert_eq!(s.add(0, 50), (0, 0));
        assert_eq!(s.add(0, 50), (0, 0));
        assert_eq!(s.add(0, 30), (0, 1));
        assert_eq!(s.add(0, -250), (0, -2));
        assert_eq!(s.add(240, 0), (2, 0));
    }
}
