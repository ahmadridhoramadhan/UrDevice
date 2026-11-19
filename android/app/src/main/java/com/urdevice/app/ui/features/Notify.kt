package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Notification mirroring (#26).
 *
 * Slot for the notify feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object NotifyFeature : Feature {
    override val id = "notify"

    // Shown only when the host lists the capability.
    override fun tiles(modes: Map<String, FfiMode>) = listed(modes, FeatureTile("notify.v1", "notify.v1"))
}
