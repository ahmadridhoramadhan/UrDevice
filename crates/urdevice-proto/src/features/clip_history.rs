//! Shared clipboard history (#37, design §18.11): wire messages for `ServiceId::CLIP_HISTORY`.
//!
//! Owned by the clip_history feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/clip_history.proto`.
