package com.urdevice.app.ui.features

/**
 * Shared mouse and keyboard across desktops (#30).
 *
 * Slot for the kvm feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object KvmFeature : Feature {
    override val id = "kvm"
}
