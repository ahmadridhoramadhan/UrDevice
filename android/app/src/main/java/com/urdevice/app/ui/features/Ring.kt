package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Find my device (#27).
 *
 * Slot for the ring feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object RingFeature : Feature {
    override val id = "ring"

    // Shown only when the host lists the capability.
    override fun tiles(modes: Map<String, FfiMode>) = listed(modes, FeatureTile("ring.v1", "ring.v1"))
}
