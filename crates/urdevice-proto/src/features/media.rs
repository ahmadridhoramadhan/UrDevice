//! Media control (#20, design §9.4): wire messages for `ServiceId::MEDIA`.
//!
//! Owned by the media feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/media.proto`.
