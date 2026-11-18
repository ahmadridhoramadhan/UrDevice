//! Glue between the Slint UI and the core node (design §12: the UI runs on the
//! main thread, the core on its own tokio runtime).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::error::Error;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use urdevice_core::node::{EventSink, PeerStatus, PermissionView, Snapshot};
use urdevice_core::{Capability, Mode, Node, NodeConfig, NodeEvent};

use crate::ui::{
    ActivityItem, CodeItem, Labels, LogItem, MainWindow, NearbyItem, PeerItem, PermItem,
    PromptWindow, TileItem,
};

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

/// Forwards core events to the UI thread. `Changed` events are merged so a
/// burst causes one refresh.
struct Sink {
    pending: Arc<AtomicBool>,
}

impl EventSink for Sink {
    fn on_event(&self, event: NodeEvent) {
        if event == NodeEvent::Changed {
            if self.pending.swap(true, Ordering::AcqRel) {
                return;
            }
            let pending = self.pending.clone();
            let _ = slint::invoke_from_event_loop(move || match UI.with(|u| u.borrow().clone()) {
                Some(ui) => ui.schedule_refresh(pending),
                None => pending.store(false, Ordering::Release),
            });
            return;
        }
        let _ = slint::invoke_from_event_loop(move || with_ui(|ui| ui.handle(event)));
    }
}

fn with_ui(f: impl FnOnce(&Rc<Ui>)) {
    let ui = UI.with(|u| u.borrow().clone());
    if let Some(ui) = ui {
        f(&ui);
    }
}

fn mode_index(m: Mode) -> i32 {
    match m {
        Mode::Allow => 0,
        Mode::Ask => 1,
        Mode::Deny => 2,
    }
}

fn mode_from_index(i: i32) -> Mode {
    match i {
        0 => Mode::Allow,
        1 => Mode::Ask,
        _ => Mode::Deny,
    }
}

thread_local! {
    static ICONS: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
}

/// The icon for a capability. Each SVG is parsed once; the same image is
/// handed out after that, so unchanged rows compare equal.
fn icon(cap: &str) -> Image {
    ICONS.with(|icons| {
        icons
            .borrow_mut()
            .entry(cap.to_owned())
            .or_insert_with(|| load_icon(cap))
            .clone()
    })
}

fn load_icon(cap: &str) -> Image {
    let svg: &[u8] = match cap {
        "clipboard.v1" => include_bytes!("../ui/icons/clipboard.svg"),
        "files.v1" => include_bytes!("../ui/icons/folder.svg"),
        "gallery.v1" => include_bytes!("../ui/icons/image.svg"),
        "input.v1" => include_bytes!("../ui/icons/mouse.svg"),
        "keyboard" => include_bytes!("../ui/icons/keyboard.svg"),
        "media.v1" => include_bytes!("../ui/icons/play.svg"),
        "screen.h264.v1" => include_bytes!("../ui/icons/monitor.svg"),
        "tunnel.v1" => include_bytes!("../ui/icons/code.svg"),
        "term.v1" => include_bytes!("../ui/icons/terminal.svg"),
        _ => include_bytes!("../ui/icons/pencil.svg"),
    };
    Image::load_from_svg_data(svg).unwrap_or_default()
}

/// Display name of a platform string.
fn platform_label(p: &str) -> String {
    match p {
        "linux" => "Linux".into(),
        "windows" => "Windows".into(),
        "macos" => "macOS".into(),
        "android" => "Android".into(),
        other => other.into(),
    }
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// Bring `m` in line with `rows`, touching only rows that changed. Replacing
/// the whole model makes Slint rebuild every row, which drops a click that
/// is in progress (#45).
fn sync<T: Clone + PartialEq + 'static>(m: &VecModel<T>, rows: Vec<T>) {
    if m.row_count() != rows.len() {
        m.set_vec(rows);
        return;
    }
    for (i, row) in rows.into_iter().enumerate() {
        if m.row_data(i).as_ref() != Some(&row) {
            m.set_row_data(i, row);
        }
    }
}

/// The list models the window shows. They are set on the window once and
/// then updated in place.
#[derive(Default)]
struct Models {
    permissions: Rc<VecModel<PermItem>>,
    onboarding_perms: Rc<VecModel<PermItem>>,
    peers: Rc<VecModel<PeerItem>>,
    tiles: Rc<VecModel<TileItem>>,
    nearby: Rc<VecModel<NearbyItem>>,
    codes: Rc<VecModel<CodeItem>>,
    activities: Rc<VecModel<ActivityItem>>,
}

impl Models {
    fn attach(&self, w: &MainWindow) {
        w.set_permissions(self.permissions.clone().into());
        w.set_onboarding_perms(self.onboarding_perms.clone().into());
        w.set_peers(self.peers.clone().into());
        w.set_tiles(self.tiles.clone().into());
        w.set_nearby(self.nearby.clone().into());
        w.set_codes(self.codes.clone().into());
        w.set_activities(self.activities.clone().into());
    }
}

/// Fewest milliseconds between two redraws caused by core events (#45).
const MIN_REFRESH_GAP: Duration = Duration::from_millis(250);

fn local_time(ts: u64) -> String {
    i64::try_from(ts)
        .ok()
        .and_then(|t| jiff::Timestamp::from_second(t).ok())
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// Render a QR matrix into an image with a 4-module quiet zone.
fn qr_image(text: &str) -> Image {
    let Ok(m) = urdevice_core::qr::encode(text) else {
        return Image::default();
    };
    let quiet = 4u32;
    let scale = 6u32;
    let side = (m.size + quiet * 2) * scale;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(side, side);
    let dark = slint::Rgb8Pixel {
        r: 0x0f,
        g: 0x1f,
        b: 0x10,
    };
    let light = slint::Rgb8Pixel {
        r: 255,
        g: 255,
        b: 255,
    };
    let pixels = buf.make_mut_slice();
    for y in 0..side {
        for x in 0..side {
            let (mx, my) = (
                (x / scale).wrapping_sub(quiet),
                (y / scale).wrapping_sub(quiet),
            );
            let on = mx < m.size && my < m.size && m.modules[(my * m.size + mx) as usize];
            pixels[(y * side + x) as usize] = if on { dark } else { light };
        }
    }
    Image::from_rgb8(buf)
}

pub(crate) struct App {
    node: Arc<Node>,
}

impl App {
    pub(crate) fn start(window: &MainWindow) -> Result<Self, Box<dyn Error>> {
        let data_dir = urdevice_platform::data_dir();
        let secrets = Arc::new(urdevice_platform::DesktopSecretStore::new(&data_dir));
        let sink = Arc::new(Sink {
            pending: Arc::new(AtomicBool::new(false)),
        });
        let mut cfg = NodeConfig::new(
            data_dir.clone(),
            urdevice_platform::platform_id(),
            &urdevice_platform::device_name(),
            secrets,
            sink,
        );
        if let Some(port) = std::env::var("URDEVICE_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
        {
            cfg.quic_port = port;
        }
        let node = Arc::new(Node::start(cfg)?);
        node.set_battery(urdevice_platform::battery());

        let ui = Rc::new(Ui {
            window: window.as_weak(),
            node: node.clone(),
            onboarding_modes: RefCell::new(
                Capability::for_platform(urdevice_platform::platform_id())
                    .into_iter()
                    .filter(|c| !c.local_only())
                    .map(|c| PermissionView {
                        capability: c,
                        mode: c.default_mode(),
                    })
                    .collect(),
            ),
            prompts: RefCell::new(HashMap::new()),
            qr_uri: RefCell::new(String::new()),
            toast_timer: Timer::default(),
            scanner: RefCell::new(None),
            data_dir: data_dir.display().to_string(),
            last_snapshot: RefCell::new(None),
            onboard_shown: Cell::new(false),
            models: Models::default(),
            last_refresh: Cell::new(Instant::now()),
            refresh_timer: Timer::default(),
        });
        UI.with(|u| *u.borrow_mut() = Some(ui.clone()));
        ui.models.attach(window);
        ui.wire(window);
        window.set_core_version(urdevice_core::core_version().into());
        window.set_has_camera(crate::camera::available());
        window.set_autostart(
            autostart()
                .and_then(|a| a.is_enabled().ok())
                .unwrap_or(false),
        );
        window.set_data_path(ui.data_dir.clone().into());
        ui.refresh();
        Ok(Self { node })
    }

    pub(crate) fn node(&self) -> Arc<Node> {
        self.node.clone()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        UI.with(|u| u.borrow_mut().take());
    }
}

/// Report the battery once a minute (R4.9). The core sends it only when it moved.
pub(crate) fn battery_thread(node: Weak<Node>) {
    let _ = std::thread::Builder::new()
        .name("urdevice-battery".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                let Some(node) = node.upgrade() else { return };
                node.set_battery(urdevice_platform::battery());
            }
        });
}

fn autostart() -> Option<auto_launch::AutoLaunch> {
    let exe = std::env::current_exe().ok()?;
    auto_launch::AutoLaunchBuilder::new()
        .set_app_name("UrDevice")
        .set_app_path(&exe.to_string_lossy())
        .build()
        .ok()
}

struct Ui {
    window: slint::Weak<MainWindow>,
    node: Arc<Node>,
    onboarding_modes: RefCell<Vec<PermissionView>>,
    prompts: RefCell<HashMap<u64, (PromptWindow, Timer)>>,
    qr_uri: RefCell<String>,
    toast_timer: Timer,
    scanner: RefCell<Option<crate::camera::Scanner>>,
    data_dir: String,
    last_snapshot: RefCell<Option<Snapshot>>,
    onboard_shown: Cell<bool>,
    models: Models,
    last_refresh: Cell<Instant>,
    refresh_timer: Timer,
}

impl Ui {
    /// Redraw for a core `Changed` event, at most once per [`MIN_REFRESH_GAP`].
    /// `pending` is cleared just before the redraw reads the snapshot, so no
    /// change is lost.
    fn schedule_refresh(&self, pending: Arc<AtomicBool>) {
        let wait = MIN_REFRESH_GAP.saturating_sub(self.last_refresh.get().elapsed());
        if wait.is_zero() {
            pending.store(false, Ordering::Release);
            self.refresh();
            return;
        }
        self.refresh_timer
            .start(TimerMode::SingleShot, wait, move || {
                pending.store(false, Ordering::Release);
                with_ui(|ui| ui.refresh());
            });
    }

    fn labels(w: &MainWindow) -> Labels<'_> {
        w.global::<Labels>()
    }

    fn toast(&self, text: SharedString) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        w.set_toast(text);
        let weak = self.window.clone();
        self.toast_timer
            .start(TimerMode::SingleShot, Duration::from_secs(6), move || {
                if let Some(w) = weak.upgrade() {
                    w.set_toast(SharedString::new());
                }
            });
    }

    fn perm_item(w: &MainWindow, cap: Capability, mode: Mode) -> PermItem {
        let l = Self::labels(w);
        PermItem {
            cap: cap.as_str().into(),
            label: l.invoke_cap_label(cap.as_str().into()),
            desc: l.invoke_cap_desc(cap.as_str().into()),
            icon: icon(cap.as_str()),
            mode: mode_index(mode),
            local_only: cap.local_only(),
        }
    }

    fn refresh(&self) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        self.last_refresh.set(Instant::now());
        let snap = self.node.snapshot();
        let l = Self::labels(&w);

        if !snap.me.onboarded && !self.onboard_shown.get() {
            self.onboard_shown.set(true);
            w.set_onboarding(true);
            w.set_onboarding_step(1);
            w.set_onboarding_name(snap.me.name.clone().into());
        }
        if snap.me.onboarded {
            self.onboard_shown.set(false);
        }

        w.set_me_name(snap.me.name.clone().into());
        w.set_me_platform(platform_label(&snap.me.platform).into());
        w.set_me_fingerprint(snap.me.fingerprint.clone().into());
        w.set_me_address(
            snap.me
                .addresses
                .first()
                .cloned()
                .unwrap_or_default()
                .into(),
        );
        w.set_me_battery(
            snap.me
                .battery_pct
                .map(|p| format!("{p}%"))
                .unwrap_or_default()
                .into(),
        );
        w.set_me_hidden(snap.me.hidden);

        sync(
            &self.models.permissions,
            snap.permissions
                .iter()
                .map(|p| Self::perm_item(&w, p.capability, p.mode))
                .collect(),
        );
        sync(
            &self.models.onboarding_perms,
            self.onboarding_modes
                .borrow()
                .iter()
                .map(|p| Self::perm_item(&w, p.capability, p.mode))
                .collect(),
        );

        let peers: Vec<PeerItem> = snap
            .peers
            .iter()
            .map(|p| {
                let (status, status_text) = match &p.status {
                    PeerStatus::Connected => ("connected", String::new()),
                    PeerStatus::Disconnected { by } => (
                        "disconnected",
                        l.invoke_disconnected_by(by.clone().into()).to_string(),
                    ),
                    PeerStatus::Offline if p.last_seen > 0 => (
                        "offline",
                        l.invoke_offline_since(local_time(p.last_seen).into())
                            .to_string(),
                    ),
                    PeerStatus::Offline => ("offline", l.invoke_offline().to_string()),
                };
                PeerItem {
                    id: p.id.clone().into(),
                    name: p.name.clone().into(),
                    platform: platform_label(&p.platform).into(),
                    status: status.into(),
                    status_text: status_text.into(),
                    address: p.address.clone().into(),
                    path: if p.path == "wifi" {
                        "Wi-Fi".into()
                    } else {
                        p.path.clone().into()
                    },
                    rtt: p
                        .rtt_ms
                        .map(|r| format!("{r} ms"))
                        .unwrap_or_default()
                        .into(),
                    battery: p
                        .battery_pct
                        .map(|b| {
                            if p.charging {
                                format!("{b}% ⚡")
                            } else {
                                format!("{b}%")
                            }
                        })
                        .unwrap_or_default()
                        .into(),
                    fingerprint: p.fingerprint.clone().into(),
                }
            })
            .collect();

        let selected = w.get_selected_peer().to_string();
        if let Some(sp) = snap.peers.iter().find(|p| p.id == selected) {
            if let Some(item) = peers.iter().find(|p| p.id.as_str() == selected) {
                w.set_peer(item.clone());
            }
            let mut tiles = Vec::new();
            for rp in &sp.remote_permissions {
                if rp.capability.local_only() {
                    continue;
                }
                let cap = rp.capability.as_str();
                tiles.push(TileItem {
                    cap: cap.into(),
                    label: l.invoke_tile_label(cap.into()),
                    icon: icon(cap),
                    mode: mode_index(rp.mode),
                });
                if rp.capability == Capability::Input {
                    tiles.push(TileItem {
                        cap: cap.into(),
                        label: l.invoke_tile_label("keyboard".into()),
                        icon: icon("keyboard"),
                        mode: mode_index(rp.mode),
                    });
                }
            }
            sync(&self.models.tiles, tiles);
        } else if w.get_page() == crate::ui::Page::Peer {
            w.set_page(crate::ui::Page::ThisDevice);
        }
        sync(&self.models.peers, peers);

        sync(
            &self.models.nearby,
            snap.nearby
                .iter()
                .filter(|n| !snap.code_pairings.iter().any(|c| c.key.starts_with(&n.key)))
                .map(|n| NearbyItem {
                    key: n.key.clone().into(),
                    name: n.name.clone().into(),
                    platform: platform_label(&n.platform).into(),
                    address: n.address.clone().into(),
                })
                .collect(),
        );
        sync(
            &self.models.codes,
            snap.code_pairings
                .iter()
                .map(|c| CodeItem {
                    key: c.key.clone().into(),
                    name: c.name.clone().into(),
                    code: format!("{} {}", &c.code[..3], &c.code[3..]).into(),
                    confirmed: c.confirmed_here,
                    incoming: c.incoming,
                })
                .collect(),
        );
        sync(
            &self.models.activities,
            snap.activities
                .iter()
                .map(|a| ActivityItem {
                    id: a.id.to_string().into(),
                    device: a.device_name.clone().into(),
                    label: l.invoke_cap_label(a.capability.clone().into()),
                    incoming: a.incoming,
                })
                .collect(),
        );

        match &snap.pair_screen {
            Some(p) => {
                if *self.qr_uri.borrow() != p.qr_uri {
                    *self.qr_uri.borrow_mut() = p.qr_uri.clone();
                    w.set_qr(qr_image(&p.qr_uri));
                }
                w.set_qr_countdown(
                    l.invoke_valid_for(i32::try_from(p.expires_in_secs).unwrap_or(0)),
                );
                let locked = if p.blocked_by_hidden {
                    l.invoke_hidden_blocks()
                } else if p.locked_secs > 0 {
                    l.invoke_locked_for(i32::try_from(p.locked_secs).unwrap_or(0))
                } else {
                    SharedString::new()
                };
                w.set_pair_locked(locked);
            }
            None => {
                self.qr_uri.borrow_mut().clear();
            }
        }
        *self.last_snapshot.borrow_mut() = Some(snap);
    }

    fn handle(self: &Rc<Self>, event: NodeEvent) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        let l = Self::labels(&w);
        match event {
            NodeEvent::Changed => self.refresh(),
            NodeEvent::PairingFinished { name, ok, reason } => {
                if ok {
                    self.toast(l.invoke_paired_with(name.clone().into()));
                    if w.get_onboarding() {
                        w.set_paired_name(name.into());
                        w.set_onboarding_step(6);
                    }
                } else {
                    self.toast(l.invoke_pairing_failed(reason.into()));
                }
                self.refresh();
            }
            NodeEvent::Prompt(p) => self.show_prompt(p),
            NodeEvent::PromptClosed { id } => {
                if let Some((win, _)) = self.prompts.borrow_mut().remove(&id) {
                    let _ = win.hide();
                }
            }
            NodeEvent::ActivityResult {
                device_name,
                capability,
                granted,
                reason,
                ..
            } => {
                let feature = l.invoke_cap_label(capability.into());
                self.toast(l.invoke_activity_result(
                    device_name.into(),
                    feature,
                    granted,
                    reason.into(),
                ));
            }
            NodeEvent::Notice { code, device_name } => {
                self.toast(l.invoke_notice(code.into(), device_name.into()));
            }
        }
    }

    fn show_prompt(self: &Rc<Self>, p: urdevice_core::node::PromptView) {
        let Some(w) = self.window.upgrade() else {
            return;
        };
        let Ok(win) = PromptWindow::new() else { return };
        win.set_device(p.device_name.into());
        win.set_feature(Self::labels(&w).invoke_cap_label(p.capability.into()));
        win.set_seconds_left(i32::try_from(p.timeout_secs).unwrap_or(30));
        let id = p.id;
        let node = self.node.clone();
        win.on_answer(move |allow| node.answer_prompt(id, allow));
        let timer = Timer::default();
        let weak = win.as_weak();
        timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
            if let Some(win) = weak.upgrade() {
                win.set_seconds_left((win.get_seconds_left() - 1).max(0));
            }
        });
        let _ = win.show();
        self.prompts.borrow_mut().insert(id, (win, timer));
    }

    fn wire(self: &Rc<Self>, w: &MainWindow) {
        let node = self.node.clone();
        let me = Rc::downgrade(self);

        {
            let me = me.clone();
            w.on_set_onboarding_mode(move |i, m| {
                let Some(ui) = me.upgrade() else { return };
                if let Some(p) = ui.onboarding_modes.borrow_mut().get_mut(i as usize) {
                    p.mode = mode_from_index(m);
                }
                ui.refresh();
            });
        }
        {
            let me = me.clone();
            w.on_complete_onboarding(move |name| {
                let Some(ui) = me.upgrade() else { return };
                let modes = ui.onboarding_modes.borrow().clone();
                if let Err(e) = ui.node.complete_onboarding(&name, &modes) {
                    tracing::warn!(error = %e, "onboarding save failed");
                }
            });
        }
        {
            let me = me.clone();
            w.on_set_permission(move |cap, m, confirmed| {
                let Some(ui) = me.upgrade() else { return };
                let Ok(cap) = cap.parse::<Capability>() else {
                    return;
                };
                if let Err(e) = ui.node.set_permission(cap, mode_from_index(m), confirmed) {
                    tracing::warn!(error = %e, "permission change failed");
                }
            });
        }
        let n = node.clone();
        w.on_open_pair(move || n.open_pair_screen());
        {
            let me = me.clone();
            w.on_close_pair(move || {
                if let Some(ui) = me.upgrade() {
                    ui.scanner.borrow_mut().take();
                    ui.node.close_pair_screen();
                }
            });
        }
        {
            let me = me.clone();
            w.on_scan_qr(move || {
                let Some(ui) = me.upgrade() else { return };
                let node = ui.node.clone();
                let weak = me.clone();
                let scanner = crate::camera::Scanner::open(move |text| {
                    if let Some(ui) = weak.upgrade() {
                        ui.scanner.borrow_mut().take();
                        if let Err(e) = node.scan_qr(&text) {
                            let msg = match e {
                                urdevice_core::CoreError::Pairing(r) => r,
                                _ => "invalid".into(),
                            };
                            if let Some(w) = ui.window.upgrade() {
                                ui.toast(Self::labels(&w).invoke_pairing_failed(msg.into()));
                            }
                        }
                    }
                });
                match scanner {
                    Ok(s) => *ui.scanner.borrow_mut() = Some(s),
                    Err(e) => {
                        tracing::warn!(error = %e, "camera unavailable");
                        ui.toast(e.to_string().into());
                    }
                }
            });
        }
        let n = node.clone();
        let m2 = me.clone();
        w.on_pair_nearby(move |key| {
            let failed = n.pair_nearby(&key).is_err();
            if let (true, Some(ui)) = (failed, m2.upgrade())
                && let Some(w) = ui.window.upgrade()
            {
                ui.toast(Self::labels(&w).invoke_pairing_failed("unreachable".into()));
            }
        });
        let n = node.clone();
        w.on_confirm_code(move |key, ok| {
            let _ = n.confirm_code(&key, ok);
        });
        {
            let me = me.clone();
            w.on_select_peer(move |id| {
                if let (Some(ui), Some(w)) =
                    (me.upgrade(), me.upgrade().and_then(|u| u.window.upgrade()))
                {
                    w.set_selected_peer(id);
                    ui.refresh();
                }
            });
        }
        let n = node.clone();
        w.on_disconnect(move |id| {
            let _ = n.disconnect(&id);
        });
        let n = node.clone();
        w.on_reconnect(move |id| {
            let _ = n.reconnect(&id);
        });
        let n = node.clone();
        w.on_forget(move |id| {
            let _ = n.forget(&id);
        });
        let n = node.clone();
        w.on_request(move |id, cap| {
            if let Ok(cap) = cap.parse::<Capability>() {
                let _ = n.request_activity(&id, cap);
            }
        });
        let n = node.clone();
        w.on_stop_activity(move |id| {
            if let Ok(id) = id.parse() {
                n.stop_activity(id);
            }
        });
        let n = node.clone();
        w.on_set_name(move |name| {
            let _ = n.set_name(&name);
        });
        let n = node.clone();
        w.on_set_hidden(move |v| {
            let _ = n.set_hidden(v);
        });
        {
            let me = me.clone();
            w.on_set_autostart(move |on| {
                let Some(a) = autostart() else { return };
                let r = if on { a.enable() } else { a.disable() };
                if let Err(e) = r {
                    tracing::warn!(error = %e, "start at login change failed");
                    if let Some(ui) = me.upgrade() {
                        ui.toast(e.to_string().into());
                    }
                }
            });
        }
        {
            let me = me.clone();
            w.on_reset_identity(move || {
                let Some(ui) = me.upgrade() else { return };
                if let Err(e) = ui.node.reset_identity() {
                    ui.toast(e.to_string().into());
                }
            });
        }
        {
            let me = me.clone();
            w.on_delete_all(move || {
                let Some(ui) = me.upgrade() else { return };
                if let Err(e) = ui.node.delete_all_data() {
                    ui.toast(e.to_string().into());
                    return;
                }
                if let Some(w) = ui.window.upgrade() {
                    w.set_onboarding(true);
                    w.set_onboarding_step(1);
                }
                ui.refresh();
            });
        }
        {
            let me = me.clone();
            w.on_export_data(move || {
                let Some(ui) = me.upgrade() else { return };
                let text = match ui.node.export_data() {
                    Ok(t) => t,
                    Err(e) => return ui.toast(e.to_string().into()),
                };
                let dir = directories::UserDirs::new()
                    .and_then(|u| u.download_dir().map(|d| d.to_path_buf()))
                    .unwrap_or_else(urdevice_platform::data_dir);
                let path = dir.join(format!(
                    "urdevice-export-{}.txt",
                    jiff::Zoned::now().strftime("%Y%m%d-%H%M%S")
                ));
                match std::fs::write(&path, text) {
                    Ok(()) => {
                        if let Some(w) = ui.window.upgrade() {
                            ui.toast(
                                Self::labels(&w).invoke_exported(path.display().to_string().into()),
                            );
                        }
                    }
                    Err(e) => ui.toast(e.to_string().into()),
                }
            });
        }
        {
            let me = me.clone();
            w.on_show_log(move || {
                let Some(ui) = me.upgrade() else { return };
                let Some(w) = ui.window.upgrade() else { return };
                let l = Self::labels(&w);
                let rows = ui.node.activity_log(500).unwrap_or_default();
                w.set_log(model(
                    rows.into_iter()
                        .map(|e| LogItem {
                            device: e.device_name.into(),
                            label: l.invoke_cap_label(e.capability.into()),
                            decision_label: l.invoke_decision(e.decision.clone().into()),
                            decision: e.decision.into(),
                            time: local_time(e.ts).into(),
                        })
                        .collect(),
                ));
            });
        }
    }
}
