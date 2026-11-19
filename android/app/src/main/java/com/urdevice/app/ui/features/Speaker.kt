package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Use another device as a speaker (#40).
 *
 * Slot for the speaker feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object SpeakerFeature : Feature {
    override val id = "speaker"

    // Shown only when the host lists the capability.
    override fun tiles(modes: Map<String, FfiMode>) = listed(modes, FeatureTile("audio.out.v1", "audio.out.v1"))
}
