//! Android gallery (#17, design §9.2): wire messages for `ServiceId::GALLERY`.
//!
//! Owned by the gallery feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/gallery.proto`.
//!
//! The controller (a desktop) asks for pages of items with
//! [`GalleryList`]; the host (the phone) answers with [`GalleryPage`]. A
//! download opens a [`STREAM_FETCH`] stream whose `meta` is a
//! [`GalleryFetch`]; the host answers on the same stream with the file data,
//! framed like a file transfer, in the order of [`GalleryFetch::items`].

use prost::Message;

/// Stream kind of a download. `meta` is a [`GalleryFetch`].
pub const STREAM_FETCH: u32 = 1;

/// Stream kind of a large preview. `meta` is a [`GalleryPreview`]. The host
/// answers with one JPEG (at most [`MAX_PREVIEW`] bytes) and finishes the stream.
pub const STREAM_PREVIEW: u32 = 2;

/// Largest preview the controller accepts, in bytes.
pub const MAX_PREVIEW: usize = 4 * 1024 * 1024;
/// Largest preview side the host makes, in pixels.
pub const MAX_PREVIEW_SIDE: u32 = 2048;
/// Most albums in one answer.
pub const MAX_ALBUMS: usize = 200;

/// Which items a list shows: [`KIND_ALL`], [`KIND_PHOTOS`], or [`KIND_VIDEOS`].
pub const KIND_ALL: u32 = 0;
/// Photos only.
pub const KIND_PHOTOS: u32 = 1;
/// Videos only.
pub const KIND_VIDEOS: u32 = 2;

/// Controller → host (`ToHost`): one page of items, newest first.
#[derive(Clone, PartialEq, Message)]
pub struct GalleryList {
    /// 0 for the first page, then [`GalleryPage::next_cursor`].
    #[prost(uint64, tag = "1")]
    pub cursor: u64,
    /// Items wanted; the host may send fewer.
    #[prost(uint32, tag = "2")]
    pub limit: u32,
    /// [`KIND_ALL`], [`KIND_PHOTOS`], or [`KIND_VIDEOS`].
    #[prost(uint32, tag = "3")]
    pub kind: u32,
    /// Only items of this album ([`GalleryAlbum::id`]); all albums when unset.
    #[prost(int64, optional, tag = "4")]
    pub album: Option<i64>,
    /// Echoed in [`GalleryPage::tag`], so the viewer can drop an answer to a
    /// list it no longer shows.
    #[prost(uint64, tag = "5")]
    pub tag: u64,
}

/// One album (a folder of pictures on the phone).
#[derive(Clone, PartialEq, Message)]
pub struct GalleryAlbum {
    /// The host's id for the album.
    #[prost(int64, tag = "1")]
    pub id: i64,
    /// Display name. Untrusted.
    #[prost(string, tag = "2")]
    pub name: String,
    /// Photos and videos in it.
    #[prost(uint64, tag = "3")]
    pub count: u64,
}

/// Controller → host: list the albums.
#[derive(Clone, PartialEq, Message)]
pub struct GalleryAlbumsReq {}

/// Host → controller: the albums, newest first, or why there are none.
#[derive(Clone, PartialEq, Message)]
pub struct GalleryAlbums {
    /// The albums.
    #[prost(message, repeated, tag = "1")]
    pub albums: Vec<GalleryAlbum>,
    /// Empty, or `permission` / `failed` / `busy`.
    #[prost(string, tag = "2")]
    pub error: String,
}

/// The `meta` of a [`STREAM_PREVIEW`] stream.
#[derive(Clone, PartialEq, Message)]
pub struct GalleryPreview {
    /// [`GalleryItem::id`].
    #[prost(uint64, tag = "1")]
    pub id: u64,
    /// Longest side wanted, in pixels. The host clamps it to [`MAX_PREVIEW_SIDE`].
    #[prost(uint32, tag = "2")]
    pub max_side: u32,
}

/// One photo or video.
#[derive(Clone, PartialEq, Message)]
pub struct GalleryItem {
    /// The host's id for the item.
    #[prost(uint64, tag = "1")]
    pub id: u64,
    /// File name. Untrusted.
    #[prost(string, tag = "2")]
    pub name: String,
    /// MIME type, for example `image/jpeg`.
    #[prost(string, tag = "3")]
    pub mime: String,
    /// Size in bytes.
    #[prost(uint64, tag = "4")]
    pub size: u64,
    /// When it was taken or added, Unix milliseconds.
    #[prost(int64, tag = "5")]
    pub taken_ms: i64,
    /// Video length in milliseconds, 0 for photos.
    #[prost(uint64, tag = "6")]
    pub duration_ms: u64,
    /// A JPEG thumbnail of at most 256 px per side.
    #[prost(bytes = "vec", tag = "7")]
    pub thumb: Vec<u8>,
}

/// Host → controller (`ToController`): a page of items.
#[derive(Clone, PartialEq, Message)]
pub struct GalleryPage {
    /// The cursor this page answers.
    #[prost(uint64, tag = "1")]
    pub cursor: u64,
    /// The items.
    #[prost(message, repeated, tag = "2")]
    pub items: Vec<GalleryItem>,
    /// Cursor of the next page, 0 at the end.
    #[prost(uint64, tag = "3")]
    pub next_cursor: u64,
    /// Empty, or why the page is empty: `permission` when the phone has not
    /// given UrDevice access to its photos, `failed` otherwise.
    #[prost(string, tag = "4")]
    pub error: String,
    /// The [`GalleryList::tag`] this page answers.
    #[prost(uint64, tag = "5")]
    pub tag: u64,
}

/// One item to download.
#[derive(Clone, PartialEq, Message)]
pub struct FetchItem {
    /// [`GalleryItem::id`].
    #[prost(uint64, tag = "1")]
    pub id: u64,
    /// Bytes the controller already has from an earlier attempt.
    #[prost(uint64, tag = "2")]
    pub offset: u64,
    /// Size the controller expects ([`GalleryItem::size`]); an item that
    /// changed since it was listed fails instead of overrunning the plan.
    #[prost(uint64, tag = "3")]
    pub size: u64,
}

/// The `meta` of a [`STREAM_FETCH`] stream.
#[derive(Clone, PartialEq, Message)]
pub struct GalleryFetch {
    /// The items, in the order the controller wants them.
    #[prost(message, repeated, tag = "1")]
    pub items: Vec<FetchItem>,
}

/// The envelope of every gallery control message.
#[derive(Clone, PartialEq, Message)]
pub struct GalleryMsg {
    /// The message.
    #[prost(oneof = "gallery_msg::Msg", tags = "1, 2, 3, 4")]
    pub msg: Option<gallery_msg::Msg>,
}

/// The messages in [`GalleryMsg`].
pub mod gallery_msg {
    /// One gallery control message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Msg {
        /// See [`super::GalleryList`].
        #[prost(message, tag = "1")]
        List(super::GalleryList),
        /// See [`super::GalleryPage`].
        #[prost(message, tag = "2")]
        Page(super::GalleryPage),
        /// See [`super::GalleryAlbumsReq`].
        #[prost(message, tag = "3")]
        AlbumsReq(super::GalleryAlbumsReq),
        /// See [`super::GalleryAlbums`].
        #[prost(message, tag = "4")]
        Albums(super::GalleryAlbums),
    }
}

impl From<gallery_msg::Msg> for GalleryMsg {
    fn from(m: gallery_msg::Msg) -> Self {
        Self { msg: Some(m) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_round_trips() {
        let m = GalleryMsg::from(gallery_msg::Msg::Page(GalleryPage {
            cursor: 0,
            items: vec![GalleryItem {
                id: 1,
                name: "IMG_1.jpg".into(),
                mime: "image/jpeg".into(),
                size: 10,
                taken_ms: 5,
                duration_ms: 0,
                thumb: vec![0xff, 0xd8],
            }],
            next_cursor: 0,
            error: String::new(),
            tag: 7,
        }));
        assert_eq!(GalleryMsg::decode(m.encode_to_vec().as_slice()).unwrap(), m);
    }

    #[test]
    fn list_and_albums_round_trip() {
        let l = GalleryMsg::from(gallery_msg::Msg::List(GalleryList {
            cursor: 3,
            limit: 60,
            kind: KIND_VIDEOS,
            album: Some(-42),
            tag: 9,
        }));
        assert_eq!(GalleryMsg::decode(l.encode_to_vec().as_slice()).unwrap(), l);
        let a = GalleryMsg::from(gallery_msg::Msg::Albums(GalleryAlbums {
            albums: vec![GalleryAlbum {
                id: -1,
                name: "Camera".into(),
                count: 4,
            }],
            error: String::new(),
        }));
        assert_eq!(GalleryMsg::decode(a.encode_to_vec().as_slice()).unwrap(), a);
    }
}
