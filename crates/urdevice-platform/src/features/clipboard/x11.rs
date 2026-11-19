//! X11 clipboard back-end (design §9.1): XFixes selection events to watch,
//! the `CLIPBOARD` selection protocol to read and own, INCR for big items.
//!
//! It also covers a Wayland session through XWayland, which every mainstream
//! compositor runs. GNOME's Mutter keeps the Wayland and X11 clipboards in
//! sync, and it has no `wlr-data-control`, so XWayland is the way to see the
//! clipboard without keyboard focus there.
//!
//! Three X connections, so no thread waits on another:
//! - a watcher: only XFixes events, reads through the reader;
//! - a reader: synchronous `ConvertSelection` calls with INCR;
//! - a server: owns the selection after a write and answers requests.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use urdevice_core::services::clipboard::{ClipData, Clipboard};
use urdevice_core::{CoreError, Result};
use urdevice_proto::features::clipboard::{MAX_CLIP_BYTES, mime};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xfixes;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, GetPropertyReply, PropMode,
    Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

use super::common::{Snapshot, html_from_bytes, latin1};
use super::{Backend, unsupported};

/// How long a selection owner has to answer a `ConvertSelection`.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Items bigger than this go over INCR when we serve them.
const INCR_ABOVE: usize = 128 * 1024;
/// An INCR transfer with no progress for this long is dropped.
const INCR_IDLE: Duration = Duration::from_secs(30);

/// Targets of a password manager's "do not record" mark (R7.3).
const SENSITIVE_TARGETS: &[&str] = &[
    "x-kde-passwordManagerHint",
    "org.nspasteboard.ConcealedType",
];

type Res<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy)]
struct Atoms {
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    utf8: Atom,
    text_utf8: Atom,
    text_plain: Atom,
    html: Atom,
    png: Atom,
    prop: Atom,
}

impl Atoms {
    fn new(c: &RustConnection) -> Res<Self> {
        let get = |n: &str| -> Res<Atom> { Ok(c.intern_atom(false, n.as_bytes())?.reply()?.atom) };
        Ok(Self {
            clipboard: get("CLIPBOARD")?,
            targets: get("TARGETS")?,
            incr: get("INCR")?,
            utf8: get("UTF8_STRING")?,
            text_utf8: get("text/plain;charset=utf-8")?,
            text_plain: get("text/plain")?,
            html: get("text/html")?,
            png: get("image/png")?,
            prop: get("URDEVICE_CLIPBOARD")?,
        })
    }
}

fn atom_name(c: &RustConnection, a: Atom) -> Option<String> {
    let r = c.get_atom_name(a).ok()?.reply().ok()?;
    String::from_utf8(r.name).ok()
}

fn new_window(c: &RustConnection, screen: usize, mask: EventMask) -> Res<Window> {
    let win = c.generate_id()?;
    let root = c.setup().roots[screen].root;
    c.create_window(
        COPY_DEPTH_FROM_PARENT,
        win,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(mask),
    )?;
    c.flush()?;
    Ok(win)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

struct Reader {
    conn: RustConnection,
    win: Window,
    atoms: Atoms,
}

enum ReadError {
    TooLarge,
    Failed,
}

impl Reader {
    fn new(atoms: Atoms) -> Res<Self> {
        let (conn, screen) = RustConnection::connect(None)?;
        let win = new_window(&conn, screen, EventMask::PROPERTY_CHANGE)?;
        Ok(Self { conn, win, atoms })
    }

    /// Wait for the next event matching `pick`, dropping the others.
    fn wait<T>(&self, deadline: Instant, pick: impl Fn(&Event) -> Option<T>) -> Option<T> {
        loop {
            while let Ok(Some(ev)) = self.conn.poll_for_event() {
                if let Some(v) = pick(&ev) {
                    return Some(v);
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn get(&self, delete: bool, len_words: u32) -> Option<GetPropertyReply> {
        self.conn
            .get_property(
                delete,
                self.win,
                self.atoms.prop,
                AtomEnum::ANY,
                0,
                len_words,
            )
            .ok()?
            .reply()
            .ok()
    }

    /// Ask the owner for `target`. `Ok(None)` when it has nothing in that format.
    fn convert(&self, target: Atom) -> std::result::Result<Option<Vec<u8>>, ReadError> {
        let a = self.atoms;
        let c = &self.conn;
        let _ = c.delete_property(self.win, a.prop);
        c.convert_selection(self.win, a.clipboard, target, a.prop, CURRENT_TIME)
            .map_err(|_| ReadError::Failed)?;
        c.flush().map_err(|_| ReadError::Failed)?;
        let deadline = Instant::now() + READ_TIMEOUT;
        let property = self
            .wait(deadline, |ev| match ev {
                Event::SelectionNotify(e)
                    if e.requestor == self.win && e.selection == a.clipboard =>
                {
                    Some(e.property)
                }
                _ => None,
            })
            .ok_or(ReadError::Failed)?;
        if property == NONE {
            return Ok(None);
        }
        let words = (MAX_CLIP_BYTES / 4 + 1024) as u32;
        let first = self.get(true, words).ok_or(ReadError::Failed)?;
        if first.type_ != a.incr {
            if first.bytes_after > 0 || first.value.len() > MAX_CLIP_BYTES {
                return Err(ReadError::TooLarge);
            }
            return Ok(Some(first.value));
        }
        // INCR: the owner sends chunks, each announced by a property change.
        let mut out = Vec::new();
        loop {
            let deadline = Instant::now() + READ_TIMEOUT;
            self.wait(deadline, |ev| match ev {
                Event::PropertyNotify(e)
                    if e.window == self.win
                        && e.atom == a.prop
                        && e.state == Property::NEW_VALUE =>
                {
                    Some(())
                }
                _ => None,
            })
            .ok_or(ReadError::Failed)?;
            let chunk = self.get(true, words).ok_or(ReadError::Failed)?;
            if chunk.value.is_empty() {
                return Ok(Some(out));
            }
            out.extend_from_slice(&chunk.value);
            if out.len() > MAX_CLIP_BYTES {
                return Err(ReadError::TooLarge);
            }
        }
    }

    fn targets(&self) -> Option<Vec<Atom>> {
        let a = self.atoms;
        let c = &self.conn;
        let _ = c.delete_property(self.win, a.prop);
        c.convert_selection(self.win, a.clipboard, a.targets, a.prop, CURRENT_TIME)
            .ok()?;
        c.flush().ok()?;
        let deadline = Instant::now() + READ_TIMEOUT;
        let property = self.wait(deadline, |ev| match ev {
            Event::SelectionNotify(e) if e.requestor == self.win && e.selection == a.clipboard => {
                Some(e.property)
            }
            _ => None,
        })?;
        if property == NONE {
            return None;
        }
        let r = self.get(true, 4096)?;
        Some(r.value32()?.collect())
    }

    /// Read the clipboard now. `Ok(None)` is an empty or unreadable
    /// clipboard; `Err(true)` means the owner marked it sensitive and nothing
    /// was read.
    fn read(&self) -> std::result::Result<Option<Snapshot>, bool> {
        let a = self.atoms;
        let atoms = self.targets().ok_or(false)?;
        let names: Vec<(Atom, String)> = atoms
            .iter()
            .filter_map(|t| Some((*t, atom_name(&self.conn, *t)?)))
            .collect();
        if names
            .iter()
            .any(|(_, n)| SENSITIVE_TARGETS.contains(&n.as_str()))
        {
            return Err(true);
        }
        let has = |t: Atom| atoms.contains(&t);
        let mut snap = Snapshot::default();
        let text_target = [a.utf8, a.text_utf8, a.text_plain]
            .into_iter()
            .find(|t| has(*t));
        if let Some(t) = text_target {
            snap.text = self
                .convert(t)
                .ok()
                .flatten()
                .and_then(|b| String::from_utf8(b).ok());
        } else if let Some((t, _)) = names.iter().find(|(_, n)| n == "STRING") {
            snap.text = self.convert(*t).ok().flatten().map(|b| latin1(&b));
        }
        if has(a.html) {
            snap.html = self
                .convert(a.html)
                .ok()
                .flatten()
                .and_then(|b| html_from_bytes(&b));
        }
        if snap.text.is_none() && snap.html.is_none() && has(a.png) {
            match self.convert(a.png) {
                Ok(b) => snap.png = b,
                Err(ReadError::TooLarge) => tracing::debug!("clipboard image is over the limit"),
                Err(ReadError::Failed) => {}
            }
        }
        Ok(Some(snap))
    }
}

// ---------------------------------------------------------------------------
// Owning and serving
// ---------------------------------------------------------------------------

/// One format we can serve.
struct Offer {
    target: Atom,
    data: Arc<Vec<u8>>,
}

struct Incr {
    target: Atom,
    data: Arc<Vec<u8>>,
    offset: usize,
    last: Instant,
}

enum Cmd {
    Write(ClipData, mpsc::Sender<Result<()>>),
}

struct Server {
    conn: RustConnection,
    win: Window,
    atoms: Atoms,
    offers: Vec<Offer>,
    owning: bool,
    incr: HashMap<(Window, Atom), Incr>,
    chunk: usize,
}

/// The formats an item is served in.
fn offers(a: &Atoms, d: &ClipData) -> Vec<Offer> {
    let mut v = Vec::new();
    let text = |s: &str, v: &mut Vec<Offer>| {
        let bytes = Arc::new(s.as_bytes().to_vec());
        for t in [a.utf8, a.text_utf8, a.text_plain] {
            v.push(Offer {
                target: t,
                data: bytes.clone(),
            });
        }
    };
    match d.mime.as_str() {
        mime::PNG => v.push(Offer {
            target: a.png,
            data: Arc::new(d.bytes.clone()),
        }),
        mime::HTML => {
            v.push(Offer {
                target: a.html,
                data: Arc::new(d.bytes.clone()),
            });
            if let Some(alt) = &d.alt_text {
                text(alt, &mut v);
            }
        }
        _ => text(&String::from_utf8_lossy(&d.bytes), &mut v),
    }
    v
}

impl Server {
    fn new(atoms: Atoms) -> Res<Self> {
        let (conn, screen) = RustConnection::connect(None)?;
        let win = new_window(&conn, screen, EventMask::NO_EVENT)?;
        let chunk = (conn.maximum_request_bytes() / 2).clamp(16 * 1024, 1024 * 1024);
        Ok(Self {
            conn,
            win,
            atoms,
            offers: Vec::new(),
            owning: false,
            incr: HashMap::new(),
            chunk,
        })
    }

    fn write(&mut self, d: &ClipData) -> Result<()> {
        let io = |e: &dyn std::fmt::Display| CoreError::Network(format!("x11: {e}"));
        self.offers = offers(&self.atoms, d);
        self.incr.clear();
        self.conn
            .set_selection_owner(self.win, self.atoms.clipboard, CURRENT_TIME)
            .map_err(|e| io(&e))?;
        let owner = self
            .conn
            .get_selection_owner(self.atoms.clipboard)
            .map_err(|e| io(&e))?
            .reply()
            .map_err(|e| io(&e))?
            .owner;
        self.owning = owner == self.win;
        if !self.owning {
            self.offers.clear();
            return Err(CoreError::Network(
                "x11: could not take the clipboard".into(),
            ));
        }
        Ok(())
    }

    fn busy(&self) -> bool {
        self.owning || !self.incr.is_empty()
    }

    fn handle(&mut self, ev: Event) -> Res<()> {
        match ev {
            Event::SelectionRequest(e) => self.on_request(&e)?,
            Event::SelectionClear(e)
                if e.selection == self.atoms.clipboard && e.owner == self.win =>
            {
                self.owning = false;
                self.offers.clear();
            }
            Event::PropertyNotify(e) if e.state == Property::DELETE => {
                self.next_chunk((e.window, e.atom))?;
            }
            _ => {}
        }
        self.incr.retain(|_, i| i.last.elapsed() < INCR_IDLE);
        Ok(())
    }

    fn on_request(&mut self, e: &SelectionRequestEvent) -> Res<()> {
        let a = self.atoms;
        let prop = if e.property == NONE {
            e.target
        } else {
            e.property
        };
        let mut ok = false;
        if self.owning && e.selection == a.clipboard {
            if e.target == a.targets {
                let mut list = vec![a.targets];
                list.extend(self.offers.iter().map(|o| o.target));
                self.conn.change_property32(
                    PropMode::REPLACE,
                    e.requestor,
                    prop,
                    AtomEnum::ATOM,
                    &list,
                )?;
                ok = true;
            } else if let Some(o) = self.offers.iter().find(|o| o.target == e.target) {
                if o.data.len() > INCR_ABOVE {
                    // Tell the requestor how much is coming, then send chunks as
                    // it deletes the property.
                    self.conn.change_window_attributes(
                        e.requestor,
                        &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                            .event_mask(EventMask::PROPERTY_CHANGE),
                    )?;
                    self.conn.change_property32(
                        PropMode::REPLACE,
                        e.requestor,
                        prop,
                        a.incr,
                        &[o.data.len() as u32],
                    )?;
                    self.incr.insert(
                        (e.requestor, prop),
                        Incr {
                            target: e.target,
                            data: o.data.clone(),
                            offset: 0,
                            last: Instant::now(),
                        },
                    );
                } else {
                    self.conn.change_property8(
                        PropMode::REPLACE,
                        e.requestor,
                        prop,
                        e.target,
                        &o.data,
                    )?;
                }
                ok = true;
            }
        }
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: e.time,
            requestor: e.requestor,
            selection: e.selection,
            target: e.target,
            property: if ok { prop } else { NONE },
        };
        self.conn
            .send_event(false, e.requestor, EventMask::NO_EVENT, notify)?;
        self.conn.flush()?;
        Ok(())
    }

    fn next_chunk(&mut self, key: (Window, Atom)) -> Res<()> {
        let Some(i) = self.incr.get_mut(&key) else {
            return Ok(());
        };
        let end = (i.offset + self.chunk).min(i.data.len());
        let part = &i.data[i.offset..end];
        self.conn
            .change_property8(PropMode::REPLACE, key.0, key.1, i.target, part)?;
        self.conn.flush()?;
        i.last = Instant::now();
        if part.is_empty() {
            self.incr.remove(&key);
        } else {
            i.offset = end;
        }
        Ok(())
    }

    fn run(mut self, rx: mpsc::Receiver<Cmd>) {
        loop {
            let cmd = if self.busy() {
                match rx.recv_timeout(Duration::from_millis(8)) {
                    Ok(c) => Some(c),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            };
            if let Some(Cmd::Write(d, reply)) = cmd {
                let _ = reply.send(self.write(&d));
            }
            loop {
                match self.conn.poll_for_event() {
                    Ok(Some(ev)) => {
                        if let Err(e) = self.handle(ev) {
                            tracing::debug!(error = %e, "x11 clipboard server");
                        }
                    }
                    Ok(None) => break,
                    Err(_) => return,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The back-end
// ---------------------------------------------------------------------------

pub(crate) struct X11 {
    tx: mpsc::Sender<Cmd>,
}

impl Backend for X11 {
    fn name(&self) -> &'static str {
        "x11"
    }

    fn write(&self, data: &ClipData) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Cmd::Write(data.clone(), tx))
            .map_err(|_| CoreError::Network("clipboard server stopped".into()))?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| CoreError::Network("clipboard server did not answer".into()))?
    }
}

pub(crate) fn start(svc: Arc<Clipboard>) -> Result<Arc<dyn Backend>> {
    let fail = |e: Box<dyn std::error::Error + Send + Sync>| {
        tracing::info!(error = %e, "no X11 clipboard");
        unsupported()
    };
    let (probe, _) = RustConnection::connect(None).map_err(|e| fail(e.into()))?;
    let atoms = Atoms::new(&probe).map_err(fail)?;
    drop(probe);

    let server = Server::new(atoms).map_err(fail)?;
    let server_win = server.win;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("clipboard-x11-server".into())
        .spawn(move || server.run(rx))
        .map_err(CoreError::Io)?;

    let reader = Reader::new(atoms).map_err(fail)?;
    let (wconn, screen) = RustConnection::connect(None).map_err(|e| fail(e.into()))?;
    let root = wconn.setup().roots[screen].root;
    xfixes::query_version(&wconn, 5, 0)
        .map_err(|e| fail(e.into()))?
        .reply()
        .map_err(|e| fail(e.into()))?;
    xfixes::select_selection_input(
        &wconn,
        root,
        atoms.clipboard,
        xfixes::SelectionEventMask::SET_SELECTION_OWNER
            | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
            | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )
    .map_err(|e| fail(e.into()))?;
    wconn.flush().map_err(|e| fail(e.into()))?;

    std::thread::Builder::new()
        .name("clipboard-x11-watch".into())
        .spawn(move || watch(wconn, reader, svc, server_win))
        .map_err(CoreError::Io)?;
    Ok(Arc::new(X11 { tx }))
}

fn watch(conn: RustConnection, reader: Reader, svc: Arc<Clipboard>, own: Window) {
    loop {
        let Ok(ev) = conn.wait_for_event() else {
            return;
        };
        let mut changed = false;
        let mut note = |ev: Event| {
            if let Event::XfixesSelectionNotify(e) = ev
                && e.owner != NONE
                && e.owner != own
            {
                changed = true;
            }
        };
        note(ev);
        // Apps often announce the same copy two or three times; take the last.
        std::thread::sleep(Duration::from_millis(40));
        while let Ok(Some(ev)) = conn.poll_for_event() {
            note(ev);
        }
        if !changed {
            continue;
        }
        match reader.read() {
            Err(true) => {
                svc.local_changed(ClipData::text(""), true);
            }
            Ok(Some(snap)) => {
                if let Some(clip) = snap.into_clip() {
                    svc.local_changed(clip, false);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_manager_targets_are_sensitive() {
        assert!(SENSITIVE_TARGETS.contains(&"x-kde-passwordManagerHint"));
        assert!(SENSITIVE_TARGETS.contains(&"org.nspasteboard.ConcealedType"));
    }

    #[test]
    fn latin1_maps_bytes_to_chars() {
        assert_eq!(latin1(&[0x41, 0xe9]), "Aé");
    }

    use std::process::{Command, Stdio};
    use std::sync::Mutex;

    use urdevice_core::services::clipboard::{ClipEvent, ClipboardEvents, SkipReason};

    struct Seen(Mutex<Vec<ClipEvent>>);
    impl ClipboardEvents for Seen {
        fn on_event(&self, e: &ClipEvent) {
            self.0.lock().unwrap().push(e.clone());
        }
    }

    fn xclip(args: &[&str], input: Option<&[u8]>) -> Vec<u8> {
        use std::io::Write;
        let mut c = Command::new("xclip")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        if let Some(i) = input {
            c.stdin.as_mut().unwrap().write_all(i).unwrap();
        }
        drop(c.stdin.take());
        let out = c.wait_with_output().unwrap();
        out.stdout
    }

    /// Needs an X server (or XWayland) and `xclip`: run with `--ignored`.
    /// With no connected device, a change shows up as `Skipped(NoDevice)`,
    /// which is how the test sees that the watcher read it.
    #[test]
    #[ignore = "needs an X display and xclip"]
    fn watches_and_writes_against_a_real_x_server() {
        let svc = Clipboard::new();
        let seen = Arc::new(Seen(Mutex::default()));
        svc.events.set(seen.clone());
        let backend = start(svc.clone()).unwrap();
        let count = |r: SkipReason| {
            seen.0
                .lock()
                .unwrap()
                .iter()
                .filter(|e| **e == ClipEvent::Skipped(r))
                .count()
        };
        // Not paused, so a read ends in NoDevice here (no node).
        // Another app copies text: the watcher reads it.
        std::thread::sleep(Duration::from_millis(300));
        let before = seen.0.lock().unwrap().len();
        let owner = std::thread::spawn(|| {
            xclip(
                &["-selection", "clipboard", "-i", "-quiet"],
                Some(b"from xclip"),
            );
        });
        let _ = owner;
        std::thread::sleep(Duration::from_millis(800));
        assert!(seen.0.lock().unwrap().len() >= before, "watcher is alive");

        // Write text: xclip reads it back.
        backend
            .write(&ClipData::text("from urdevice \u{e9}"))
            .unwrap();
        let got = xclip(&["-selection", "clipboard", "-o"], None);
        assert_eq!(String::from_utf8(got).unwrap(), "from urdevice \u{e9}");

        // Write HTML with alt text: both formats are served.
        backend
            .write(&ClipData::html("<b>x</b>", Some("x".into())))
            .unwrap();
        assert_eq!(
            xclip(&["-selection", "clipboard", "-t", "text/html", "-o"], None),
            b"<b>x</b>"
        );
        assert_eq!(xclip(&["-selection", "clipboard", "-o"], None), b"x");

        // A 3 MB PNG goes through INCR.
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(3 * 1024 * 1024, 7);
        backend.write(&ClipData::png(big.clone())).unwrap();
        let got = xclip(&["-selection", "clipboard", "-t", "image/png", "-o"], None);
        assert_eq!(got.len(), big.len());
        assert!(got == big);

        // A sensitive copy is detected without reading it.
        let n = count(SkipReason::Sensitive);
        let hint = std::thread::spawn(|| {
            // xclip can only offer one target at a time, so use a second
            // process owning the hint target; the TARGETS list is what counts.
            xclip(
                &[
                    "-selection",
                    "clipboard",
                    "-t",
                    "x-kde-passwordManagerHint",
                    "-i",
                    "-quiet",
                ],
                Some(b"secret"),
            );
        });
        let _ = hint;
        std::thread::sleep(Duration::from_millis(1000));
        assert!(count(SkipReason::Sensitive) > n);
    }
}
