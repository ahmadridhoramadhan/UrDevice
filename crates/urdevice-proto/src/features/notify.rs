//! Notification mirroring (#26, design §18.2): wire messages for `ServiceId::NOTIFY`.
//!
//! Owned by the notify feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/notify.proto`.
