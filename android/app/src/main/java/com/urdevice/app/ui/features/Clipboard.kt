package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Clipboard sharing (#16).
 *
 * Slot for the clipboard feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object ClipboardFeature : Feature {
    override val id = "clipboard"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("clipboard.v1", "clipboard.v1"))
}
