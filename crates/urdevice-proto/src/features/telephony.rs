//! SMS and calls from the desktop (#29, design §18.13): wire messages for `ServiceId::TELEPHONY`.
//!
//! Owned by the telephony feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/telephony.proto`.
