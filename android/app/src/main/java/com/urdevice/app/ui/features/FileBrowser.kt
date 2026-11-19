package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Remote file browser and search (#36).
 *
 * Slot for the file_browser feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object FileBrowserFeature : Feature {
    override val id = "file_browser"

    // Shown only when the host lists the capability.
    override fun tiles(modes: Map<String, FfiMode>) = listed(modes, FeatureTile("fs.browse.v1", "fs.browse.v1"))
}
