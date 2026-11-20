//! File transfer (#17): the desktop side.
//!
//! The view is `ui/features/files.slint` (global `FilesFeature`): file and
//! folder pickers, drag-and-drop onto the window, the transfer list with
//! progress, and the download folder in Settings (R8.1, R8.3, R8.6). See
//! `docs/dev/adding-a-feature.md`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use urdevice_core::Capability;
use urdevice_core::identity::DeviceId;
use urdevice_core::node::PeerView;
use urdevice_core::services::files::{
    Files as FilesService, FilesEvents, TransferState, TransferView,
};
use urdevice_platform::features::files::{DesktopFiles, open_folder};

use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::{FilesFeature, MainWindow, Page, TransferRow};

pub(crate) struct Files;

/// `1.5 MB`, `320 KB`, `12 B`.
pub(crate) fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else if v < 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.0} {}", UNITS[u])
    }
}

fn state_index(s: TransferState) -> i32 {
    match s {
        TransferState::Asking => 0,
        TransferState::Running => 1,
        TransferState::Interrupted => 2,
        TransferState::Done => 3,
        TransferState::Failed => 4,
        TransferState::Cancelled => 5,
        TransferState::Declined => 6,
    }
}

fn row(t: &TransferView) -> TransferRow {
    let first = t
        .names
        .first()
        .map(|n| n.rsplit('/').next().unwrap_or(n).to_owned())
        .unwrap_or_default();
    let saved = match t.saved.as_slice() {
        [one] => one.clone(),
        many => many
            .first()
            .and_then(|p| PathBuf::from(p).parent().map(|d| d.display().to_string()))
            .unwrap_or_default(),
    };
    TransferRow {
        id: t.id as i32,
        outgoing: t.outgoing,
        device: t.peer_name.clone().into(),
        first: first.into(),
        more: t.names.len().saturating_sub(1) as i32,
        done_text: human(t.done).into(),
        total_text: human(t.total).into(),
        speed_text: human(t.speed).into(),
        eta: t
            .eta_secs
            .map(|s| s.min(i32::MAX as u64) as i32)
            .unwrap_or(-1),
        progress: if t.total == 0 {
            if t.state == TransferState::Done {
                1.0
            } else {
                0.0
            }
        } else {
            t.done as f32 / t.total as f32
        },
        state: state_index(t.state),
        reason: t.reason.clone().into(),
        failed: t.failed as i32,
        saved: saved.into(),
    }
}

struct Ui {
    svc: Arc<FilesService>,
    host: Arc<DesktopFiles>,
    rows: Rc<VecModel<TransferRow>>,
    dropped: RefCell<Vec<PathBuf>>,
    drop_timer: slint::Timer,
}

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

fn with_ui(f: impl FnOnce(&Rc<Ui>)) {
    if let Some(ui) = UI.with(|u| u.borrow().clone()) {
        f(&ui);
    }
}

impl Ui {
    fn redraw(&self) {
        let rows: Vec<TransferRow> = self.svc.transfers().iter().map(row).collect();
        if self.rows.row_count() != rows.len() {
            self.rows.set_vec(rows);
            return;
        }
        for (i, r) in rows.into_iter().enumerate() {
            if self.rows.row_data(i).as_ref() != Some(&r) {
                self.rows.set_row_data(i, r);
            }
        }
    }
}

/// Core events arrive on core threads; redraw on the Slint thread.
struct Events;

impl FilesEvents for Events {
    fn changed(&self) {
        let _ = slint::invoke_from_event_loop(|| with_ui(|ui| ui.redraw()));
    }
}

/// The device whose Files view is open, if any.
fn open_peer(w: &MainWindow) -> Option<DeviceId> {
    if w.get_page() != Page::Feature || w.get_feature() != "files" {
        return None;
    }
    DeviceId::from_hex(&w.global::<FilesFeature>().get_peer_id())
}

fn send(w: &MainWindow, svc: &Arc<FilesService>, paths: Vec<PathBuf>) {
    let Some(peer) = DeviceId::from_hex(&w.global::<FilesFeature>().get_peer_id()) else {
        return;
    };
    if paths.is_empty() {
        return;
    }
    // Reading folders can take a moment; do it off the UI thread.
    let svc = svc.clone();
    std::thread::spawn(move || {
        if let Err(e) = svc.send_paths(peer, &paths) {
            tracing::warn!(error = %e, "could not start the transfer");
        }
    });
}

/// Run a file dialog without blocking the window (Windows, Linux portal),
/// then hand the result to `done` on the Slint thread. macOS dialogs must
/// run on the main thread, so there it blocks while open.
fn dialog(folder: bool, done: impl FnOnce(Vec<PathBuf>) + Send + 'static) {
    let run = move || -> Vec<PathBuf> {
        let d = rfd::FileDialog::new();
        if folder {
            d.pick_folder().into_iter().collect()
        } else {
            d.pick_files().unwrap_or_default()
        }
    };
    if cfg!(target_os = "macos") {
        done(run());
    } else {
        std::thread::spawn(move || {
            let picked = run();
            let _ = slint::invoke_from_event_loop(move || done(picked));
        });
    }
}

impl Feature for Files {
    fn id(&self) -> &'static str {
        "files"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Files,
            key: "files.v1",
        }]
    }

    fn has_view(&self) -> bool {
        true
    }

    fn start(&self, ctx: &FeatureCtx) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        let Some(svc) = ctx.node.service::<FilesService>() else {
            return;
        };
        let host = Arc::new(DesktopFiles::new(&urdevice_platform::data_dir()));
        svc.set_host(host.clone());
        svc.set_events(Arc::new(Events));
        let rows = Rc::new(VecModel::default());
        let ui = Rc::new(Ui {
            svc,
            host,
            rows: rows.clone(),
            dropped: RefCell::new(Vec::new()),
            drop_timer: slint::Timer::default(),
        });
        UI.with(|u| *u.borrow_mut() = Some(ui.clone()));

        let g = w.global::<FilesFeature>();
        g.set_transfers(ModelRc::from(rows));
        g.set_folder(ui.host.dir().display().to_string().into());
        let weak = ctx.window.clone();
        g.on_pick_files(move || {
            let weak = weak.clone();
            dialog(false, move |paths| {
                if let Some(w) = weak.upgrade() {
                    with_ui(|ui| send(&w, &ui.svc, paths));
                }
            });
        });
        let weak = ctx.window.clone();
        g.on_pick_folder(move || {
            let weak = weak.clone();
            dialog(true, move |paths| {
                if let Some(w) = weak.upgrade() {
                    with_ui(|ui| send(&w, &ui.svc, paths));
                }
            });
        });
        g.on_cancel(|id| with_ui(|ui| ui.svc.cancel(id as u64)));
        g.on_retry(|id| {
            with_ui(|ui| {
                let _ = ui.svc.retry(id as u64);
            });
        });
        g.on_clear(|| with_ui(|ui| ui.svc.clear_finished()));
        g.on_open_folder(|| with_ui(|ui| open_folder(&ui.host.dir())));
        let weak = ctx.window.clone();
        g.on_change_folder(move || {
            let weak = weak.clone();
            dialog(true, move |paths| {
                let (Some(w), Some(dir)) = (weak.upgrade(), paths.into_iter().next()) else {
                    return;
                };
                with_ui(|ui| match ui.host.set_dir(&dir) {
                    Ok(()) => w
                        .global::<FilesFeature>()
                        .set_folder(SharedString::from(dir.display().to_string())),
                    Err(e) => tracing::warn!(error = %e, "download folder not changed"),
                });
            });
        });

        // Drag-and-drop from the file manager onto the window (R8.1). winit
        // sends one event per file; collect them briefly into one transfer.
        // winit has no drop support on Wayland, so there the pickers are the
        // way (see the PR notes).
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
        let weak = ctx.window.clone();
        w.window().on_winit_window_event(move |_, ev| {
            if let winit::event::WindowEvent::DroppedFile(path) = ev {
                let path = path.clone();
                let weak = weak.clone();
                with_ui(move |ui| {
                    ui.dropped.borrow_mut().push(path);
                    ui.drop_timer.start(
                        slint::TimerMode::SingleShot,
                        Duration::from_millis(150),
                        move || {
                            let Some(w) = weak.upgrade() else { return };
                            with_ui(|ui| {
                                let paths = std::mem::take(&mut *ui.dropped.borrow_mut());
                                if open_peer(&w).is_some() {
                                    send(&w, &ui.svc, paths);
                                }
                            });
                        },
                    );
                });
            }
            EventResult::Propagate
        });
        ui.redraw();
    }

    fn open(&self, ctx: &FeatureCtx, peer: &PeerView, _tile: &str) {
        if let Some(w) = ctx.window.upgrade() {
            w.global::<FilesFeature>()
                .set_peer_id(peer.id.clone().into());
        }
        with_ui(|ui| ui.redraw());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes_read_well() {
        assert_eq!(super::human(12), "12 B");
        assert_eq!(super::human(1_500_000), "1.5 MB");
        assert_eq!(super::human(320_000), "320 KB");
        assert_eq!(super::human(25_000_000_000), "25 GB");
    }
}
