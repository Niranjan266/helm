package com.niranjan.helm.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

val OnlineGreen = Color(0xFF4ADE80)

private val HelmDark = darkColorScheme(
    primary = Color(0xFF7DB4FF),
    onPrimary = Color(0xFF00213F),
    primaryContainer = Color(0xFF1B3A5E),
    onPrimaryContainer = Color(0xFFD3E4FF),
    secondary = Color(0xFFB8C7DB),
    background = Color(0xFF0F1216),
    onBackground = Color(0xFFE2E4E8),
    surface = Color(0xFF0F1216),
    onSurface = Color(0xFFE2E4E8),
    surfaceVariant = Color(0xFF1D2229),
    onSurfaceVariant = Color(0xFFB4BAC4),
    surfaceContainer = Color(0xFF181C22),
    surfaceContainerHigh = Color(0xFF20252C),
    error = Color(0xFFFFB4AB),
)

/** True black surfaces for OLED panels (and less glow on a desk at night). */
private val HelmAmoled = HelmDark.copy(
    background = Color.Black,
    surface = Color.Black,
    surfaceVariant = Color(0xFF111418),
    surfaceContainer = Color(0xFF0A0C0F),
    surfaceContainerHigh = Color(0xFF121519),
    surfaceContainerLow = Color(0xFF050607),
    surfaceContainerHighest = Color(0xFF181B20),
)

@Composable
fun HelmTheme(amoled: Boolean = false, content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (amoled) HelmAmoled else HelmDark, content = content)
}
