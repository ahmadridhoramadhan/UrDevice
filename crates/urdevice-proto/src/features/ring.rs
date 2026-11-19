//! Find my device (#27, design §18.9): wire messages for `ServiceId::RING`.
//!
//! Owned by the ring feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/ring.proto`.
