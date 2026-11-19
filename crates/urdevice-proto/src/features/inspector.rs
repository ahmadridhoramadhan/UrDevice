//! HTTP request inspector (#34, design §18.8): wire messages for `ServiceId::INSPECTOR`.
//!
//! Owned by the inspector feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/inspector.proto`.
