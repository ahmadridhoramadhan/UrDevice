package com.urdevice.app.ui.features

import android.content.Context
import androidx.compose.runtime.Composable
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPeer
import com.urdevice.core.UrDeviceNode

/**
 * One tile on the paired-device view. [cap] is the host's capability it uses: the tile follows the
 * host's Allow / Always Ask / Deny for it. [key] picks the icon (`capIcon`). [label] is a string
 * resource; without one the capability's name is shown.
 */
data class FeatureTile(val cap: String, val key: String, val label: Int? = null)

/**
 * One Android feature (design §13). Each feature is one file in this package, listed in
 * [allFeatures], so features never edit the shared screens. Host adapters (the Kotlin side of the
 * core's platform traits) go in `com.urdevice.app.host`. See docs/dev/adding-a-feature.md.
 */
interface Feature {
    /** The feature id, the same as the core module name, for example `clipboard`. */
    val id: String

    /** Tiles on the device view, in order. [modes] is the host's PermissionState. */
    fun tiles(modes: Map<String, FfiMode>): List<FeatureTile> = emptyList()

    /** True when a tile opens [Screen]; otherwise a tile only sends an activity request. */
    val hasScreen: Boolean get() = false

    /** The feature screen for [peer], opened from the tile with key [tile]. */
    @Composable
    fun Screen(peer: FfiPeer, tile: String) {}

    /** Settings rows this feature adds to the Settings screen (for example a global switch). */
    @Composable
    fun Settings() {}

    /** Called once after the core started, on the main thread: register host adapters and listeners. */
    fun onCoreStarted(context: Context, node: UrDeviceNode) {}
}

/** The tiles whose capability the host lists. */
fun listed(modes: Map<String, FfiMode>, vararg tiles: FeatureTile): List<FeatureTile> =
    tiles.filter { modes.containsKey(it.cap) }

/** The feature with this id. */
fun feature(id: String): Feature? = allFeatures.firstOrNull { it.id == id }
