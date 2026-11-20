//! Framing on a transfer's data stream (design §9.2).
//!
//! Data direction (sender → receiver), each frame starts with a tag byte:
//!
//! | Tag | Frame | Body |
//! |---|---|---|
//! | 1 | Data | `[index u32][offset u64][len u32][len bytes]`, `len ≤ 1 MiB` |
//! | 2 | End | `[index u32][sha256 32 bytes]`: the file's last byte was sent |
//!
//! Ack direction (receiver → sender):
//!
//! | Tag | Frame | Body |
//! |---|---|---|
//! | 1 | Ack | `[index u32][offset u64]`: bytes written so far |
//! | 2 | Result | `[index u32][status u8]`: 1 verified, 0 hash failed (send it again), 2 failed |
//!
//! All integers are big-endian. Everything read here is untrusted: sizes are
//! checked before anything is allocated, and an unknown tag is an error.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Chunk size the sender uses (T3.10).
pub const CHUNK: usize = 1024 * 1024;
/// Largest data frame accepted.
pub const MAX_FRAME: usize = 1024 * 1024;

const T_DATA: u8 = 1;
const T_END: u8 = 2;
const T_ACK: u8 = 1;
const T_RESULT: u8 = 2;

/// Result status: the file matched its SHA-256 and was saved.
pub const OK: u8 = 1;
/// Result status: the hash did not match; the sender may send it once more.
pub const RETRY: u8 = 0;
/// Result status: the file failed for good.
pub const FAILED: u8 = 2;

/// A frame from the sender. `Data` borrows its bytes from the reader's
/// buffer.
#[derive(Debug, PartialEq, Eq)]
pub enum DataFrame<'a> {
    /// File bytes at `offset`.
    Data {
        /// File index.
        index: u32,
        /// Byte offset in the file.
        offset: u64,
        /// The bytes.
        bytes: &'a [u8],
    },
    /// The whole file was sent; its SHA-256.
    End {
        /// File index.
        index: u32,
        /// SHA-256 of the whole file.
        sha256: [u8; 32],
    },
}

/// A frame from the receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckFrame {
    /// Bytes of file `index` written so far.
    Ack {
        /// File index.
        index: u32,
        /// Bytes written.
        offset: u64,
    },
    /// The outcome of file `index`: [`OK`], [`RETRY`], or [`FAILED`].
    Result {
        /// File index.
        index: u32,
        /// Status.
        status: u8,
    },
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

/// Read one optional tag byte: `None` on a clean end of stream.
async fn read_tag<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<u8>> {
    let mut tag = [0u8; 1];
    match r.read(&mut tag).await? {
        0 => Ok(None),
        _ => Ok(Some(tag[0])),
    }
}

/// Reads [`DataFrame`]s, reusing one buffer.
#[derive(Debug, Default)]
pub struct DataReader {
    buf: Vec<u8>,
}

impl DataReader {
    /// The next frame, or `None` when the sender finished the stream.
    pub async fn next<R: AsyncRead + Unpin>(
        &mut self,
        r: &mut R,
    ) -> io::Result<Option<DataFrame<'_>>> {
        let Some(tag) = read_tag(r).await? else {
            return Ok(None);
        };
        match tag {
            T_DATA => {
                let mut h = [0u8; 16];
                r.read_exact(&mut h).await?;
                let index = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
                let offset = u64::from_be_bytes([h[4], h[5], h[6], h[7], h[8], h[9], h[10], h[11]]);
                let len = u32::from_be_bytes([h[12], h[13], h[14], h[15]]) as usize;
                if len > MAX_FRAME {
                    return Err(bad("data frame too large"));
                }
                self.buf.resize(len, 0);
                r.read_exact(&mut self.buf).await?;
                Ok(Some(DataFrame::Data {
                    index,
                    offset,
                    bytes: &self.buf,
                }))
            }
            T_END => {
                let mut h = [0u8; 36];
                r.read_exact(&mut h).await?;
                let index = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
                let mut sha256 = [0u8; 32];
                sha256.copy_from_slice(&h[4..]);
                Ok(Some(DataFrame::End { index, sha256 }))
            }
            _ => Err(bad("unknown frame")),
        }
    }
}

/// The 17 bytes in front of a data chunk.
pub fn data_header(index: u32, offset: u64, len: usize) -> [u8; 17] {
    let mut h = [0u8; 17];
    h[0] = T_DATA;
    h[1..5].copy_from_slice(&index.to_be_bytes());
    h[5..13].copy_from_slice(&offset.to_be_bytes());
    // `len` is at most CHUNK.
    h[13..17].copy_from_slice(&(len.min(MAX_FRAME) as u32).to_be_bytes());
    h
}

/// Write a data frame.
pub async fn write_data<W: AsyncWrite + Unpin>(
    w: &mut W,
    index: u32,
    offset: u64,
    bytes: &[u8],
) -> io::Result<()> {
    if bytes.len() > MAX_FRAME {
        return Err(bad("chunk too large"));
    }
    w.write_all(&data_header(index, offset, bytes.len()))
        .await?;
    w.write_all(bytes).await
}

/// Write an end frame.
pub async fn write_end<W: AsyncWrite + Unpin>(
    w: &mut W,
    index: u32,
    sha256: &[u8; 32],
) -> io::Result<()> {
    let mut f = [0u8; 37];
    f[0] = T_END;
    f[1..5].copy_from_slice(&index.to_be_bytes());
    f[5..].copy_from_slice(sha256);
    w.write_all(&f).await
}

/// Read the next ack frame, or `None` when the receiver finished.
pub async fn read_ack<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<AckFrame>> {
    let Some(tag) = read_tag(r).await? else {
        return Ok(None);
    };
    match tag {
        T_ACK => {
            let index = r.read_u32().await?;
            let offset = r.read_u64().await?;
            Ok(Some(AckFrame::Ack { index, offset }))
        }
        T_RESULT => {
            let index = r.read_u32().await?;
            let status = r.read_u8().await?;
            if status > FAILED {
                return Err(bad("bad status"));
            }
            Ok(Some(AckFrame::Result { index, status }))
        }
        _ => Err(bad("unknown ack")),
    }
}

/// Encode an ack frame.
pub fn encode_ack(f: AckFrame) -> Vec<u8> {
    match f {
        AckFrame::Ack { index, offset } => {
            let mut v = vec![T_ACK];
            v.extend_from_slice(&index.to_be_bytes());
            v.extend_from_slice(&offset.to_be_bytes());
            v
        }
        AckFrame::Result { index, status } => {
            let mut v = vec![T_RESULT];
            v.extend_from_slice(&index.to_be_bytes());
            v.push(status);
            v
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::pin::Pin;
    use std::task::{Context, Poll};

    use proptest::prelude::*;
    use tokio::io::ReadBuf;

    use super::*;

    /// A reader that hands out its bytes in the given piece sizes, so frames
    /// arrive split at arbitrary points.
    struct Pieces {
        data: Vec<u8>,
        pos: usize,
        sizes: Vec<usize>,
        n: usize,
    }

    impl AsyncRead for Pieces {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let s = &mut *self;
            let want = s
                .sizes
                .get(s.n % s.sizes.len().max(1))
                .copied()
                .unwrap_or(1)
                .max(1);
            s.n += 1;
            let left = s.data.len() - s.pos;
            let k = want.min(left).min(buf.remaining());
            buf.put_slice(&s.data[s.pos..s.pos + k]);
            s.pos += k;
            Poll::Ready(Ok(()))
        }
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Owned {
        Data(u32, u64, Vec<u8>),
        End(u32, [u8; 32]),
    }

    fn frame() -> impl Strategy<Value = Owned> {
        prop_oneof![
            (
                any::<u32>(),
                any::<u64>(),
                proptest::collection::vec(any::<u8>(), 0..2000)
            )
                .prop_map(|(i, o, b)| Owned::Data(i, o, b)),
            (any::<u32>(), any::<[u8; 32]>()).prop_map(|(i, h)| Owned::End(i, h)),
        ]
    }

    async fn decode_all(r: &mut Pieces) -> io::Result<Vec<Owned>> {
        let mut reader = DataReader::default();
        let mut out = Vec::new();
        while let Some(f) = reader.next(r).await? {
            out.push(match f {
                DataFrame::Data {
                    index,
                    offset,
                    bytes,
                } => Owned::Data(index, offset, bytes.to_vec()),
                DataFrame::End { index, sha256 } => Owned::End(index, sha256),
            });
        }
        Ok(out)
    }

    proptest! {
        /// Framing survives any split of the byte stream (design §16).
        #[test]
        fn frames_round_trip_with_any_split(
            frames in proptest::collection::vec(frame(), 0..20),
            sizes in proptest::collection::vec(1usize..64, 1..8),
        ) {
            let mut data = Vec::new();
            rt().block_on(async {
                for f in &frames {
                    match f {
                        Owned::Data(i, o, b) => write_data(&mut data, *i, *o, b).await.unwrap(),
                        Owned::End(i, h) => write_end(&mut data, *i, h).await.unwrap(),
                    }
                }
            });
            let mut r = Pieces { data, pos: 0, sizes, n: 0 };
            let back = rt().block_on(decode_all(&mut r)).unwrap();
            prop_assert_eq!(back, frames);
        }

        /// Arbitrary bytes never panic and never allocate past the limit.
        #[test]
        fn garbage_is_an_error_or_frames(
            data in proptest::collection::vec(any::<u8>(), 0..4096),
            sizes in proptest::collection::vec(1usize..64, 1..4),
        ) {
            let mut r = Pieces { data, pos: 0, sizes, n: 0 };
            let _ = rt().block_on(decode_all(&mut r));
        }

        #[test]
        fn acks_round_trip(index in any::<u32>(), offset in any::<u64>(), status in 0u8..3) {
            let mut bytes = encode_ack(AckFrame::Ack { index, offset });
            bytes.extend(encode_ack(AckFrame::Result { index, status }));
            let mut r = Pieces { data: bytes, pos: 0, sizes: vec![3], n: 0 };
            let (a, b, c) = rt().block_on(async {
                (read_ack(&mut r).await.unwrap(), read_ack(&mut r).await.unwrap(), read_ack(&mut r).await.unwrap())
            });
            prop_assert_eq!(a, Some(AckFrame::Ack { index, offset }));
            prop_assert_eq!(b, Some(AckFrame::Result { index, status }));
            prop_assert_eq!(c, None);
        }
    }

    #[test]
    fn oversized_frames_are_refused_before_allocating() {
        let mut data = vec![T_DATA];
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u64.to_be_bytes());
        data.extend_from_slice(&u32::MAX.to_be_bytes());
        let mut r = Pieces {
            data,
            pos: 0,
            sizes: vec![100],
            n: 0,
        };
        let e = rt().block_on(decode_all(&mut r)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
        let mut r = Pieces {
            data: vec![9],
            pos: 0,
            sizes: vec![1],
            n: 0,
        };
        assert!(rt().block_on(decode_all(&mut r)).is_err());
        let mut r = Pieces {
            data: vec![T_RESULT, 0, 0, 0, 0, 7],
            pos: 0,
            sizes: vec![1],
            n: 0,
        };
        assert!(rt().block_on(read_ack(&mut r)).is_err());
    }

    #[test]
    fn truncated_frame_is_an_error() {
        let mut data = Vec::new();
        rt().block_on(write_data(&mut data, 1, 2, b"hello"))
            .unwrap();
        data.pop();
        let mut r = Pieces {
            data,
            pos: 0,
            sizes: vec![4],
            n: 0,
        };
        assert!(rt().block_on(decode_all(&mut r)).is_err());
    }
}
