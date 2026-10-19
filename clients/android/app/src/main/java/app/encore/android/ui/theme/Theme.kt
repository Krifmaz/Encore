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
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.unit.dp
import app.encore.android.data.session.Settings
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

// the web's :root (data-theme="dark"): the original Encore dark, blue-black
val EncorePaletteNight: EncorePalette = EncorePalette(
    bg = Color(0xFF0B0B12),
    bg2 = Color(0xFF12121C),
    panel = Color(0xFF161624),
    panel2 = Color(0xFF1D1D2E),
    border = Color(0xFF2A2A40),
    text = Color(0xFFECECF4),
    muted = Color(0xFF9A9AB3),
    faint = Color(0xFF6B6B85),
    accent = EncoreColors.Cream,
    accent2 = EncoreColors.CreamDeep,
    onAccent = Color(0xFF16161F),
    dark = true,
)

/** One accent of Settings → Appearance (web-rs/src/appearance.rs ACCENTS): the two colours of its gradient. */
data class Accent(val id: String, val name: String, val a: String, val b: String)

/** One preset of Settings → Appearance (appearance.rs PRESETS): a theme, an accent and a background. */
data class Preset(val id: String, val name: String, val theme: ThemeChoice, val accent: String, val custom: String?, val background: String)

object Appearance {
    val ACCENTS: List<Accent> = listOf(
        Accent("logo", "Logo", "#f0ead6", "#d6c49a"),
        Accent("violet", "Violet", "#8b5cf6", "#ec4899"),
        Accent("encore", "Encore", "#f2a7e4", "#9fd2fb"),
        Accent("lilac", "Lilac", "#b48cf2", "#7cc4fa"),
        Accent("pink", "Pink", "#ec4899", "#f97316"),
        Accent("sky", "Sky", "#0ea5e9", "#8b5cf6"),
        Accent("teal", "Teal", "#14b8a6", "#0ea5e9"),
        Accent("green", "Green", "#22c55e", "#14b8a6"),
        Accent("amber", "Amber", "#f59e0b", "#ef4444"),
        Accent("red", "Red", "#ef4444", "#ec4899"),
    )

    val PRESETS: List<Preset> = listOf(
        Preset("encore", "Encore", ThemeChoice.ENCORE, "logo", null, "solid"),
        Preset("default", "Violet dark", ThemeChoice.DARK, "violet", null, "fade"),
        Preset("oled", "OLED black", ThemeChoice.OLED, "violet", null, "solid"),
        Preset("pastel", "Pastel", ThemeChoice.DARK, "encore", null, "fade"),
        Preset("light", "Daylight", ThemeChoice.LIGHT, "sky", null, "fade"),
        Preset("mono", "Mono", ThemeChoice.OLED, "custom", "#e5e5e5", "solid"),
    )

    const val DEFAULT_CUSTOM = "#f0ead6"

    /** "#abc" / "abc" / "#aabbcc" as "#aabbcc"; null when it is not a hex colour (the web's /^#([0-9a-f]{3}|[0-9a-f]{6})$/i). */
    fun cleanHex(input: String): String? {
        var v = input.trim()
        if (!v.startsWith("#")) v = "#$v"
        val d = v.substring(1)
        if ((d.length != 3 && d.length != 6) || !d.all { it.isDigit() || it.lowercaseChar() in 'a'..'f' }) return null
        return if (d.length == 3) "#" + d.flatMap { listOf(it, it) }.joinToString("").lowercase() else v.lowercase()
    }

    private fun rgb(hex: String): Triple<Int, Int, Int> {
        val h = hex.trimStart('#')
        val full = if (h.length == 3) h.flatMap { listOf(it, it) }.joinToString("") else h
        val n = full.toIntOrNull(16) ?: 0
        return Triple((n shr 16) and 255, (n shr 8) and 255, n and 255)
    }

    fun colorOf(hex: String): Color {
        val (r, g, b) = rgb(hex)
        return Color(r, g, b)
    }

    private fun luminance(hex: String): Double {
        val (r, g, b) = rgb(hex)
        fun lin(v: Int): Double { val x = v / 255.0; return if (x <= 0.03928) x / 12.92 else Math.pow((x + 0.055) / 1.055, 2.4) }
        return lin(r) * 0.2126 + lin(g) * 0.7152 + lin(b) * 0.0722
    }

    /** A custom colour gets a partner shifted around the colour wheel for the gradient (appearance.rs partner). */
    fun partner(hex: String): String {
        val (ri, gi, bi) = rgb(hex)
        val r = ri / 255.0
        val g = gi / 255.0
        val b = bi / 255.0
        val max = maxOf(r, g, b)
        val min = minOf(r, g, b)
        val l = (max + min) / 2
        var h = 0.0
        var s = 0.0
        if (max != min) {
            val d = max - min
            s = if (l > 0.5) d / (2 - max - min) else d / (max + min)
            h = when (max) {
                r -> (g - b) / d + if (g < b) 6 else 0
                g -> (b - r) / d + 2
                else -> (r - g) / d + 4
            }
            h /= 6
        }
        h = (h + 0.1) % 1.0
        val q = if (l < 0.5) l * (1 + s) else l + s - l * s
        val p = 2 * l - q
        fun f(t0: Double): Double {
            val t = ((t0 + 1) % 1.0)
            return when {
                t < 1.0 / 6 -> p + (q - p) * 6 * t
                t < 0.5 -> q
                t < 2.0 / 3 -> p + (q - p) * (2.0 / 3 - t) * 6
                else -> p
            }
        }
        fun c(v: Double): Int = Math.round(v * 255).toInt().coerceIn(0, 255)
        return "#%02x%02x%02x".format(c(f(h + 1.0 / 3)), c(f(h)), c(f(h - 1.0 / 3)))
    }

    /** The accent's two colours as hex (a preset's, or the custom colour and its partner). */
    fun accentHex(accent: String, custom: String): Pair<String, String> {
        val preset = ACCENTS.firstOrNull { it.id == accent }
        if (preset != null) return preset.a to preset.b
        val a = cleanHex(custom) ?: DEFAULT_CUSTOM
        return a to partner(a)
    }

    /** Dark text on light accents, white on dark ones (appearance.rs apply: `--on-accent`). */
    fun onAccent(a: String, b: String): Color = if ((luminance(a) + luminance(b)) / 2 > 0.45) Color(0xFF16161F) else Color.White
}

/**
 * The palette for the account's look: the theme's tokens, the accent's colours (the logo's cream and gold keep the
 * theme's own accent, charcoal on Light) and, for the faded background, a faint wash of the accent in the page colours.
 */
fun paletteOf(settings: Settings, systemDark: Boolean): EncorePalette {
    val base = when (settings.theme) {
        ThemeChoice.ENCORE -> EncorePaletteDark
        ThemeChoice.DARK -> EncorePaletteNight
        ThemeChoice.OLED -> EncorePaletteOled
        ThemeChoice.LIGHT -> EncorePaletteLight
        ThemeChoice.SYSTEM -> if (systemDark) EncorePaletteNight else EncorePaletteLight
    }
    val accented = if (settings.accent == "logo") base else {
        val (a, b) = Appearance.accentHex(settings.accent, settings.custom)
        base.copy(accent = Appearance.colorOf(a), accent2 = Appearance.colorOf(b), onAccent = Appearance.onAccent(a, b))
    }
    if (settings.background != "fade") return accented
    return accented.copy(
        bg = lerp(accented.bg, accented.accent, if (accented.dark) 0.06f else 0.05f),
        bg2 = lerp(accented.bg2, accented.accent, if (accented.dark) 0.04f else 0.03f),
    )
}

/** The theme a setting resolves to on this device (Match device follows the system). */
fun isDarkTheme(settings: Settings, systemDark: Boolean): Boolean = paletteOf(settings, systemDark).dark

/** app.css: --radius 14px for cards, 12px for tiles / menus, 10px for buttons and inputs, 8px for thumbs. */
val EncoreShapes: Shapes = Shapes(
    extraSmall = RoundedCornerShape(8.dp),
    small = RoundedCornerShape(10.dp),
    medium = RoundedCornerShape(12.dp),
    large = RoundedCornerShape(14.dp),
    extraLarge = RoundedCornerShape(20.dp),
)

@Composable
fun EncoreTheme(settings: Settings, content: @Composable () -> Unit) {
    val systemDark = isSystemInDarkTheme()
    val palette = remember(settings, systemDark) { paletteOf(settings, systemDark) }
    val scheme = remember(palette) { schemeOf(palette) }
    CompositionLocalProvider(LocalEncorePalette provides palette) {
        // A Surface at the root sets the content colour for every Text (without it, Compose's default is black)
        MaterialTheme(colorScheme = scheme, typography = EncoreTypography, shapes = EncoreShapes) {
            Surface(color = scheme.background, contentColor = scheme.onBackground, content = content)
        }
    }
}
