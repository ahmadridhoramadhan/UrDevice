package com.urdevice.app.ui.features

import com.urdevice.core.FfiMode

/**
 * Localhost sharing and Expose on LAN (#22).
 *
 * Slot for the tunnel feature: screens go here, host adapters in
 * `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
object TunnelFeature : Feature {
    override val id = "tunnel"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("tunnel.v1", "tunnel.v1"))
}
