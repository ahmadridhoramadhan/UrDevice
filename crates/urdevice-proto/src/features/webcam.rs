//! Phone as webcam and microphone (#28, design §18.1): wire messages for `ServiceId::CAMERA` and `ServiceId::MIC`.
//!
//! Owned by the webcam feature. Field numbers and stream kinds are local to
//! this service, so they never clash with another feature. Mirror each
//! message in `proto/features/webcam.proto`.
