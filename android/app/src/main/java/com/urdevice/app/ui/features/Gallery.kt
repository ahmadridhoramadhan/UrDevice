package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Android gallery (#17).
 *
 * Slot for the gallery feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object GalleryFeature : Feature {
    override val id = "gallery"

    // Shown only when the host lists the capability.
    override fun tiles(modes: Map<String, FfiMode>) = listed(modes, FeatureTile("gallery.v1", "gallery.v1"))
}
