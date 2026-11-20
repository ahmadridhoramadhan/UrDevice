//! Bluetooth fallback (#23, design §8): wire messages for `ServiceId::BLUETOOTH`.
//!
//! The service has no protobuf messages. Its id marks the empty path probes
//! the node sends as QUIC datagrams (design §8.1); the receiver drops them.
//! The Bluetooth LE channel's packet framing and message types are binary
//! and live with the code in `urdevice_core::services::bluetooth`.
