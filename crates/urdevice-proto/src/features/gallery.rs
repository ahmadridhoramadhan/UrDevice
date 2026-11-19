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

/// Controller → host (`ToHost`): one page of items, newest first.
#[derive(Clone, PartialEq, Message)]
pub struct GalleryList {
    /// 0 for the first page, then [`GalleryPage::next_cursor`].
    #[prost(uint64, tag = "1")]
    pub cursor: u64,
    /// Items wanted; the host may send fewer.
    #[prost(uint32, tag = "2")]
    pub limit: u32,
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
    #[prost(oneof = "gallery_msg::Msg", tags = "1, 2")]
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
        }));
        assert_eq!(GalleryMsg::decode(m.encode_to_vec().as_slice()).unwrap(), m);
    }
}
