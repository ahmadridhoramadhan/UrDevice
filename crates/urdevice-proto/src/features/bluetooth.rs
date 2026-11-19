//! Bluetooth fallback (#23, design §8): wire messages for `ServiceId::BLUETOOTH`.
//!
//! Owned by the bluetooth feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/bluetooth.proto`.
