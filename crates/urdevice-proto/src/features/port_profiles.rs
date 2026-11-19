//! Port forwarding profiles (#33, design §18.7): wire messages for `ServiceId::PORT_PROFILES`.
//!
//! Owned by the port_profiles feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/port_profiles.proto`.
