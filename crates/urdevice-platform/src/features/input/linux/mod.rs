//! Linux input back-ends and how one is chosen (design §9.3, R9.2).
//!
//! 1. A Wayland session uses the RemoteDesktop portal ([`portal`]): the user
//!    approves once and the restore token is kept in the keystore, so later
//!    sessions start without a dialog.
//! 2. Where there is no portal (some wlroots compositors), or on a session
//!    with no display server we know, a `uinput` virtual keyboard and mouse
//!    ([`uinput`]) is used. It needs write access to `/dev/uinput`, which
//!    [`install_uinput_rule`] gives with a one-time administrator prompt.
//! 3. An X11 session uses XTest ([`x11`]).

use std::sync::Arc;

use urdevice_core::secret::SecretStore;
use urdevice_core::services::input::Missing;
use urdevice_core::{CoreError, Result};

use super::{Backend, Fail};

mod portal;
mod uinput;
mod x11;

/// True when this is a Wayland session.
fn wayland_session() -> bool {
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => true,
        Ok("x11") => false,
        _ => std::env::var_os("WAYLAND_DISPLAY").is_some(),
    }
}

pub(super) fn open(
    secrets: &Arc<dyn SecretStore>,
    _prompt: bool,
) -> std::result::Result<Box<dyn Backend>, Fail> {
    if wayland_session() {
        match portal::open(secrets.clone()) {
            Ok(b) => return Ok(b),
            Err(portal::Error::Declined) => return Err(Fail::Missing(Missing::Portal)),
            Err(portal::Error::NoPortal) => {}
            Err(portal::Error::Failed(why)) => {
                tracing::warn!(why, "the RemoteDesktop portal failed");
            }
        }
        return uinput::open();
    }
    if std::env::var_os("DISPLAY").is_some() {
        match x11::open() {
            Ok(b) => return Ok(b),
            Err(why) => tracing::warn!(why, "XTest is not available"),
        }
    }
    uinput::open()
}

/// The keysym that types `ch`, for X11 and the portal. Latin-1 characters
/// are their own keysym; the rest use the Unicode range (`0x01000000 + code`).
/// Newline and tab are the Return and Tab keys. Other control characters
/// have none.
pub(super) fn keysym(ch: char) -> Option<u32> {
    match ch {
        '\n' => Some(0xFF0D),
        '\t' => Some(0xFF09),
        c if (c as u32) < 0x20 || (0x7F..0xA0).contains(&(c as u32)) => None,
        c if (c as u32) <= 0xFF => Some(c as u32),
        c => Some(0x0100_0000 | c as u32),
    }
}

const RULE_PATH: &str = "/etc/udev/rules.d/70-urdevice-uinput.rules";
const MODULE_PATH: &str = "/etc/modules-load.d/urdevice-uinput.conf";

/// The udev rule: `uaccess` gives the user at the active seat an ACL on
/// `/dev/uinput`, and `static_node` creates the node at boot.
const RULE: &str =
    r#"KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput""#;

/// The shell script `pkexec` runs as root.
fn install_script() -> String {
    format!(
        "set -e; printf '%s\\n' '{RULE}' > {RULE_PATH}; printf 'uinput\\n' > {MODULE_PATH}; \
         modprobe uinput; udevadm control --reload-rules; \
         udevadm trigger --subsystem-match=misc --sysname-match=uinput"
    )
}

pub(super) fn install_uinput_rule() -> Result<()> {
    let status = std::process::Command::new("pkexec")
        .args(["sh", "-c", &install_script()])
        .status()
        .map_err(CoreError::from)?;
    if status.success() {
        Ok(())
    } else {
        Err(CoreError::PermissionDenied(
            "the administrator step did not finish".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms_follow_the_x11_rules() {
        assert_eq!(keysym('a'), Some(0x61));
        assert_eq!(keysym('é'), Some(0xE9));
        assert_eq!(keysym('世'), Some(0x0100_4E16));
        assert_eq!(keysym('😀'), Some(0x0101_F600));
        assert_eq!(keysym('\n'), Some(0xFF0D));
        assert_eq!(keysym('\t'), Some(0xFF09));
        assert_eq!(keysym('\u{1b}'), None);
        assert_eq!(keysym('\u{85}'), None);
    }

    #[test]
    fn the_install_script_writes_the_rule_and_loads_the_module() {
        let s = install_script();
        assert!(s.contains("uaccess") && s.contains(RULE_PATH) && s.contains("modprobe uinput"));
        // The rule sits inside single quotes, so it must not hold one.
        assert!(!RULE.contains('\''));
    }
}
