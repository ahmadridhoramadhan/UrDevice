//! File transfer (#17): bindings for the Android app.
//!
//! Kotlin gives the core a [`FilesHostFfi`] (the private incoming folder and
//! the move to MediaStore Downloads, R15.3) and a [`FilesListener`]. Files to
//! send come from the share sheet or the picker as content URIs; Kotlin opens
//! them on request through [`FileSourceFfi`] and hands over a file
//! descriptor, so the core streams them without copying.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use urdevice_core::services::files::{
    Files, FilesEvents, FilesHost, Opener, Source, TransferState, TransferView,
};

use crate::{FfiError, UrDeviceNode, device_id};

/// The Android side of receiving.
#[uniffi::export(with_foreign)]
pub trait FilesHostFfi: Send + Sync {
    /// App-private folder where files arrive before they are published.
    fn incoming_dir(&self) -> String;
    /// A verified file is at `path`; publish it to Downloads/UrDevice under
    /// the cleaned folder `rel_dir` and return where it is shown, or null to
    /// keep it where it is.
    fn saved(&self, path: String, rel_dir: String) -> Option<String>;
}

/// Opens the files of one outgoing transfer.
#[uniffi::export(with_foreign)]
pub trait FileSourceFfi: Send + Sync {
    /// A readable file descriptor for file `index`, owned by the core from
    /// now on, or -1. Called again for a retry or a resume.
    fn open(&self, index: u32) -> i32;
}

/// Transfer list changes. Called on core threads.
#[uniffi::export(with_foreign)]
pub trait FilesListener: Send + Sync {
    /// Something in `files_transfers` changed (at most every 100 ms).
    fn on_changed(&self);
    /// A device started sending files here.
    fn on_incoming(&self, transfer: FfiTransfer);
}

/// One file to send.
#[derive(uniffi::Record)]
pub struct FfiSendFile {
    /// Display name, or `Folder/name` inside a folder.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
}

/// Where a transfer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiTransferState {
    Asking,
    Running,
    Interrupted,
    Done,
    Failed,
    Cancelled,
    Declined,
}

impl From<TransferState> for FfiTransferState {
    fn from(s: TransferState) -> Self {
        match s {
            TransferState::Asking => Self::Asking,
            TransferState::Running => Self::Running,
            TransferState::Interrupted => Self::Interrupted,
            TransferState::Done => Self::Done,
            TransferState::Failed => Self::Failed,
            TransferState::Cancelled => Self::Cancelled,
            TransferState::Declined => Self::Declined,
        }
    }
}

/// One transfer, for the UI and the progress notification.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiTransfer {
    pub id: u64,
    pub device_id: String,
    pub device_name: String,
    pub outgoing: bool,
    pub names: Vec<String>,
    pub total: u64,
    pub done: u64,
    pub speed: u64,
    /// Seconds left, or null.
    pub eta_secs: Option<u64>,
    pub state: FfiTransferState,
    pub reason: String,
    pub saved: Vec<String>,
    pub failed: u32,
}

impl From<&TransferView> for FfiTransfer {
    fn from(t: &TransferView) -> Self {
        Self {
            id: t.id,
            device_id: t.peer.hex(),
            device_name: t.peer_name.clone(),
            outgoing: t.outgoing,
            names: t.names.clone(),
            total: t.total,
            done: t.done,
            speed: t.speed,
            eta_secs: t.eta_secs,
            state: t.state.into(),
            reason: t.reason.clone(),
            saved: t.saved.clone(),
            failed: t.failed,
        }
    }
}

struct Host(Arc<dyn FilesHostFfi>);

impl FilesHost for Host {
    fn incoming_dir(&self) -> PathBuf {
        PathBuf::from(self.0.incoming_dir())
    }
    fn saved(&self, path: &Path, rel_dir: &str) -> Option<String> {
        self.0.saved(path.display().to_string(), rel_dir.to_owned())
    }
}

struct Listener(Arc<dyn FilesListener>);

impl FilesEvents for Listener {
    fn changed(&self) {
        self.0.on_changed();
    }
    fn incoming(&self, t: &TransferView) {
        self.0.on_incoming(t.into());
    }
}

/// Take ownership of a descriptor Kotlin detached for us.
#[cfg(unix)]
fn file_from_fd(fd: i32) -> std::io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "could not open",
        ));
    }
    // SAFETY: Kotlin passes a descriptor from
    // `ParcelFileDescriptor.detachFd()`, which it no longer owns or closes.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(not(unix))]
fn file_from_fd(_fd: i32) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::other("not available"))
}

impl UrDeviceNode {
    fn files(&self) -> Result<Arc<Files>, FfiError> {
        self.node.service::<Files>().ok_or(FfiError::NotFound)
    }
}

#[uniffi::export]
impl UrDeviceNode {
    /// Set the receiving adapter.
    pub fn files_set_host(&self, host: Arc<dyn FilesHostFfi>) -> Result<(), FfiError> {
        self.files()?.set_host(Arc::new(Host(host)));
        Ok(())
    }

    /// Set the transfer listener.
    pub fn files_set_listener(&self, l: Arc<dyn FilesListener>) -> Result<(), FfiError> {
        self.files()?.set_events(Arc::new(Listener(l)));
        Ok(())
    }

    /// Send files to a paired device (R8.1). Returns the transfer id.
    pub fn files_send(
        &self,
        device: String,
        files: Vec<FfiSendFile>,
        source: Arc<dyn FileSourceFfi>,
    ) -> Result<u64, FfiError> {
        let peer = device_id(&device)?;
        let sources = files
            .into_iter()
            .enumerate()
            .map(|(i, f)| {
                let src = source.clone();
                let index = i as u32;
                let open: Opener = Arc::new(move || file_from_fd(src.open(index)));
                Source {
                    rel_path: f.name,
                    size: f.size,
                    open,
                }
            })
            .collect();
        Ok(self.files()?.send(peer, sources)?)
    }

    /// Every transfer since the core started, newest first.
    pub fn files_transfers(&self) -> Vec<FfiTransfer> {
        self.files()
            .map(|f| f.transfers().iter().map(FfiTransfer::from).collect())
            .unwrap_or_default()
    }

    /// Cancel a transfer on both devices.
    pub fn files_cancel(&self, id: u64) {
        if let Ok(f) = self.files() {
            f.cancel(id);
        }
    }

    /// Send the unverified files of an interrupted or failed transfer again.
    pub fn files_retry(&self, id: u64) -> Result<(), FfiError> {
        Ok(self.files()?.retry(id)?)
    }

    /// Forget finished transfers.
    pub fn files_clear_finished(&self) {
        if let Ok(f) = self.files() {
            f.clear_finished();
        }
    }
}
