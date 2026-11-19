//! Remote DevTools and ADB (#35, design §18.6): wire messages for `ServiceId::ADB`.
//!
//! Owned by the adb feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/adb.proto`.
