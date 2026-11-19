package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Media control (#20).
 *
 * Slot for the media feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object MediaFeature : Feature {
    override val id = "media"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("media.v1", "media.v1"))
}
