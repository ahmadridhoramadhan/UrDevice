//! Clipboard sharing (#16, design §9.1): wire messages for `ServiceId::CLIPBOARD`.
//!
//! Owned by the clipboard feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/clipboard.proto`.
