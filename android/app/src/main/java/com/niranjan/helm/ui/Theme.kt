package com.niranjan.helm.ui

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import com.niranjan.helm.R
import com.niranjan.helm.device.ThemeMode

/**
 * Blueprint design tokens (same names as the website's `tokens.css`). Dark = blueprint (white
 * line on blue paper), light = whiteprint (blue line on white paper), AMOLED = deep (blueprint
 * lines on near-black). Pencil yellow, red and the bubble colours stay the same everywhere.
 */
@Immutable
data class BlueprintColors(
    val paper: Color,
    val paper2: Color,
    val deep: Color,
    val ink: Color,
    val ink2: Color,
    /** Yellow fill for the primary action (text on it is [deep]). */
    val pencil: Color,
    /** Yellow-ish accent for text and lines on [paper]. */
    val pencilInk: Color,
    val red: Color,
    /** Red for text on [paper]. */
    val redInk: Color,
    val ok: Color,
    val bubble: Color,
    val bubbleInk: Color,
    val grid: Color,
    val gridFine: Color,
    val dots: Color,
    /** Background of a FIG. panel. */
    val panel: Color,
    /** The touch area / input wells. */
    val well: Color,
    val fillSoft: Color,
    val fillBlue: Color,
    /** Hairlines between rows. */
    val line: Color,
    val isLight: Boolean,
)

val Blueprint = BlueprintColors(
    paper = Color(0xFF0D3A86),
    paper2 = Color(0xFF11469A),
    deep = Color(0xFF061F4D),
    ink = Color(0xFFEAF2FF),
    ink2 = Color(0xFF9DB8E8),
    pencil = Color(0xFFFFD84D),
    pencilInk = Color(0xFFFFD84D),
    red = Color(0xFFFF5A47),
    redInk = Color(0xFFFFB3A8),
    ok = Color(0xFF4ADE80),
    bubble = Color.White,
    bubbleInk = Color(0xFF061F4D),
    grid = Color.White.copy(alpha = 0.07f),
    gridFine = Color.White.copy(alpha = 0.03f),
    dots = Color(0xFFEAF2FF).copy(alpha = 0.12f),
    panel = Color(0xFF061F4D).copy(alpha = 0.45f),
    well = Color(0xFF061F4D),
    fillSoft = Color(0xFFEAF2FF).copy(alpha = 0.14f),
    fillBlue = Color(0xFF2A63C4),
    line = Color(0xFFEAF2FF).copy(alpha = 0.3f),
    isLight = false,
)

val Whiteprint = BlueprintColors(
    paper = Color(0xFFF3F6FB),
    paper2 = Color.White,
    deep = Color(0xFF0B2A66),
    ink = Color(0xFF0D3A86),
    ink2 = Color(0xFF4A6AA3),
    pencil = Color(0xFFFFD84D),
    pencilInk = Color(0xFFA86A00),
    red = Color(0xFFFF5A47),
    redInk = Color(0xFFC0321F),
    ok = Color(0xFF15803D),
    bubble = Color.White,
    bubbleInk = Color(0xFF0B2A66),
    grid = Color(0xFF0D3A86).copy(alpha = 0.10f),
    gridFine = Color(0xFF0D3A86).copy(alpha = 0.045f),
    dots = Color(0xFF0D3A86).copy(alpha = 0.10f),
    panel = Color.White.copy(alpha = 0.75f),
    well = Color(0xFFDFE8F6),
    fillSoft = Color(0xFF0D3A86).copy(alpha = 0.07f),
    fillBlue = Color(0xFF2A63C4),
    line = Color(0xFF0D3A86).copy(alpha = 0.18f),
    isLight = true,
)

/** AMOLED: the blueprint lines on (almost) black paper. */
val DeepPrint = Blueprint.copy(
    paper = Color.Black,
    paper2 = Color(0xFF071533),
    grid = Color(0xFF9DB8E8).copy(alpha = 0.07f),
    gridFine = Color(0xFF9DB8E8).copy(alpha = 0.03f),
    panel = Color(0xFF061F4D).copy(alpha = 0.35f),
    well = Color(0xFF030C20),
)

val LocalBlueprint = staticCompositionLocalOf { Blueprint }

/** Shortcut for the current blueprint tokens: `Bp.c.pencil`. */
object Bp {
    val c: BlueprintColors
        @Composable @ReadOnlyComposable get() = LocalBlueprint.current
}

/** Kept for older call sites: the "online" green. */
val OnlineGreen = Color(0xFF4ADE80)

// ---- Type ---------------------------------------------------------------------------------

@OptIn(ExperimentalTextApi::class)
private fun archivo(w: Int) = Font(
    R.font.archivo_variable,
    FontWeight(w),
    variationSettings = FontVariation.Settings(FontVariation.weight(w)),
)

@OptIn(ExperimentalTextApi::class)
private fun plexSans(w: Int) = Font(
    R.font.ibm_plex_sans_variable,
    FontWeight(w),
    variationSettings = FontVariation.Settings(FontVariation.weight(w)),
)

/** Archivo: heavy, uppercase display and button text. */
val ArchivoFamily = FontFamily(archivo(600), archivo(700), archivo(800), archivo(900))

/** IBM Plex Sans: body text. */
val PlexSansFamily = FontFamily(plexSans(400), plexSans(500), plexSans(600), plexSans(700))

/** IBM Plex Mono: labels, status lines, keys. */
val PlexMonoFamily = FontFamily(
    Font(R.font.ibm_plex_mono_regular, FontWeight.Normal),
    Font(R.font.ibm_plex_mono_medium, FontWeight.Medium),
    Font(R.font.ibm_plex_mono_semibold, FontWeight.SemiBold),
    Font(R.font.ibm_plex_mono_semibold, FontWeight.Bold),
)

/** Comic Neue Bold: speech bubbles only. */
val ComicFamily = FontFamily(Font(R.font.comic_neue_bold, FontWeight.Bold))

/** Extra text styles the Material type scale has no slot for. */
object BpType {
    val display = TextStyle(fontFamily = ArchivoFamily, fontWeight = FontWeight.Black, fontSize = 34.sp, lineHeight = 35.sp)
    val title = TextStyle(fontFamily = ArchivoFamily, fontWeight = FontWeight.ExtraBold, fontSize = 18.sp, lineHeight = 22.sp)
    val button = TextStyle(fontFamily = ArchivoFamily, fontWeight = FontWeight.ExtraBold, fontSize = 15.sp, lineHeight = 18.sp)
    val mono = TextStyle(fontFamily = PlexMonoFamily, fontWeight = FontWeight.Normal, fontSize = 11.sp, lineHeight = 15.sp, letterSpacing = 0.1.em)
    val monoStrong = mono.copy(fontWeight = FontWeight.SemiBold, fontSize = 12.sp, letterSpacing = 0.04.em)
    val bubble = TextStyle(fontFamily = ComicFamily, fontWeight = FontWeight.Bold, fontSize = 14.sp, lineHeight = 18.sp)
}

private val HelmTypography: Typography = Typography().let { t ->
    fun TextStyle.display(w: FontWeight = FontWeight.Black) = copy(fontFamily = ArchivoFamily, fontWeight = w)
    fun TextStyle.body(w: FontWeight = FontWeight.Normal) = copy(fontFamily = PlexSansFamily, fontWeight = w)
    fun TextStyle.mono(w: FontWeight = FontWeight.Medium) = copy(fontFamily = PlexMonoFamily, fontWeight = w, letterSpacing = 0.06.em)
    Typography(
        displayLarge = t.displayLarge.display(),
        displayMedium = t.displayMedium.display(),
        displaySmall = t.displaySmall.display(),
        headlineLarge = t.headlineLarge.display(),
        headlineMedium = t.headlineMedium.display(),
        headlineSmall = t.headlineSmall.display(FontWeight.ExtraBold),
        titleLarge = t.titleLarge.display(FontWeight.ExtraBold),
        titleMedium = t.titleMedium.body(FontWeight.SemiBold),
        titleSmall = t.titleSmall.mono(FontWeight.SemiBold),
        bodyLarge = t.bodyLarge.body(),
        bodyMedium = t.bodyMedium.body(),
        bodySmall = t.bodySmall.body(),
        // labelLarge is Material's button text: Archivo, like the mockups' buttons.
        labelLarge = t.labelLarge.display(FontWeight.ExtraBold).copy(letterSpacing = 0.02.em),
        labelMedium = t.labelMedium.mono(),
        labelSmall = t.labelSmall.mono(),
    )
}

// ---- Shape --------------------------------------------------------------------------------

/** Square, drafting-table corners. */
val HelmShapes = Shapes(
    extraSmall = RoundedCornerShape(1.dp),
    small = RoundedCornerShape(2.dp),
    medium = RoundedCornerShape(2.dp),
    large = RoundedCornerShape(3.dp),
    extraLarge = RoundedCornerShape(4.dp),
)

// ---- Colour scheme ------------------------------------------------------------------------

private fun schemeFor(c: BlueprintColors): ColorScheme {
    // Opaque mixes for the Material container slots (panels are drawn with alpha on the grid).
    fun mix(a: Color, b: Color, t: Float) = Color(
        red = a.red + (b.red - a.red) * t,
        green = a.green + (b.green - a.green) * t,
        blue = a.blue + (b.blue - a.blue) * t,
        alpha = 1f,
    )
    return if (c.isLight) {
        lightColorScheme(
            primary = c.ink, onPrimary = Color.White,
            primaryContainer = c.pencil, onPrimaryContainer = c.deep,
            secondary = c.ink2, onSecondary = Color.White,
            secondaryContainer = Color(0xFFDFE8F6), onSecondaryContainer = c.ink,
            tertiary = c.ok, onTertiary = Color.White,
            tertiaryContainer = c.pencil, onTertiaryContainer = c.deep,
            background = c.paper, onBackground = c.ink,
            surface = c.paper, onSurface = c.ink,
            surfaceVariant = Color(0xFFE6EDF8), onSurfaceVariant = c.ink2,
            surfaceTint = Color.Transparent,
            surfaceBright = Color.White, surfaceDim = Color(0xFFE6EDF8),
            surfaceContainerLowest = Color.White,
            surfaceContainerLow = Color(0xFFF8FAFD),
            surfaceContainer = Color.White,
            surfaceContainerHigh = Color.White,
            surfaceContainerHighest = Color(0xFFEAF0F9),
            outline = c.ink, outlineVariant = mix(c.paper, c.ink, 0.3f),
            error = c.redInk, onError = Color.White,
            errorContainer = Color(0xFFFFE1DC), onErrorContainer = c.redInk,
            inverseSurface = c.deep, inverseOnSurface = Color.White, inversePrimary = c.pencil,
            scrim = Color.Black,
        )
    } else {
        darkColorScheme(
            primary = c.pencil, onPrimary = c.deep,
            primaryContainer = c.pencil, onPrimaryContainer = c.deep,
            secondary = c.ink2, onSecondary = c.deep,
            secondaryContainer = c.paper2, onSecondaryContainer = c.ink,
            tertiary = c.ok, onTertiary = c.deep,
            tertiaryContainer = c.pencil, onTertiaryContainer = c.deep,
            background = c.paper, onBackground = c.ink,
            surface = c.paper, onSurface = c.ink,
            surfaceVariant = mix(c.deep, c.paper, 0.6f), onSurfaceVariant = c.ink2,
            surfaceTint = Color.Transparent,
            surfaceBright = c.paper2, surfaceDim = c.deep,
            surfaceContainerLowest = c.deep,
            surfaceContainerLow = mix(c.deep, c.paper, 0.4f),
            surfaceContainer = mix(c.deep, c.paper, 0.55f),
            surfaceContainerHigh = mix(c.deep, c.paper, 0.7f),
            surfaceContainerHighest = c.paper2,
            outline = c.ink, outlineVariant = mix(c.paper, c.ink, 0.35f),
            error = c.red, onError = c.deep,
            errorContainer = mix(c.deep, c.red, 0.25f), onErrorContainer = c.redInk,
            inverseSurface = c.ink, inverseOnSurface = c.deep, inversePrimary = c.paper,
            scrim = Color.Black,
        )
    }
}

fun blueprintFor(mode: ThemeMode): BlueprintColors = when (mode) {
    ThemeMode.DARK -> Blueprint
    ThemeMode.LIGHT -> Whiteprint
    ThemeMode.AMOLED -> DeepPrint
}

@Composable
fun HelmTheme(mode: ThemeMode = ThemeMode.DARK, content: @Composable () -> Unit) {
    val c = blueprintFor(mode)
    CompositionLocalProvider(LocalBlueprint provides c) {
        MaterialTheme(colorScheme = androidx.compose.runtime.remember(c) { schemeFor(c) }, typography = HelmTypography, shapes = HelmShapes, content = content)
    }
}

// ---- Paper --------------------------------------------------------------------------------

/**
 * Grid paper: the paper colour with a fine 24 dp grid and a stronger 120 dp grid, aligned to
 * the top-left of the element (as the website's `.bp` background).
 */
fun Modifier.gridPaper(c: BlueprintColors): Modifier = drawBehind {
    drawRect(c.paper)
    val fine = 24.dp.toPx()
    val stroke = 1.dp.toPx().coerceAtLeast(1f)
    var x = 0f
    var i = 0
    while (x <= size.width) {
        drawLine(if (i % 5 == 0) c.grid else c.gridFine, Offset(x, 0f), Offset(x, size.height), stroke)
        x += fine
        i++
    }
    var y = 0f
    i = 0
    while (y <= size.height) {
        drawLine(if (i % 5 == 0) c.grid else c.gridFine, Offset(0f, y), Offset(size.width, y), stroke)
        y += fine
        i++
    }
}

/** Dotted well (the touch area): deep fill with a dot every 18 dp. */
fun Modifier.dotPaper(c: BlueprintColors): Modifier = drawBehind {
    drawRect(c.well)
    val step = 18.dp.toPx()
    val r = 1.dp.toPx()
    var y = step / 2
    while (y < size.height) {
        var x = step / 2
        while (x < size.width) {
            drawCircle(c.dots, radius = r, center = Offset(x, y))
            x += step
        }
        y += step
    }
}
