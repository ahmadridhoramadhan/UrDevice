//! Remote terminal (#32, design §18.5): wire messages for `ServiceId::TERMINAL`.
//!
//! Owned by the terminal feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/terminal.proto`.
