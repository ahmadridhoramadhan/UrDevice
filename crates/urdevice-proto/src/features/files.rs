//! File transfer (#17, design §9.2): wire messages for `ServiceId::FILES`.
//!
//! Owned by the files feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/files.proto`.
//!
//! Control messages travel in [`FilesMsg`]. The file bytes travel on one
//! bidirectional stream per transfer ([`STREAM_DATA`]), framed as described
//! in `urdevice_core::services::files::wire`.

use prost::Message;

/// Stream kind of the data stream of one transfer. `meta` is the transfer id
/// as 8 big-endian bytes.
pub const STREAM_DATA: u32 = 1;

/// One file in an offer. `rel_path` uses `/` between folders and is
/// untrusted: the receiver cleans it (R8.6).
#[derive(Clone, PartialEq, Message)]
pub struct FileEntry {
    /// Path relative to the transfer root, for example `Photos/a.jpg`.
    #[prost(string, tag = "1")]
    pub rel_path: String,
    /// Size in bytes.
    #[prost(uint64, tag = "2")]
    pub size: u64,
}

/// Sender → receiver (`ToHost`): the files of a transfer (R8.2).
#[derive(Clone, PartialEq, Message)]
pub struct Offer {
    /// Picked by the sender; the same id again means "resume" (R8.5).
    #[prost(uint64, tag = "1")]
    pub transfer_id: u64,
    /// The files, in the order their data follows.
    #[prost(message, repeated, tag = "2")]
    pub files: Vec<FileEntry>,
    /// Sum of the sizes.
    #[prost(uint64, tag = "3")]
    pub total_size: u64,
}

/// Where the receiver already has verified bytes of a file.
#[derive(Clone, PartialEq, Message)]
pub struct ResumeAt {
    /// Index in [`Offer::files`].
    #[prost(uint32, tag = "1")]
    pub index: u32,
    /// Bytes the receiver already has.
    #[prost(uint64, tag = "2")]
    pub offset: u64,
}

/// Receiver → sender (`ToController`): send the data, starting at these
/// offsets (files not listed start at 0).
#[derive(Clone, PartialEq, Message)]
pub struct Accept {
    /// The offer this answers.
    #[prost(uint64, tag = "1")]
    pub transfer_id: u64,
    /// Resume points from earlier attempts.
    #[prost(message, repeated, tag = "2")]
    pub resume: Vec<ResumeAt>,
}

/// Receiver → sender (`ToController`): the offer was not taken.
#[derive(Clone, PartialEq, Message)]
pub struct Decline {
    /// The offer this answers.
    #[prost(uint64, tag = "1")]
    pub transfer_id: u64,
    /// `invalid`, `busy`, `storage`, or `too_large`.
    #[prost(string, tag = "2")]
    pub reason: String,
}

/// Either side: the user cancelled the transfer. Partial files are deleted.
#[derive(Clone, PartialEq, Message)]
pub struct Cancel {
    /// The transfer.
    #[prost(uint64, tag = "1")]
    pub transfer_id: u64,
}

/// The envelope of every files control message.
#[derive(Clone, PartialEq, Message)]
pub struct FilesMsg {
    /// The message.
    #[prost(oneof = "files_msg::Msg", tags = "1, 2, 3, 4")]
    pub msg: Option<files_msg::Msg>,
}

/// The messages in [`FilesMsg`].
pub mod files_msg {
    /// One files control message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Msg {
        /// See [`super::Offer`].
        #[prost(message, tag = "1")]
        Offer(super::Offer),
        /// See [`super::Accept`].
        #[prost(message, tag = "2")]
        Accept(super::Accept),
        /// See [`super::Decline`].
        #[prost(message, tag = "3")]
        Decline(super::Decline),
        /// See [`super::Cancel`].
        #[prost(message, tag = "4")]
        Cancel(super::Cancel),
    }
}

impl From<files_msg::Msg> for FilesMsg {
    fn from(m: files_msg::Msg) -> Self {
        Self { msg: Some(m) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_round_trips() {
        let m = FilesMsg::from(files_msg::Msg::Offer(Offer {
            transfer_id: 7,
            files: vec![FileEntry {
                rel_path: "a/b.txt".into(),
                size: 3,
            }],
            total_size: 3,
        }));
        let back = FilesMsg::decode(m.encode_to_vec().as_slice()).unwrap();
        assert_eq!(back, m);
    }
}
