//! Clipboard sharing (#16, design §9.1): wire messages for `ServiceId::CLIPBOARD`.
//!
//! Owned by the clipboard feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/clipboard.proto`.

use prost::Message;

/// Largest clipboard item, in bytes (R7.2).
pub const MAX_CLIP_BYTES: usize = 10 * 1024 * 1024;

/// Largest item sent in a control message. Bigger items go over a stream
/// (design §9.1). Well under the 1 MiB control frame, so a `Clip` with its
/// plain-text fallback always fits.
pub const MAX_INLINE_BYTES: usize = 512 * 1024;

/// Largest plain-text fallback that travels with an HTML item.
pub const MAX_ALT_TEXT_BYTES: usize = 256 * 1024;

/// Stream kind: one clipboard item. The stream starts with the item's
/// [`ClipHeader`] in `StreamOpen::meta`; the item's bytes are the stream body.
pub const STREAM_CLIP: u32 = 1;

/// The MIME types a clipboard item may have (R7.2).
pub mod mime {
    /// Plain text, UTF-8.
    pub const TEXT: &str = "text/plain";
    /// Rich text.
    pub const HTML: &str = "text/html";
    /// An image.
    pub const PNG: &str = "image/png";
}

/// One clipboard item sent in a control message (design §9.1).
///
/// Everything in it is untrusted on arrival: the receiver checks the MIME
/// type against [`mime`], the size against [`MAX_CLIP_BYTES`], and that a PNG
/// is a PNG.
#[derive(Clone, PartialEq, Message)]
pub struct Clip {
    /// Random item id (16 bytes). Lets the receiver drop a repeat.
    #[prost(bytes = "vec", tag = "1")]
    pub id: Vec<u8>,
    /// One of [`mime`].
    #[prost(string, tag = "2")]
    pub mime: String,
    /// The content in that format.
    #[prost(bytes = "vec", tag = "3")]
    pub bytes: Vec<u8>,
    /// Device ID of the device the content was copied on (32 bytes, or empty).
    /// For display only; the receiver trusts the session, not this field.
    #[prost(bytes = "vec", tag = "4")]
    pub origin_device: Vec<u8>,
    /// Plain-text version of an HTML item, for apps that paste only text.
    #[prost(string, tag = "5")]
    pub alt_text: String,
}

/// Describes an item that follows on a stream ([`STREAM_CLIP`]), carried in
/// `StreamOpen::meta`.
#[derive(Clone, PartialEq, Message)]
pub struct ClipHeader {
    /// Random item id (16 bytes).
    #[prost(bytes = "vec", tag = "1")]
    pub id: Vec<u8>,
    /// One of [`mime`].
    #[prost(string, tag = "2")]
    pub mime: String,
    /// Exact number of bytes in the stream body.
    #[prost(uint64, tag = "3")]
    pub size: u64,
    /// SHA-256 of the body (32 bytes), so a cut stream is never written.
    #[prost(bytes = "vec", tag = "4")]
    pub sha256: Vec<u8>,
    /// Device ID of the device the content was copied on, as in [`Clip`].
    #[prost(bytes = "vec", tag = "5")]
    pub origin_device: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_round_trips() {
        let c = Clip {
            id: vec![1; 16],
            mime: mime::HTML.into(),
            bytes: b"<b>x</b>".to_vec(),
            origin_device: vec![2; 32],
            alt_text: "x".into(),
        };
        assert_eq!(Clip::decode(c.encode_to_vec().as_slice()).unwrap(), c);
    }

    #[test]
    fn header_round_trips_and_fits_a_stream_header() {
        let h = ClipHeader {
            id: vec![1; 16],
            mime: mime::PNG.into(),
            size: MAX_CLIP_BYTES as u64,
            sha256: vec![3; 32],
            origin_device: vec![2; 32],
        };
        let b = h.encode_to_vec();
        assert!(b.len() < crate::MAX_STREAM_OPEN);
        assert_eq!(ClipHeader::decode(b.as_slice()).unwrap(), h);
    }

    #[test]
    fn inline_item_with_alt_text_fits_a_control_frame() {
        const { assert!(MAX_INLINE_BYTES + MAX_ALT_TEXT_BYTES + 256 < crate::MAX_FRAME) };
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(Clip::decode(&[0xff, 0xff, 0xff][..]).is_err());
    }
}
