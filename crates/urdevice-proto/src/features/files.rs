//! File transfer (#17, design §9.2): wire messages for `ServiceId::FILES`.
//!
//! Owned by the files feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/files.proto`.
