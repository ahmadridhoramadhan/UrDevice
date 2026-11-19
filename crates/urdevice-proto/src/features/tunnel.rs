//! Localhost sharing and Expose on LAN (#22, design §11): wire messages for `ServiceId::TUNNEL`.
//!
//! Owned by the tunnel feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/tunnel.proto`.
