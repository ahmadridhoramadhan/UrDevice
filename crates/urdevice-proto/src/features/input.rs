//! Remote control: pointer, keyboard, text, and presenter (#18, #19, design §9.3): wire messages for `ServiceId::INPUT`.
//!
//! Owned by the input feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/input.proto`.
