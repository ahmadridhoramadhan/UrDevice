//! Android gallery (#17, design §9.2): wire messages for `ServiceId::GALLERY`.
//!
//! Owned by the gallery feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/gallery.proto`.
