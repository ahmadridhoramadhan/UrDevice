//! Use another device as a speaker (#40, design §18.15): wire messages for `ServiceId::SPEAKER`.
//!
//! Owned by the speaker feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/speaker.proto`.
