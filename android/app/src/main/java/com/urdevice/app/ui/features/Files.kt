package com.urdevice.app.ui.features

import com.urdevice.app.R
import com.urdevice.core.FfiMode

/**
 * File transfer (#17).
 *
 * Slot for the files feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object FilesFeature : Feature {
    override val id = "files"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("files.v1", "files.v1", R.string.tile_send_files))
}
