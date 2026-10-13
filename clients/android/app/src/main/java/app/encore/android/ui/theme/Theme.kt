package app.encore.android.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Surface
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import app.encore.android.data.session.ThemeChoice

// The logo's colours (web/css/app.css :root[data-theme="encore"]): charcoal and cream.
object EncoreColors {
    val Background = Color(0xFF161617)
    val Surface = Color(0xFF1C1C1E)
    val SurfaceContainer = Color(0xFF222224)
    val SurfaceContainerHigh = Color(0xFF2A2A2D)
    val Outline = Color(0xFF353538)
    val Cream = Color(0xFFF0EAD6)
    val CreamDeep = Color(0xFFD6C49A)
    val Muted = Color(0xFFAAA493)
    val Faint = Color(0xFF77725F)

    // app.css :root status colours (--ok, --warn, --err, --info); the same in every theme
    val Ok = Color(0xFF22C55E)
    val Warn = Color(0xFFF59E0B)
    val Err = Color(0xFFEF4444)
    val Info = Color(0xFF38BDF8)

    /** The accent: cream into gold, as on the logo (the web's --grad for this theme). */
    val AccentGradient: Brush = Brush.linearGradient(listOf(Cream, CreamDeep))
}

/**
 * The web's CSS variables for one theme (app.css `:root[data-theme=…]`), so screens can use the same tokens the
 * web player does: --bg, --bg-2, --panel, --panel-2, --border, --text, --muted, --faint, --accent / --accent-2.
 */
@Immutable
data class EncorePalette(
    val bg: Color,
    val bg2: Color,
    val panel: Color,
    val panel2: Color,
    val border: Color,
    val text: Color,
    val muted: Color,
    val faint: Color,
    val accent: Color,
    val accent2: Color,
    val onAccent: Color,
    val dark: Boolean,
) {
    /** `--grad`: accent into accent-2 (cream → gold on the dark themes). */
    val gradient: Brush get() = Brush.linearGradient(listOf(accent, accent2))
}

val EncorePaletteDark: EncorePalette = EncorePalette(
    bg = EncoreColors.Background,
    bg2 = EncoreColors.Surface,
    panel = EncoreColors.SurfaceContainer,
    panel2 = EncoreColors.SurfaceContainerHigh,
    border = EncoreColors.Outline,
    text = EncoreColors.Cream,
    muted = EncoreColors.Muted,
    faint = EncoreColors.Faint,
    accent = EncoreColors.Cream,
    accent2 = EncoreColors.CreamDeep,
    onAccent = EncoreColors.Background,
    dark = true,
)

// app.css :root[data-theme="oled"]: true black, so pixels switch off.
val EncorePaletteOled: EncorePalette = EncorePalette(
    bg = Color.Black,
    bg2 = Color.Black,
    panel = Color(0xFF0B0B0D),
    panel2 = Color(0xFF17171B),
    border = Color(0xFF232329),
    text = EncoreColors.Cream,
    muted = Color(0xFF9A9AA8),
    faint = Color(0xFF66666F),
    accent = EncoreColors.Cream,
    accent2 = EncoreColors.CreamDeep,
    onAccent = Color.Black,
    dark = true,
)

// app.css :root[data-theme="light"]; the accent turns charcoal so it reads on white.
val EncorePaletteLight: EncorePalette = EncorePalette(
    bg = Color(0xFFF6F6FB),
    bg2 = Color(0xFFFFFFFF),
    panel = Color(0xFFFFFFFF),
    panel2 = Color(0xFFF0F0F7),
    border = Color(0xFFE2E2EE),
    text = Color(0xFF16161F),
    muted = Color(0xFF5D5D75),
    faint = Color(0xFF8D8DA5),
    accent = Color(0xFF2A2A2D),
    accent2 = Color(0xFF8A7A4E),
    onAccent = EncoreColors.Cream,
    dark = false,
)

val LocalEncorePalette = staticCompositionLocalOf { EncorePaletteDark }

/** The current theme's web tokens (`MaterialTheme.colorScheme` carries the same colours for Material widgets). */
val MaterialTheme.palette: EncorePalette
    @Composable
    @ReadOnlyComposable
    get() = LocalEncorePalette.current

private fun schemeOf(p: EncorePalette): ColorScheme {
    val base = if (p.dark) darkColorScheme() else lightColorScheme()
    return base.copy(
        primary = p.accent,
        onPrimary = p.onAccent,
        primaryContainer = p.panel2,
        onPrimaryContainer = p.text,
        secondary = p.accent2,
        onSecondary = p.onAccent,
        secondaryContainer = p.panel2,
        onSecondaryContainer = p.text,
        tertiary = p.accent2,
        onTertiary = p.onAccent,
        background = p.bg,
        onBackground = p.text,
        surface = p.bg2,
        onSurface = p.text,
        surfaceVariant = p.panel,
        onSurfaceVariant = p.muted,
        surfaceContainerLowest = p.bg,
        surfaceContainerLow = p.bg2,
        surfaceContainer = p.panel,
        surfaceContainerHigh = p.panel2,
        surfaceContainerHighest = p.border,
        surfaceTint = Color.Transparent,
        outline = p.border,
        outlineVariant = p.faint,
        error = EncoreColors.Err,
        onError = Color.White,
    )
}

private val EncoreScheme: ColorScheme = schemeOf(EncorePaletteDark)
private val OledScheme: ColorScheme = schemeOf(EncorePaletteOled)
private val LightScheme: ColorScheme = schemeOf(EncorePaletteLight)

/** app.css: --radius 14px for cards, 12px for tiles / menus, 10px for buttons and inputs, 8px for thumbs. */
val EncoreShapes: Shapes = Shapes(
    extraSmall = RoundedCornerShape(8.dp),
    small = RoundedCornerShape(10.dp),
    medium = RoundedCornerShape(12.dp),
    large = RoundedCornerShape(14.dp),
    extraLarge = RoundedCornerShape(20.dp),
)

fun isDarkTheme(theme: ThemeChoice): Boolean = theme != ThemeChoice.LIGHT

fun paletteOf(theme: ThemeChoice): EncorePalette = when (theme) {
    ThemeChoice.ENCORE -> EncorePaletteDark
    ThemeChoice.OLED -> EncorePaletteOled
    ThemeChoice.LIGHT -> EncorePaletteLight
}

@Composable
fun EncoreTheme(theme: ThemeChoice, content: @Composable () -> Unit) {
    val scheme = when (theme) {
        ThemeChoice.ENCORE -> EncoreScheme
        ThemeChoice.OLED -> OledScheme
        ThemeChoice.LIGHT -> LightScheme
    }
    CompositionLocalProvider(LocalEncorePalette provides paletteOf(theme)) {
        // A Surface at the root sets the content colour for every Text (without it, Compose's default is black)
        MaterialTheme(colorScheme = scheme, typography = EncoreTypography, shapes = EncoreShapes) {
            Surface(color = scheme.background, contentColor = scheme.onBackground, content = content)
        }
    }
}
