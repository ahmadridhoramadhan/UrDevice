//! Screen sharing (#21, design §10): wire messages for `ServiceId::SCREEN`.
//!
//! Owned by the screen feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/screen.proto`.
