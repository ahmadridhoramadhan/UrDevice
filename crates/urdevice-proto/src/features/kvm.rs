//! Shared mouse and keyboard across desktops (#30, design §18.3): wire messages for `ServiceId::KVM`.
//!
//! Owned by the kvm feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/kvm.proto`.
