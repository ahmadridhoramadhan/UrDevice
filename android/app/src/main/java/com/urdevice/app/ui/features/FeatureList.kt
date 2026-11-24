package com.urdevice.app.ui.features

/** Every Android feature, in the order of the device-view tiles. */
val allFeatures: List<Feature> = listOf(
    // First: its player sits under the device info and above the file transfers (no tile of its own).
    MediaFeature,
    ClipboardFeature,
    FilesFeature,
    GalleryFeature,
    InputFeature,
    ScreenFeature,
    TunnelFeature,
    BluetoothFeature,
    NotifyFeature,
    RingFeature,
    WebcamFeature,
    TelephonyFeature,
    KvmFeature,
    DisplayFeature,
    TerminalFeature,
    PortProfilesFeature,
    InspectorFeature,
    AdbFeature,
    FileBrowserFeature,
    ClipHistoryFeature,
    ProximityLockFeature,
    AutomationsFeature,
    SpeakerFeature,
)
