//! Remote file browser and search (#36, design §18.10): wire messages for `ServiceId::FS_BROWSE` and `ServiceId::FS_WRITE`.
//!
//! Owned by the file_browser feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/file_browser.proto`.
