package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Shared clipboard history (#37).
 *
 * Slot for the clip_history feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object ClipHistoryFeature : Feature {
    override val id = "clip_history"

    // Shown only when the host lists the capability.
    override fun tiles(modes: Map<String, FfiMode>) = listed(modes, FeatureTile("cliphist.v1", "cliphist.v1"))
}
