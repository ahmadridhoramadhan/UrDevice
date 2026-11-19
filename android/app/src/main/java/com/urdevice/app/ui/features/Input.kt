package com.urdevice.app.ui.features

import com.urdevice.app.R
import com.urdevice.core.FfiMode

/**
 * Remote control: pointer, keyboard, text, and presenter (#18, #19).
 *
 * Slot for the input feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object InputFeature : Feature {
    override val id = "input"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(
        FeatureTile("input.v1", "input.v1", R.string.tile_mousepad),
        FeatureTile("input.v1", "presenter", R.string.tile_presenter),
        FeatureTile("input.v1", "keyboard", R.string.tile_keyboard),
    )
}
