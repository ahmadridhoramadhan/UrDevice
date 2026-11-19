//! Lock the desktop with the phone (#38, design §18.12): wire messages for `ServiceId::PROXIMITY_LOCK`.
//!
//! Owned by the proximity_lock feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/proximity_lock.proto`.
