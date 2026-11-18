package com.urdevice.app.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color

val Forest = Color(0xFF0F1F10)
val Lime = Color(0xFFC6F19D)

/** Colors for the Allow / Always Ask / Deny states. */
@Immutable
data class ModeColors(
    val allowBg: Color, val allowFg: Color,
    val askBg: Color, val askFg: Color,
    val denyBg: Color, val denyFg: Color,
    val warnBg: Color, val online: Color, val offline: Color,
    val header: Color, val onHeader: Color,
)

private val lightModes = ModeColors(
    Color(0xFFD7F2B5), Color(0xFF2E5D12), Color(0xFFF6E7C8), Color(0xFF8A5C12),
    Color(0xFFF1DDDD), Color(0xFF8A2C2C), Color(0xFFFBEFD2), Color(0xFF1F8A6A), Color(0xFFC9B98F),
    Forest, Color(0xFFF4F7EF),
)
private val darkModes = ModeColors(
    Color(0xFF2F4D1F), Color(0xFFD7F2B5), Color(0xFF4A3B17), Color(0xFFF3D9A0),
    Color(0xFF3B2222), Color(0xFFF0B8B8), Color(0xFF4A3B17), Color(0xFF3CCB9C), Color(0xFFC9B98F),
    Color(0xFF1F3321), Color(0xFFF4F7EF),
)

val LocalModes = staticCompositionLocalOf { lightModes }

@Composable
fun UrDeviceTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val scheme = if (dark) {
        darkColorScheme(
            primary = Lime, onPrimary = Forest, background = Color(0xFF0C180D), onBackground = Color(0xFFE8F5DC),
            surface = Color(0xFF142415), onSurface = Color(0xFFE8F5DC), surfaceVariant = Color(0xFF1A2E1C),
            onSurfaceVariant = Color(0xFF9FB59A), outline = Color(0xFF2A3D2C), secondaryContainer = Color(0xFF2F4D1F),
        )
    } else {
        lightColorScheme(
            primary = Color(0xFF1F4D24), onPrimary = Color.White, background = Color(0xFFF4F0E8),
            onBackground = Color(0xFF1C2A1D), surface = Color(0xFFFFFDF8), onSurface = Color(0xFF1C2A1D),
            surfaceVariant = Color(0xFFF6F2EA), onSurfaceVariant = Color(0xFF6B7468), outline = Color(0xFFE6DFD2),
            secondaryContainer = Color(0xFFD7F2B5),
        )
    }
    androidx.compose.runtime.CompositionLocalProvider(LocalModes provides if (dark) darkModes else lightModes) {
        MaterialTheme(colorScheme = scheme, content = content)
    }
}
