package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Screen sharing (#21).
 *
 * Slot for the screen feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object ScreenFeature : Feature {
    override val id = "screen"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("screen.h264.v1", "screen.h264.v1"))
}
