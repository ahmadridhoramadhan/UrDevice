//! Android gallery (#17): bindings for the Android app.
//!
//! The phone is the gallery host (R8.7): Kotlin reads MediaStore, makes the
//! 256-px JPEG thumbnails, and opens items for download.

use std::sync::Arc;

use urdevice_core::CoreError;
use urdevice_core::services::gallery::{Filter, Gallery, GalleryHost, HostPage};
use urdevice_proto::features::gallery::{GalleryAlbum, GalleryItem};

use crate::{FfiError, UrDeviceNode};

/// One photo or video from MediaStore.
#[derive(uniffi::Record)]
pub struct FfiGalleryItem {
    pub id: u64,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub taken_ms: i64,
    pub duration_ms: u64,
    /// JPEG, at most 256 px per side; empty when there is none.
    pub thumb: Vec<u8>,
}

/// One album (a MediaStore bucket).
#[derive(uniffi::Record)]
pub struct FfiGalleryAlbum {
    pub id: i64,
    pub name: String,
    pub count: u64,
}

/// A page, or why there is none.
#[derive(uniffi::Record)]
pub struct FfiGalleryPage {
    pub items: Vec<FfiGalleryItem>,
    pub more: bool,
    /// True when the app may not read photos and videos.
    pub no_permission: bool,
}

/// An opened item.
#[derive(uniffi::Record)]
pub struct FfiOpened {
    /// A readable descriptor from `ParcelFileDescriptor.detachFd()`, owned by
    /// the core, or -1.
    pub fd: i32,
    pub size: u64,
}

/// MediaStore, in Kotlin. Called on core threads.
#[uniffi::export(with_foreign)]
pub trait GalleryHostFfi: Send + Sync {
    /// Items `cursor..cursor + limit` of the filtered list, newest first. `kind` is 0 for
    /// photos and videos, 1 for photos, 2 for videos. `album` is a bucket id, or null for all.
    fn page(&self, cursor: u64, limit: u32, kind: u32, album: Option<i64>) -> FfiGalleryPage;
    /// Open item `id`.
    fn open(&self, id: u64) -> FfiOpened;
    /// The albums, newest first.
    fn albums(&self) -> Vec<FfiGalleryAlbum>;
    /// A JPEG of item `id`, at most `max_side` pixels on its longest side; empty when it cannot
    /// be made.
    fn preview(&self, id: u64, max_side: u32) -> Vec<u8>;
}

struct Host(Arc<dyn GalleryHostFfi>);

impl GalleryHost for Host {
    fn page(&self, cursor: u64, limit: u32, filter: &Filter) -> urdevice_core::Result<HostPage> {
        let p = self.0.page(cursor, limit, filter.kind, filter.album);
        if p.no_permission {
            return Err(CoreError::Refused("permission".into()));
        }
        Ok(HostPage {
            items: p
                .items
                .into_iter()
                .map(|i| GalleryItem {
                    id: i.id,
                    name: i.name,
                    mime: i.mime,
                    size: i.size,
                    taken_ms: i.taken_ms,
                    duration_ms: i.duration_ms,
                    thumb: i.thumb,
                })
                .collect(),
            more: p.more,
        })
    }

    fn open(&self, id: u64) -> urdevice_core::Result<(std::fs::File, u64)> {
        let o = self.0.open(id);
        Ok((file_from_fd(o.fd)?, o.size))
    }

    fn albums(&self) -> urdevice_core::Result<Vec<GalleryAlbum>> {
        Ok(self
            .0
            .albums()
            .into_iter()
            .map(|a| GalleryAlbum {
                id: a.id,
                name: a.name,
                count: a.count,
            })
            .collect())
    }

    fn preview(&self, id: u64, max_side: u32) -> urdevice_core::Result<Vec<u8>> {
        let b = self.0.preview(id, max_side);
        if b.is_empty() {
            return Err(CoreError::Refused("failed".into()));
        }
        Ok(b)
    }
}

#[cfg(unix)]
fn file_from_fd(fd: i32) -> std::io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "could not open",
        ));
    }
    // SAFETY: Kotlin passes a descriptor from `detachFd()`, which it no
    // longer owns or closes.
    Ok(unsafe { std::fs::File::from_raw_fd(fd) })
}

#[cfg(not(unix))]
fn file_from_fd(_fd: i32) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::other("not available"))
}

#[uniffi::export]
impl UrDeviceNode {
    /// Serve this phone's photos and videos to paired desktops.
    pub fn gallery_set_host(&self, host: Arc<dyn GalleryHostFfi>) -> Result<(), FfiError> {
        let g = self.node.service::<Gallery>().ok_or(FfiError::NotFound)?;
        g.host.set(Arc::new(Host(host)));
        Ok(())
    }
}
