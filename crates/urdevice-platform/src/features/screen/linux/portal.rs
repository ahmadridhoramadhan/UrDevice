//! The xdg-desktop-portal ScreenCast session (spike T0.6).
//!
//! The session lives on its own thread with a small tokio runtime, because
//! the portal closes a session when the D-Bus connection that made it goes
//! away. The thread asks for one monitor with the cursor embedded, opens the
//! PipeWire remote, and keeps the session until Stop or until the user ends
//! it from the desktop's own indicator.
//!
//! `persist_mode` is "while the application runs": the dialog shows once per
//! app run, and the restore token stays in memory only, so no secret is
//! written to disk (spike T0.6 notes the token is a secret).

use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, OnceLock};

use ashpd::desktop::PersistMode;
use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
use futures_util::StreamExt as _;
use urdevice_core::Result;

use super::super::{CaptureHandle, FrameHandler, refused};
use super::pipewire;

/// The newest restore token, for this run of the app only.
static RESTORE: Mutex<Option<String>> = Mutex::new(None);

fn restore_token() -> Option<String> {
    match RESTORE.lock() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

fn save_token(t: Option<&str>) {
    let mut g = match RESTORE.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    *g = t.map(str::to_owned);
}

/// What the portal thread reports once the session started.
struct Started {
    fd: OwnedFd,
    node: u32,
    size: Option<(i32, i32)>,
}

fn reason(e: &ashpd::Error) -> &'static str {
    match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => "consent_declined",
        _ => "capture_failed",
    }
}

async fn open() -> std::result::Result<
    (
        Screencast<'static>,
        ashpd::desktop::Session<'static, Screencast<'static>>,
        Started,
    ),
    &'static str,
> {
    let proxy = Screencast::new().await.map_err(|e| reason(&e))?;
    let session = proxy.create_session().await.map_err(|e| reason(&e))?;
    let token = restore_token();
    proxy
        .select_sources(
            &session,
            CursorMode::Embedded,
            SourceType::Monitor.into(),
            false,
            token.as_deref(),
            PersistMode::Application,
        )
        .await
        .map_err(|e| reason(&e))?
        .response()
        .map_err(|e| reason(&e))?;
    let streams = proxy
        .start(&session, None)
        .await
        .map_err(|e| reason(&e))?
        .response()
        .map_err(|e| reason(&e))?;
    save_token(streams.restore_token());
    let stream = streams.streams().first().ok_or("no_display")?;
    let (node, size) = (stream.pipe_wire_node_id(), stream.size());
    let fd = proxy
        .open_pipe_wire_remote(&session)
        .await
        .map_err(|e| reason(&e))?;
    Ok((proxy, session, Started { fd, node, size }))
}

/// The one runtime that runs every portal session, for the life of the process.
///
/// ashpd keeps a single D-Bus connection in a static, and with the `tokio` feature that
/// connection's reader task lives on the runtime that created it. A runtime made per capture
/// died with its thread, so the second capture talked to a dead connection and waited forever
/// (a black screen on the viewer). This runtime is made once and never stops.
fn runtime() -> std::result::Result<tokio::runtime::Handle, &'static str> {
    static RT: OnceLock<std::result::Result<tokio::runtime::Handle, &'static str>> =
        OnceLock::new();
    RT.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("screencast-portal".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => {
                        let _ = tx.send(Ok(rt.handle().clone()));
                        rt.block_on(std::future::pending::<()>());
                    }
                    Err(_) => {
                        let _ = tx.send(Err("capture_failed"));
                    }
                }
            })
            .map_err(|_| "capture_failed")?;
        rx.recv().map_err(|_| "capture_failed")?
    })
    .clone()
}

pub(super) fn start(fps: u32, handler: Arc<dyn FrameHandler>) -> Result<CaptureHandle> {
    let (ready_tx, ready_rx) =
        std::sync::mpsc::channel::<std::result::Result<Started, &'static str>>();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let ended = handler.clone();
    let rt = runtime().map_err(refused)?;
    rt.spawn(async move {
        let (proxy, session, started) = match open().await {
            Ok(v) => v,
            Err(r) => {
                let _ = ready_tx.send(Err(r));
                return;
            }
        };
        let _ = ready_tx.send(Ok(started));
        drop(proxy);
        let closed = session.receive_closed().await;
        let mut user_closed = false;
        match closed {
            Ok(mut closed) => {
                tokio::select! {
                    _ = stop_rx => {}
                    _ = closed.next() => user_closed = true,
                }
            }
            Err(_) => {
                let _ = stop_rx.await;
            }
        }
        let _ = session.close().await;
        if user_closed {
            ended.ended("stopped");
        }
    });

    let started = match ready_rx.recv() {
        Ok(Ok(s)) => s,
        Ok(Err(r)) => return Err(refused(r)),
        Err(_) => return Err(refused("capture_failed")),
    };
    tracing::info!(node = started.node, "screencast portal session started");
    let size = started
        .size
        .and_then(|(w, h)| Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?)));
    let stream = match pipewire::Capture::start(started.fd, started.node, fps, size, handler) {
        Ok(s) => s,
        Err(e) => {
            let _ = stop_tx.send(());
            return Err(e);
        }
    };
    Ok(CaptureHandle::new(move || {
        drop(stream);
        let _ = stop_tx.send(());
    }))
}
