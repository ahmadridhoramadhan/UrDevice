//! BLE framing (design §8.2): Noise messages split into GATT packets.
//!
//! A GATT write or notification carries at most the negotiated ATT MTU minus
//! 3 bytes. Each packet starts with a 2-byte big-endian sequence number and a
//! flags byte; the last packet of a message sets [`END`]. The receiver
//! accepts packets only in sequence and never buffers more than
//! [`MAX_MESSAGE`] bytes, so a hostile peer can neither reorder data nor make
//! it grow without bound. Any error closes the link.

/// Bytes in front of every packet: sequence number (2) and flags (1).
pub const HEADER: usize = 3;
/// Largest message a sender may split: the Noise message limit.
pub const MAX_MESSAGE: usize = 65_535;
/// Smallest packet: the default ATT MTU of 23 minus 3.
pub const MIN_PACKET: usize = 20;
/// Largest packet: the largest ATT attribute value.
pub const MAX_PACKET: usize = 512;
/// Flag: the last packet of a message.
pub const END: u8 = 0x01;

/// Why a packet or message was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// Shorter than the header.
    #[error("packet too short")]
    Short,
    /// Longer than [`MAX_PACKET`].
    #[error("packet too long")]
    Long,
    /// Not the next sequence number: lost, repeated, or reordered.
    #[error("packet out of sequence")]
    Sequence,
    /// Unknown flag bits.
    #[error("unknown packet flags")]
    Flags,
    /// The message would be larger than [`MAX_MESSAGE`].
    #[error("message too large")]
    TooLarge,
}

/// Packet size to use for an ATT payload size reported by the radio.
pub fn packet_size(att_payload: usize) -> usize {
    att_payload.clamp(MIN_PACKET, MAX_PACKET)
}

/// The sending half: numbers packets in order.
#[derive(Debug, Default)]
pub struct Splitter {
    next: u16,
}

impl Splitter {
    /// Split one message into packets of at most `packet` bytes.
    pub fn split(&mut self, msg: &[u8], packet: usize) -> Result<Vec<Vec<u8>>, FrameError> {
        if msg.len() > MAX_MESSAGE {
            return Err(FrameError::TooLarge);
        }
        let room = packet_size(packet) - HEADER;
        let mut out = Vec::with_capacity(msg.len() / room + 1);
        let mut chunks = msg.chunks(room).peekable();
        if chunks.peek().is_none() {
            out.push(self.packet(&[], true));
        }
        while let Some(c) = chunks.next() {
            let last = chunks.peek().is_none();
            out.push(self.packet(c, last));
        }
        Ok(out)
    }

    fn packet(&mut self, data: &[u8], last: bool) -> Vec<u8> {
        let mut p = Vec::with_capacity(HEADER + data.len());
        p.extend_from_slice(&self.next.to_be_bytes());
        p.push(if last { END } else { 0 });
        p.extend_from_slice(data);
        self.next = self.next.wrapping_add(1);
        p
    }
}

/// The receiving half: checks the order and joins packets into messages.
#[derive(Debug, Default)]
pub struct Reassembler {
    next: u16,
    buf: Vec<u8>,
}

impl Reassembler {
    /// Take one packet. Returns a whole message after its last packet.
    pub fn push(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>, FrameError> {
        if packet.len() < HEADER {
            return Err(FrameError::Short);
        }
        if packet.len() > MAX_PACKET {
            return Err(FrameError::Long);
        }
        let seq = u16::from_be_bytes([packet[0], packet[1]]);
        if seq != self.next {
            return Err(FrameError::Sequence);
        }
        let flags = packet[2];
        if flags & !END != 0 {
            return Err(FrameError::Flags);
        }
        let data = &packet[HEADER..];
        if self.buf.len() + data.len() > MAX_MESSAGE {
            return Err(FrameError::TooLarge);
        }
        self.next = self.next.wrapping_add(1);
        self.buf.extend_from_slice(data);
        if flags & END != 0 {
            Ok(Some(std::mem::take(&mut self.buf)))
        } else {
            Ok(None)
        }
    }

    /// Bytes held for an unfinished message.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn roundtrip(msgs: &[Vec<u8>], packet: usize) -> Vec<Vec<u8>> {
        let mut s = Splitter::default();
        let mut r = Reassembler::default();
        let mut out = Vec::new();
        for m in msgs {
            for p in s.split(m, packet).unwrap() {
                assert!(p.len() <= packet_size(packet));
                if let Some(whole) = r.push(&p).unwrap() {
                    out.push(whole);
                }
            }
        }
        out
    }

    #[test]
    fn small_and_empty_messages() {
        let msgs = vec![vec![], vec![1], vec![7; 17], vec![9; 18], vec![3; 1000]];
        assert_eq!(roundtrip(&msgs, 20), msgs);
        assert_eq!(roundtrip(&msgs, 512), msgs);
    }

    #[test]
    fn largest_message_fits_and_one_more_byte_does_not() {
        let msg = vec![5; MAX_MESSAGE];
        assert_eq!(roundtrip(std::slice::from_ref(&msg), 244), vec![msg]);
        let mut s = Splitter::default();
        assert_eq!(
            s.split(&vec![0; MAX_MESSAGE + 1], 244),
            Err(FrameError::TooLarge)
        );
    }

    #[test]
    fn hostile_packets_are_refused() {
        let mut r = Reassembler::default();
        assert_eq!(r.push(&[0, 0]), Err(FrameError::Short));
        assert_eq!(r.push(&[0; MAX_PACKET + 1]), Err(FrameError::Long));
        assert_eq!(r.push(&[0, 1, END]), Err(FrameError::Sequence));
        assert_eq!(r.push(&[0, 0, 0x80]), Err(FrameError::Flags));

        // A message that never ends stops at the limit.
        let mut r = Reassembler::default();
        let mut seq = 0u16;
        let data = vec![0u8; MAX_PACKET - HEADER];
        let err = loop {
            let mut p = seq.to_be_bytes().to_vec();
            p.push(0);
            p.extend_from_slice(&data);
            match r.push(&p) {
                Ok(None) => seq = seq.wrapping_add(1),
                Ok(Some(_)) => panic!("no END was sent"),
                Err(e) => break e,
            }
        };
        assert_eq!(err, FrameError::TooLarge);
        assert!(r.pending() <= MAX_MESSAGE);
    }

    #[test]
    fn a_repeated_or_skipped_packet_is_refused() {
        let mut s = Splitter::default();
        let ps = s.split(&[1; 100], 20).unwrap();
        let mut r = Reassembler::default();
        r.push(&ps[0]).unwrap();
        assert_eq!(r.push(&ps[0]), Err(FrameError::Sequence));
        let mut r = Reassembler::default();
        r.push(&ps[0]).unwrap();
        assert_eq!(r.push(&ps[2]), Err(FrameError::Sequence));
    }

    #[test]
    fn sequence_numbers_wrap() {
        let msgs: Vec<Vec<u8>> = (0..70_000u32).map(|i| vec![(i % 251) as u8]).collect();
        assert_eq!(roundtrip(&msgs, 20), msgs);
    }

    proptest! {
        // T8.4: the reassembler is fed arbitrary bytes and must never panic,
        // never hold more than MAX_MESSAGE, and never return more than that.
        #[test]
        fn arbitrary_packets_never_panic(packets in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..600), 0..64)) {
            let mut r = Reassembler::default();
            for p in packets {
                match r.push(&p) {
                    Ok(Some(m)) => prop_assert!(m.len() <= MAX_MESSAGE),
                    Ok(None) => prop_assert!(r.pending() <= MAX_MESSAGE),
                    Err(_) => break,
                }
            }
        }

        // Packets with valid headers in order but random flags and data.
        #[test]
        fn ordered_random_packets_stay_bounded(
            parts in prop::collection::vec((any::<bool>(), prop::collection::vec(any::<u8>(), 0..509)), 0..300)
        ) {
            let mut r = Reassembler::default();
            for (i, (end, data)) in parts.into_iter().enumerate() {
                let mut p = u16::try_from(i).unwrap().to_be_bytes().to_vec();
                p.push(if end { END } else { 0 });
                p.extend_from_slice(&data);
                match r.push(&p) {
                    Ok(Some(m)) => prop_assert!(m.len() <= MAX_MESSAGE),
                    Ok(None) => prop_assert!(r.pending() <= MAX_MESSAGE),
                    Err(e) => { prop_assert_eq!(e, FrameError::TooLarge); break; }
                }
            }
        }

        #[test]
        fn split_then_join_is_identity(
            msgs in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..3000), 1..8),
            packet in 0usize..700,
        ) {
            prop_assert_eq!(roundtrip(&msgs, packet), msgs);
        }
    }
}
