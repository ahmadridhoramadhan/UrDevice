//! Wayland back-end: `org.freedesktop.portal.RemoteDesktop` (design §9.3, R9.2).
//!
//! The first session shows the compositor's approval dialog. The portal then
//! returns a restore token (`persist_mode` 2), which is kept in the keystore
//! as a secret: with it the next session starts with no dialog (spike T0.6).
//! UrDevice's own rules (Always Ask, the indicator and Stop) are the only gate
//! after that first approval. A new token comes back on every start and
//! replaces the old one. If restoring fails, the dialog shows again.
//!
//! The pointer moves by relative steps: an absolute position needs a
//! ScreenCast stream, which remote control does not open.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use urdevice_core::secret::SecretStore;
use urdevice_core::services::input::{PointerEvent, button};
use urdevice_core::{CoreError, Result};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::super::keymap::evdev;
use super::super::{Backend, PORTAL_TOKEN_SECRET, ScrollAccum};
use super::keysym;

const DEST: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.portal.RemoteDesktop";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";

/// Device types: keyboard and pointer.
const DEVICES: u32 = 1 | 2;

/// How long the user has to answer the approval dialog.
const DIALOG_WAIT: Duration = Duration::from_secs(120);

/// evdev button codes the portal takes.
const BTN_LEFT: i32 = 0x110;
const BTN_RIGHT: i32 = 0x111;
const BTN_MIDDLE: i32 = 0x112;
const BTN_SIDE: i32 = 0x113;
const BTN_EXTRA: i32 = 0x114;

const PRESSED: u32 = 1;
const RELEASED: u32 = 0;

#[derive(Debug)]
pub(super) enum Error {
    /// The desktop has no RemoteDesktop portal.
    NoPortal,
    /// The user declined, or the dialog went unanswered.
    Declined,
    /// Something else went wrong.
    Failed(String),
}

fn failed(e: impl std::fmt::Display) -> Error {
    Error::Failed(e.to_string())
}

type Options<'a> = HashMap<&'a str, Value<'a>>;
type Results = HashMap<String, OwnedValue>;

pub(super) struct Portal {
    conn: Connection,
    session: OwnedObjectPath,
    scroll: ScrollAccum,
}

/// Open a session. The approval dialog may take a while, so the work runs on
/// its own thread with a time limit.
pub(super) fn open(secrets: Arc<dyn SecretStore>) -> std::result::Result<Box<dyn Backend>, Error> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("urdevice-portal".into())
        .spawn(move || {
            let _ = tx.send(setup(secrets.as_ref()));
        })
        .map_err(failed)?;
    match rx.recv_timeout(DIALOG_WAIT + Duration::from_secs(10)) {
        Ok(Ok(p)) => Ok(Box::new(p)),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(Error::Declined),
    }
}

/// Make one portal request and wait for its `Response` signal. The request
/// object's path is known before the call (it comes from our bus name and
/// `token`), so the signal is subscribed first and cannot be missed.
fn request(
    conn: &Connection,
    token: &str,
    timeout: Duration,
    call: impl FnOnce() -> zbus::Result<()>,
) -> std::result::Result<(u32, Results), Error> {
    let sender = conn
        .unique_name()
        .ok_or_else(|| failed("no bus name"))?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let req = Proxy::new(conn, DEST, path.as_str(), REQUEST).map_err(failed)?;
    let mut signals = req.receive_signal("Response").map_err(failed)?;
    call().map_err(failed)?;
    // `next` has no timeout of its own: wait on a helper thread.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(signals.next().map(|m| {
            m.body()
                .deserialize::<(u32, Results)>()
                .map_err(|e| e.to_string())
        }));
    });
    match rx.recv_timeout(timeout) {
        Ok(Some(Ok(r))) => Ok(r),
        Ok(Some(Err(e))) => Err(Error::Failed(e)),
        Ok(None) => Err(Error::Failed("the portal closed the request".into())),
        Err(_) => Err(Error::Declined),
    }
}

fn setup(secrets: &dyn SecretStore) -> std::result::Result<Portal, Error> {
    let conn = Connection::session().map_err(failed)?;
    let rd = Proxy::new(&conn, DEST, PATH, IFACE).map_err(|_| Error::NoPortal)?;
    // The interface is missing on desktops without the portal (wlroots).
    let types: u32 = rd
        .get_property("AvailableDeviceTypes")
        .map_err(|_| Error::NoPortal)?;
    if types & DEVICES == 0 {
        return Err(Error::NoPortal);
    }
    let pid = std::process::id();

    // 1. A session.
    let (token, session_token) = (format!("urdevice_cs{pid}"), format!("urdevice_s{pid}"));
    let (code, results) = request(&conn, &token, Duration::from_secs(10), || {
        let mut o = Options::new();
        o.insert("handle_token", Value::from(token.as_str()));
        o.insert("session_handle_token", Value::from(session_token.as_str()));
        rd.call_method("CreateSession", &(o,)).map(|_| ())
    })?;
    if code != 0 {
        return Err(Error::Failed(format!("CreateSession answered {code}")));
    }
    let session = results
        .get("session_handle")
        .and_then(session_path)
        .ok_or_else(|| failed("no session handle"))?;

    // 2. The devices, with a saved approval if there is one.
    let saved = secrets
        .get(PORTAL_TOKEN_SECRET)
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .filter(|t| !t.is_empty() && t.len() < 256);
    let token = format!("urdevice_sd{pid}");
    let (code, _) = request(&conn, &token, Duration::from_secs(10), || {
        let mut o = Options::new();
        o.insert("handle_token", Value::from(token.as_str()));
        o.insert("types", Value::from(DEVICES));
        // Keep the approval until the user revokes it.
        o.insert("persist_mode", Value::from(2u32));
        if let Some(t) = &saved {
            o.insert("restore_token", Value::from(t.as_str()));
        }
        rd.call_method("SelectDevices", &(&session, o)).map(|_| ())
    })?;
    if code != 0 {
        close(&conn, &session);
        return Err(Error::Failed(format!("SelectDevices answered {code}")));
    }

    // 3. Start. This shows the dialog unless the saved approval still holds.
    let token = format!("urdevice_st{pid}");
    let (code, results) = request(&conn, &token, DIALOG_WAIT, || {
        let mut o = Options::new();
        o.insert("handle_token", Value::from(token.as_str()));
        rd.call_method("Start", &(&session, "", o)).map(|_| ())
    })
    .inspect_err(|_| close(&conn, &session))?;
    if code != 0 {
        close(&conn, &session);
        // 1: the user cancelled. 2: it ended another way.
        return Err(if code == 1 {
            Error::Declined
        } else {
            Error::Failed(format!("Start answered {code}"))
        });
    }
    let granted = results
        .get("devices")
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0);
    if granted & DEVICES != DEVICES {
        close(&conn, &session);
        return Err(Error::Declined);
    }
    // The newest token replaces the old one.
    if let Some(t) = results
        .get("restore_token")
        .and_then(|v| <&str>::try_from(v).ok())
        .filter(|t| !t.is_empty() && t.len() < 256)
        && let Err(e) = secrets.set(PORTAL_TOKEN_SECRET, t.as_bytes())
    {
        tracing::warn!(error = %e, "could not save the portal restore token");
    }
    Ok(Portal {
        conn,
        session,
        scroll: ScrollAccum::default(),
    })
}

/// The session handle is a string in the reply, an object path in some
/// portal versions.
fn session_path(v: &OwnedValue) -> Option<OwnedObjectPath> {
    if let Ok(s) = <&str>::try_from(v) {
        return ObjectPath::try_from(s).ok().map(OwnedObjectPath::from);
    }
    OwnedObjectPath::try_from(v.try_clone().ok()?).ok()
}

fn close(conn: &Connection, session: &OwnedObjectPath) {
    if let Ok(p) = Proxy::new(conn, DEST, session.as_str(), SESSION) {
        let _ = p.call_method("Close", &());
    }
}

/// A failed call means the session is gone (the user revoked it, or the
/// portal restarted). Report it as a missing permission so the adapter opens
/// a new session next time.
fn gone(e: impl std::fmt::Display) -> CoreError {
    CoreError::PermissionDenied(format!("the RemoteDesktop session ended: {e}"))
}

impl Portal {
    fn rd(&self) -> Result<Proxy<'_>> {
        Proxy::new(&self.conn, DEST, PATH, IFACE).map_err(gone)
    }

    /// `NotifyPointerMotion(session, options, dx, dy)`.
    fn motion(&self, dx: f64, dy: f64) -> Result<()> {
        let opts = Options::new();
        self.rd()?
            .call_method("NotifyPointerMotion", &(&self.session, opts, dx, dy))
            .map(|_| ())
            .map_err(gone)
    }

    /// A `Notify*` method with `(session, options, i32, u32)`: a button or a
    /// key (code, state), or a keysym (keysym, state).
    fn code(&self, method: &str, code: i32, state: u32) -> Result<()> {
        let opts = Options::new();
        self.rd()?
            .call_method(method, &(&self.session, opts, code, state))
            .map(|_| ())
            .map_err(gone)
    }

    /// `NotifyPointerAxisDiscrete(session, options, axis, steps)`.
    fn discrete(&self, axis: u32, steps: i32) -> Result<()> {
        let opts = Options::new();
        self.rd()?
            .call_method(
                "NotifyPointerAxisDiscrete",
                &(&self.session, opts, axis, steps),
            )
            .map(|_| ())
            .map_err(gone)
    }
}

impl Backend for Portal {
    fn name(&self) -> &'static str {
        "wayland-portal"
    }

    fn pointer(&mut self, e: &PointerEvent) -> Result<()> {
        match *e {
            PointerEvent::Move { dx, dy } => self.motion(f64::from(dx), f64::from(dy)),
            PointerEvent::Abs { .. } => Err(CoreError::unsupported("input", "wayland-absolute")),
            PointerEvent::Button { button: b, down } => {
                let code = match b {
                    button::LEFT => BTN_LEFT,
                    button::RIGHT => BTN_RIGHT,
                    button::MIDDLE => BTN_MIDDLE,
                    button::BACK => BTN_SIDE,
                    button::FORWARD => BTN_EXTRA,
                    _ => return Ok(()),
                };
                self.code(
                    "NotifyPointerButton",
                    code,
                    if down { PRESSED } else { RELEASED },
                )
            }
            PointerEvent::Scroll { dx, dy } => {
                // Axis 0 is vertical, 1 horizontal; down and right are
                // positive, as on the wire. Whole notches only.
                let (nx, ny) = self.scroll.add(dx, dy);
                if ny != 0 {
                    self.discrete(0, ny)?;
                }
                if nx != 0 {
                    self.discrete(1, nx)?;
                }
                Ok(())
            }
        }
    }

    fn key(&mut self, usage: u32, down: bool) -> Result<()> {
        let Some(code) = evdev(usage) else {
            return Ok(());
        };
        self.code(
            "NotifyKeyboardKeycode",
            i32::from(code),
            if down { PRESSED } else { RELEASED },
        )
    }

    fn text(&mut self, text: &str) -> Result<()> {
        for ch in text.chars() {
            let Some(sym) = keysym(ch) else { continue };
            // The keysym is a signed int on the wire; Unicode keysyms fit.
            let sym = i32::try_from(sym).unwrap_or(0);
            self.code("NotifyKeyboardKeysym", sym, PRESSED)?;
            self.code("NotifyKeyboardKeysym", sym, RELEASED)?;
        }
        Ok(())
    }
}

impl Drop for Portal {
    fn drop(&mut self) {
        close(&self.conn, &self.session);
    }
}
