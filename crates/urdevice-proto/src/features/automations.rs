//! Automations (#39, design §18.14): wire messages for `ServiceId::AUTOMATIONS`.
//!
//! Owned by the automations feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/automations.proto`.
