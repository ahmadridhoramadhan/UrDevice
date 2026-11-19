//! Phone or tablet as a second display (#31, design §18.4): wire messages for `ServiceId::DISPLAY`.
//!
//! Owned by the display feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/display.proto`.
