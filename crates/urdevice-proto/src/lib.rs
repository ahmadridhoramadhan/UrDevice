//! Protobuf messages and framing for the UrDevice protocol (design §5–§7).
//!
//! The messages are written as `prost` structs by hand, so the build needs no
//! `protoc`. The field numbers are the wire contract: never reuse or renumber
//! one. `proto/urdevice.proto` lists the same schema for readers and other
//! languages.
//!
//! Everything that comes off the network is untrusted. The decoders here only
//! enforce sizes and protobuf syntax; the core validates field contents.

use prost::Message;

pub mod features;
pub mod services;

pub use services::ServiceId;

/// Protocol version spoken by this build. Sent in `Hello`, `Announce`, and `PairInvite`.
pub const PROTO_VERSION: u32 = 1;

/// Largest control frame accepted, in bytes (R4.6).
pub const MAX_FRAME: usize = 1024 * 1024;

/// Largest [`StreamOpen`] header accepted, in bytes (design §6.2).
pub const MAX_STREAM_OPEN: usize = 16 * 1024;

/// Bytes in front of every datagram payload: service (`u16`), direction
/// (`u8`), and activity (`u64`), all big-endian (design §6.2).
pub const DATAGRAM_HEADER: usize = 11;

/// Magic bytes in front of every UDP announcement.
pub const ANNOUNCE_MAGIC: &[u8; 4] = b"URDV";

/// Largest UDP announcement accepted, in bytes (design §5).
pub const MAX_ANNOUNCE: usize = 512;

/// Errors from framing and decoding.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtoError {
    /// The frame or packet is larger than the limit.
    #[error("message too large: {0} bytes")]
    TooLarge(usize),
    /// The packet does not start with the expected magic bytes.
    #[error("bad magic")]
    BadMagic,
    /// The bytes are not a valid protobuf message.
    #[error("malformed message")]
    Malformed,
    /// The envelope carries no message.
    #[error("empty envelope")]
    Empty,
}

impl From<prost::DecodeError> for ProtoError {
    fn from(_: prost::DecodeError) -> Self {
        Self::Malformed
    }
}

// ---------------------------------------------------------------------------
// Discovery and pairing payloads
// ---------------------------------------------------------------------------

/// UDP broadcast announcement (design §5). Carries no secrets (R2.3).
#[derive(Clone, PartialEq, Message)]
pub struct Announce {
    /// Protocol version.
    #[prost(uint32, tag = "1")]
    pub v: u32,
    /// First 8 bytes of the Device ID.
    #[prost(bytes = "vec", tag = "2")]
    pub short_id: Vec<u8>,
    /// Display name.
    #[prost(string, tag = "3")]
    pub name: String,
    /// QUIC port.
    #[prost(uint32, tag = "4")]
    pub port: u32,
    /// Platform, for example `linux` or `android`.
    #[prost(string, tag = "5")]
    pub platform: String,
}

/// QR pairing invite (design §7.1).
#[derive(Clone, PartialEq, Message)]
pub struct PairInvite {
    /// Protocol version.
    #[prost(uint32, tag = "1")]
    pub v: u32,
    /// Full 32-byte Device ID of the host.
    #[prost(bytes = "vec", tag = "2")]
    pub device_id: Vec<u8>,
    /// Display name of the host.
    #[prost(string, tag = "3")]
    pub name: String,
    /// Platform of the host.
    #[prost(string, tag = "4")]
    pub platform: String,
    /// IP addresses of the host, as text.
    #[prost(string, repeated, tag = "5")]
    pub addrs: Vec<String>,
    /// QUIC port of the host.
    #[prost(uint32, tag = "6")]
    pub port: u32,
    /// 16-byte one-time token.
    #[prost(bytes = "vec", tag = "7")]
    pub token: Vec<u8>,
    /// Expiry as Unix seconds.
    #[prost(uint64, tag = "8")]
    pub expires_at: u64,
}

// ---------------------------------------------------------------------------
// Control stream messages
// ---------------------------------------------------------------------------

/// First message of a session from the connecting side (design §6.3).
#[derive(Clone, PartialEq, Message)]
pub struct Hello {
    /// Lowest protocol version supported.
    #[prost(uint32, tag = "1")]
    pub proto_min: u32,
    /// Highest protocol version supported.
    #[prost(uint32, tag = "2")]
    pub proto_max: u32,
    /// App version, for display.
    #[prost(string, tag = "3")]
    pub app_version: String,
    /// Capabilities this side implements.
    #[prost(string, repeated, tag = "4")]
    pub capabilities: Vec<String>,
    /// Current display name.
    #[prost(string, tag = "5")]
    pub name: String,
    /// Platform.
    #[prost(string, tag = "6")]
    pub platform: String,
    /// Public X25519 key for the Bluetooth LE channel (32 bytes, design
    /// §8.2). The receiver pins it; empty from older versions.
    #[prost(bytes = "vec", tag = "7")]
    pub ble_static: Vec<u8>,
}

/// Reply to [`Hello`].
#[derive(Clone, PartialEq, Message)]
pub struct Welcome {
    /// Protocol version chosen.
    #[prost(uint32, tag = "1")]
    pub proto: u32,
    /// Capabilities this side implements.
    #[prost(string, repeated, tag = "2")]
    pub capabilities: Vec<String>,
    /// Random session identifier.
    #[prost(bytes = "vec", tag = "3")]
    pub session_id: Vec<u8>,
    /// Current display name.
    #[prost(string, tag = "4")]
    pub name: String,
    /// Platform.
    #[prost(string, tag = "5")]
    pub platform: String,
    /// Public X25519 key for the Bluetooth LE channel, as in [`Hello`].
    #[prost(bytes = "vec", tag = "6")]
    pub ble_static: Vec<u8>,
}

/// QR flow: the scanner proves it knows the token (design §7.1 step 3).
#[derive(Clone, PartialEq, Message)]
pub struct PairProof {
    /// HMAC-SHA256(token, binding ‖ fp_host ‖ fp_scanner).
    #[prost(bytes = "vec", tag = "1")]
    pub hmac: Vec<u8>,
    /// Scanner display name.
    #[prost(string, tag = "2")]
    pub name: String,
    /// Scanner platform.
    #[prost(string, tag = "3")]
    pub platform: String,
}

/// QR flow: the host accepted the proof.
#[derive(Clone, PartialEq, Message)]
pub struct PairAccept {
    /// Host display name.
    #[prost(string, tag = "1")]
    pub name: String,
    /// Host platform.
    #[prost(string, tag = "2")]
    pub platform: String,
}

/// Code flow: the connecting side asks to pair (design §7.2).
#[derive(Clone, PartialEq, Message)]
pub struct PairStart {
    /// Display name of the side that started.
    #[prost(string, tag = "1")]
    pub name: String,
    /// Platform of the side that started.
    #[prost(string, tag = "2")]
    pub platform: String,
}

/// Code flow: the reply to [`PairStart`] with the host's details.
#[derive(Clone, PartialEq, Message)]
pub struct PairReady {
    /// Host display name.
    #[prost(string, tag = "1")]
    pub name: String,
    /// Host platform.
    #[prost(string, tag = "2")]
    pub platform: String,
}

/// Code flow: the user tapped "Codes match".
#[derive(Clone, PartialEq, Message)]
pub struct PairConfirm {}

/// Pairing was refused or cancelled.
#[derive(Clone, PartialEq, Message)]
pub struct PairReject {
    /// Short machine-readable reason, for example `cancelled` or `locked`.
    #[prost(string, tag = "1")]
    pub reason: String,
}

/// A permission mode on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum WireMode {
    /// Refuse every request.
    Deny = 0,
    /// Ask the user on the host.
    Ask = 1,
    /// Start at once.
    Allow = 2,
}

/// One entry of [`PermissionState`].
#[derive(Clone, PartialEq, Message)]
pub struct PermissionEntry {
    /// Capability string, for example `input.v1`.
    #[prost(string, tag = "1")]
    pub capability: String,
    /// The host's mode for it.
    #[prost(enumeration = "WireMode", tag = "2")]
    pub mode: i32,
}

/// The host's permission settings, for drawing the peer's UI only (design §6.3).
#[derive(Clone, PartialEq, Message)]
pub struct PermissionState {
    /// One entry per capability.
    #[prost(message, repeated, tag = "1")]
    pub entries: Vec<PermissionEntry>,
}

/// Battery state (R4.9). Both fields are absent on devices without a battery.
#[derive(Clone, PartialEq, Message)]
pub struct DeviceStatus {
    /// Battery level, 0 to 100.
    #[prost(uint32, optional, tag = "1")]
    pub battery_pct: Option<u32>,
    /// True while charging.
    #[prost(bool, optional, tag = "2")]
    pub charging: Option<bool>,
}

/// The user chose Disconnect (design §6.5).
#[derive(Clone, PartialEq, Message)]
pub struct Disconnect {}

/// The user removed the pairing (R3.9).
#[derive(Clone, PartialEq, Message)]
pub struct Unpair {}

/// Request to start one activity on the host (design §7.4).
#[derive(Clone, PartialEq, Message)]
pub struct ActivityRequest {
    /// Chosen by the requester, unique per session.
    #[prost(uint64, tag = "1")]
    pub request_id: u64,
    /// Capability string.
    #[prost(string, tag = "2")]
    pub capability: String,
}

/// The host's answer to an [`ActivityRequest`].
#[derive(Clone, PartialEq, Message)]
pub struct ActivityReply {
    /// The request this answers.
    #[prost(uint64, tag = "1")]
    pub request_id: u64,
    /// True when the activity may start.
    #[prost(bool, tag = "2")]
    pub granted: bool,
    /// Why it was refused: `denied`, `timeout`, `declined`, `unsupported`, or `busy`.
    #[prost(string, tag = "3")]
    pub reason: String,
}

/// Either side ends a running activity.
#[derive(Clone, PartialEq, Message)]
pub struct ActivityStop {
    /// The request that started it.
    #[prost(uint64, tag = "1")]
    pub request_id: u64,
}

/// Which side of a feature a message, stream, or datagram is for (design §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum Direction {
    /// To the device that offers the capability. The receiver's permission
    /// engine checks it before the service sees it. The default, so a missing
    /// field is always checked.
    ToHost = 0,
    /// To the device that uses the capability: replies, pushed state, and
    /// media for an activity it started. Not checked by the receiver's
    /// permission engine, so services must never act on the device for it.
    ToController = 1,
}

/// A control message for one feature service (design §6.2, §9). The body is
/// the service's own protobuf message (`features::<name>`).
#[derive(Clone, PartialEq, Message)]
pub struct Feature {
    /// [`ServiceId`] of the service.
    #[prost(uint32, tag = "1")]
    pub service: u32,
    /// Who the message is for.
    #[prost(enumeration = "Direction", tag = "2")]
    pub direction: i32,
    /// `request_id` of the granted [`ActivityRequest`] this belongs to, or 0.
    #[prost(uint64, tag = "3")]
    pub activity: u64,
    /// The service's message, encoded.
    #[prost(bytes = "vec", tag = "4")]
    pub body: Vec<u8>,
}

/// The receiver refused a [`Feature`] message.
#[derive(Clone, PartialEq, Message)]
pub struct FeatureRefused {
    /// [`ServiceId`] of the service.
    #[prost(uint32, tag = "1")]
    pub service: u32,
    /// The activity from the refused message.
    #[prost(uint64, tag = "2")]
    pub activity: u64,
    /// `denied`, `ask` (Always Ask: send an `ActivityRequest` first),
    /// `no_activity` (unknown or ended activity), or `unsupported`.
    #[prost(string, tag = "3")]
    pub reason: String,
}

/// First frame on every QUIC stream other than the control stream:
/// `[u32 big-endian length][StreamOpen]`, at most [`MAX_STREAM_OPEN`] bytes.
/// The service's own bytes follow.
#[derive(Clone, PartialEq, Message)]
pub struct StreamOpen {
    /// [`ServiceId`] of the service.
    #[prost(uint32, tag = "1")]
    pub service: u32,
    /// Who the stream is for.
    #[prost(enumeration = "Direction", tag = "2")]
    pub direction: i32,
    /// `request_id` of the granted activity, or 0.
    #[prost(uint64, tag = "3")]
    pub activity: u64,
    /// Stream kind, defined by the service.
    #[prost(uint32, tag = "4")]
    pub kind: u32,
    /// Extra data for the service, for example an encoded open message.
    #[prost(bytes = "vec", tag = "5")]
    pub meta: Vec<u8>,
}

/// Envelope for every control-stream message.
///
/// Tags 1 to 63 belong to the core. Features never add a tag here; they send
/// [`Feature`] messages instead (design §6.2).
#[derive(Clone, PartialEq, Message)]
pub struct Control {
    /// The message.
    #[prost(
        oneof = "control::Msg",
        tags = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17"
    )]
    pub msg: Option<control::Msg>,
}

/// Nested types for [`Control`].
pub mod control {
    /// One control message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Msg {
        /// See [`super::Hello`].
        #[prost(message, tag = "1")]
        Hello(super::Hello),
        /// See [`super::Welcome`].
        #[prost(message, tag = "2")]
        Welcome(super::Welcome),
        /// See [`super::PairProof`].
        #[prost(message, tag = "3")]
        PairProof(super::PairProof),
        /// See [`super::PairAccept`].
        #[prost(message, tag = "4")]
        PairAccept(super::PairAccept),
        /// See [`super::PairStart`].
        #[prost(message, tag = "5")]
        PairStart(super::PairStart),
        /// See [`super::PairReady`].
        #[prost(message, tag = "6")]
        PairReady(super::PairReady),
        /// See [`super::PairConfirm`].
        #[prost(message, tag = "7")]
        PairConfirm(super::PairConfirm),
        /// See [`super::PairReject`].
        #[prost(message, tag = "8")]
        PairReject(super::PairReject),
        /// See [`super::PermissionState`].
        #[prost(message, tag = "9")]
        PermissionState(super::PermissionState),
        /// See [`super::DeviceStatus`].
        #[prost(message, tag = "10")]
        DeviceStatus(super::DeviceStatus),
        /// See [`super::Disconnect`].
        #[prost(message, tag = "11")]
        Disconnect(super::Disconnect),
        /// See [`super::Unpair`].
        #[prost(message, tag = "12")]
        Unpair(super::Unpair),
        /// See [`super::ActivityRequest`].
        #[prost(message, tag = "13")]
        ActivityRequest(super::ActivityRequest),
        /// See [`super::ActivityReply`].
        #[prost(message, tag = "14")]
        ActivityReply(super::ActivityReply),
        /// See [`super::ActivityStop`].
        #[prost(message, tag = "15")]
        ActivityStop(super::ActivityStop),
        /// See [`super::Feature`].
        #[prost(message, tag = "16")]
        Feature(super::Feature),
        /// See [`super::FeatureRefused`].
        #[prost(message, tag = "17")]
        FeatureRefused(super::FeatureRefused),
    }
}

impl From<control::Msg> for Control {
    fn from(msg: control::Msg) -> Self {
        Self { msg: Some(msg) }
    }
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// Encode a control message as `[u32 big-endian length][protobuf]`.
pub fn encode_frame(msg: &Control) -> Result<Vec<u8>, ProtoError> {
    let len = msg.encoded_len();
    if len > MAX_FRAME {
        return Err(ProtoError::TooLarge(len));
    }
    let mut out = Vec::with_capacity(4 + len);
    // `len` is at most MAX_FRAME, which fits in a u32.
    out.extend_from_slice(&(len as u32).to_be_bytes());
    msg.encode(&mut out).map_err(|_| ProtoError::Malformed)?;
    Ok(out)
}

/// Read the body length from a 4-byte frame header and check the limit.
pub fn frame_len(header: [u8; 4]) -> Result<usize, ProtoError> {
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME {
        return Err(ProtoError::TooLarge(len));
    }
    Ok(len)
}

/// Decode a frame body into its message.
pub fn decode_control(body: &[u8]) -> Result<control::Msg, ProtoError> {
    if body.len() > MAX_FRAME {
        return Err(ProtoError::TooLarge(body.len()));
    }
    Control::decode(body)?.msg.ok_or(ProtoError::Empty)
}

/// Encode an announcement packet: magic followed by the protobuf.
pub fn encode_announce(a: &Announce) -> Result<Vec<u8>, ProtoError> {
    let mut out = Vec::with_capacity(4 + a.encoded_len());
    out.extend_from_slice(ANNOUNCE_MAGIC);
    a.encode(&mut out).map_err(|_| ProtoError::Malformed)?;
    if out.len() > MAX_ANNOUNCE {
        return Err(ProtoError::TooLarge(out.len()));
    }
    Ok(out)
}

/// Decode an announcement packet, enforcing the size limit and the magic.
pub fn decode_announce(packet: &[u8]) -> Result<Announce, ProtoError> {
    if packet.len() > MAX_ANNOUNCE {
        return Err(ProtoError::TooLarge(packet.len()));
    }
    let body = packet
        .strip_prefix(ANNOUNCE_MAGIC)
        .ok_or(ProtoError::BadMagic)?;
    Ok(Announce::decode(body)?)
}

/// Encode a pairing invite as protobuf bytes (the QR code wraps them in a URI).
pub fn encode_invite(invite: &PairInvite) -> Vec<u8> {
    invite.encode_to_vec()
}

/// Encode a stream header as `[u32 big-endian length][StreamOpen]`.
pub fn encode_stream_open(open: &StreamOpen) -> Result<Vec<u8>, ProtoError> {
    let len = open.encoded_len();
    if len > MAX_STREAM_OPEN {
        return Err(ProtoError::TooLarge(len));
    }
    let mut out = Vec::with_capacity(4 + len);
    // `len` is at most MAX_STREAM_OPEN, which fits in a u32.
    out.extend_from_slice(&(len as u32).to_be_bytes());
    open.encode(&mut out).map_err(|_| ProtoError::Malformed)?;
    Ok(out)
}

/// Read the body length from a 4-byte stream header and check the limit.
pub fn stream_open_len(header: [u8; 4]) -> Result<usize, ProtoError> {
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_STREAM_OPEN {
        return Err(ProtoError::TooLarge(len));
    }
    Ok(len)
}

/// Decode a stream header body.
pub fn decode_stream_open(body: &[u8]) -> Result<StreamOpen, ProtoError> {
    if body.len() > MAX_STREAM_OPEN {
        return Err(ProtoError::TooLarge(body.len()));
    }
    Ok(StreamOpen::decode(body)?)
}

/// The header in front of a datagram payload (design §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatagramHeader {
    /// The service the datagram is for.
    pub service: ServiceId,
    /// Who it is for.
    pub direction: Direction,
    /// The activity it belongs to, or 0.
    pub activity: u64,
}

/// Build a datagram: [`DATAGRAM_HEADER`] bytes followed by `payload`.
pub fn encode_datagram(h: DatagramHeader, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(DATAGRAM_HEADER + payload.len());
    out.extend_from_slice(&h.service.0.to_be_bytes());
    out.push(match h.direction {
        Direction::ToHost => 0,
        Direction::ToController => 1,
    });
    out.extend_from_slice(&h.activity.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Split a datagram into its header and payload.
pub fn decode_datagram(d: &[u8]) -> Result<(DatagramHeader, &[u8]), ProtoError> {
    if d.len() < DATAGRAM_HEADER {
        return Err(ProtoError::Malformed);
    }
    let service = ServiceId(u16::from_be_bytes([d[0], d[1]]));
    let direction = match d[2] {
        0 => Direction::ToHost,
        1 => Direction::ToController,
        _ => return Err(ProtoError::Malformed),
    };
    let mut a = [0u8; 8];
    a.copy_from_slice(&d[3..11]);
    Ok((
        DatagramHeader {
            service,
            direction,
            activity: u64::from_be_bytes(a),
        },
        &d[DATAGRAM_HEADER..],
    ))
}

/// Decode pairing invite bytes. `max` bounds the input size.
pub fn decode_invite(bytes: &[u8], max: usize) -> Result<PairInvite, ProtoError> {
    if bytes.len() > max {
        return Err(ProtoError::TooLarge(bytes.len()));
    }
    Ok(PairInvite::decode(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip() {
        let msg = Control::from(control::Msg::ActivityRequest(ActivityRequest {
            request_id: 7,
            capability: "input.v1".into(),
        }));
        let frame = encode_frame(&msg).unwrap();
        let len = frame_len(frame[..4].try_into().unwrap()).unwrap();
        assert_eq!(len, frame.len() - 4);
        assert_eq!(decode_control(&frame[4..]).unwrap(), msg.msg.unwrap());
    }

    #[test]
    fn rejects_oversized_frame_header() {
        let header = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert_eq!(frame_len(header), Err(ProtoError::TooLarge(MAX_FRAME + 1)));
        assert!(frame_len((MAX_FRAME as u32).to_be_bytes()).is_ok());
    }

    #[test]
    fn rejects_empty_envelope_and_garbage() {
        assert_eq!(decode_control(&[]), Err(ProtoError::Empty));
        assert_eq!(
            decode_control(&[0xff, 0xff, 0xff]),
            Err(ProtoError::Malformed)
        );
    }

    #[test]
    fn announce_round_trip_and_limits() {
        let a = Announce {
            v: PROTO_VERSION,
            short_id: vec![1; 8],
            name: "Laptop".into(),
            port: 47811,
            platform: "linux".into(),
        };
        let packet = encode_announce(&a).unwrap();
        assert_eq!(decode_announce(&packet).unwrap(), a);
        assert_eq!(decode_announce(b"XXXX"), Err(ProtoError::BadMagic));
        assert_eq!(
            decode_announce(&vec![0; MAX_ANNOUNCE + 1]),
            Err(ProtoError::TooLarge(MAX_ANNOUNCE + 1))
        );
        let big = Announce {
            name: "x".repeat(600),
            ..a
        };
        assert!(matches!(
            encode_announce(&big),
            Err(ProtoError::TooLarge(_))
        ));
    }

    #[test]
    fn invite_round_trip() {
        let invite = PairInvite {
            v: 1,
            device_id: vec![9; 32],
            name: "Pixel".into(),
            platform: "android".into(),
            addrs: vec!["192.168.1.5".into()],
            port: 47811,
            token: vec![3; 16],
            expires_at: 1_800_000_000,
        };
        let bytes = encode_invite(&invite);
        assert_eq!(decode_invite(&bytes, 1024).unwrap(), invite);
        assert!(decode_invite(&bytes, 4).is_err());
    }

    #[test]
    fn feature_frame_round_trip() {
        let msg = Control::from(control::Msg::Feature(Feature {
            service: u32::from(ServiceId::CLIPBOARD.0),
            direction: Direction::ToController as i32,
            activity: 9,
            body: vec![1, 2, 3],
        }));
        let frame = encode_frame(&msg).unwrap();
        assert_eq!(decode_control(&frame[4..]).unwrap(), msg.msg.unwrap());
        // A missing direction reads as ToHost, which is always checked.
        let f = Feature {
            service: 1,
            ..Default::default()
        };
        assert_eq!(f.direction(), Direction::ToHost);
    }

    #[test]
    fn stream_open_round_trip_and_limits() {
        let open = StreamOpen {
            service: 7,
            direction: 0,
            activity: 42,
            kind: 1,
            meta: vec![5; 10],
        };
        let bytes = encode_stream_open(&open).unwrap();
        let len = stream_open_len(bytes[..4].try_into().unwrap()).unwrap();
        assert_eq!(len, bytes.len() - 4);
        assert_eq!(decode_stream_open(&bytes[4..]).unwrap(), open);
        let header = ((MAX_STREAM_OPEN + 1) as u32).to_be_bytes();
        assert!(stream_open_len(header).is_err());
        let big = StreamOpen {
            meta: vec![0; MAX_STREAM_OPEN],
            ..open
        };
        assert!(encode_stream_open(&big).is_err());
    }

    #[test]
    fn datagram_round_trip_and_garbage() {
        let h = DatagramHeader {
            service: ServiceId::INPUT,
            direction: Direction::ToHost,
            activity: 0x0102_0304_0506_0708,
        };
        let d = encode_datagram(h, b"xy");
        assert_eq!(d.len(), DATAGRAM_HEADER + 2);
        let (h2, payload) = decode_datagram(&d).unwrap();
        assert_eq!(h2, h);
        assert_eq!(payload, b"xy");
        assert!(decode_datagram(&d[..DATAGRAM_HEADER - 1]).is_err());
        let mut bad = d.clone();
        bad[2] = 7;
        assert!(decode_datagram(&bad).is_err());
    }
}
