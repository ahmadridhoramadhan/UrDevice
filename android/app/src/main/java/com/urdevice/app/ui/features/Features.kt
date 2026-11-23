package com.urdevice.app.ui.features

import android.content.Context
import androidx.compose.runtime.Composable
import androidx.compose.runtime.staticCompositionLocalOf
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

    /**
     * For a tile that is a plain switch (no screen, no settings icon): its state for [peer], or
     * null for a normal tile.
     */
    fun switch(peer: FfiPeer): Boolean? = null

    /** The user flipped the switch tile of [peer]. */
    fun setSwitch(peer: FfiPeer, on: Boolean) {}

    /**
     * The settings icon of a tile: return true after handling it here (for example by opening a
     * folder). False opens the Settings screen.
     */
    fun openSettings(context: Context): Boolean = false

    /**
     * The tile key of a settings page for the tile with key [tile], or null. When set, the
     * settings icon of that tile opens the page ([Screen] gets this key as its tile).
     */
    fun settingsPage(tile: String): String? = null

    /** True when tapping a tile opens a menu ([TileMenu]) instead of a screen. */
    val tileMenu: Boolean get() = false

    /** The menu of a [tileMenu] tile, anchored to the tile. Show it while [expanded]. */
    @Composable
    fun TileMenu(peer: FfiPeer, tile: String, expanded: Boolean, onDismiss: () -> Unit) {}

    /** The feature screen for [peer], opened from the tile with key [tile]. */
    @Composable
    fun Screen(peer: FfiPeer, tile: String) {}

    /** Cards this feature shows on the device view under the header, for example a banner. */
    @Composable
    fun DeviceBanner(peer: FfiPeer) {}

    /** Rows this feature adds to the "This device" screen, below the permissions. */
    @Composable
    fun ThisDevice() {}

    /** Settings rows this feature adds to the Settings screen (for example a global switch). */
    @Composable
    fun Settings() {}

    /** Called once after the core started, on the main thread: register host adapters and listeners. */
    fun onCoreStarted(context: Context, node: UrDeviceNode) {}
}

/**
 * Opens another page of a feature for the same device: (feature id, tile key). The screen host
 * provides it, so a feature screen can link to its own settings page.
 */
val LocalOpenFeaturePage = staticCompositionLocalOf<(String, String) -> Unit> { { _, _ -> } }

/** The tiles whose capability the host lists. */
fun listed(modes: Map<String, FfiMode>, vararg tiles: FeatureTile): List<FeatureTile> =
    tiles.filter { modes.containsKey(it.cap) }

/** The feature with this id. */
fun feature(id: String): Feature? = allFeatures.firstOrNull { it.id == id }
