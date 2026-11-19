package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Phone or tablet as a second display (#31).
 *
 * Slot for the display feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object DisplayFeature : Feature {
    override val id = "display"

    // Shown only when the host lists the capability.
    override fun tiles(modes: Map<String, FfiMode>) = listed(modes, FeatureTile("display.v1", "display.v1"))
}
