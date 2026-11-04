package app.encore.android.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import app.encore.android.data.session.Settings
import app.encore.android.data.session.ThemeChoice
import dev.krifmaz.kit.compose.theme.AppearanceRules
import dev.krifmaz.kit.compose.theme.KitAppearance
import dev.krifmaz.kit.compose.theme.KitLook
import dev.krifmaz.kit.compose.theme.KitPalette
import dev.krifmaz.kit.compose.theme.KitPalettes
import dev.krifmaz.kit.compose.theme.KitShapes
import dev.krifmaz.kit.compose.theme.KitStatusColors
import dev.krifmaz.kit.compose.theme.KitTheme
import dev.krifmaz.kit.compose.theme.LocalKitPalette
import dev.krifmaz.kit.compose.theme.kitPaletteOf

// Encore's look comes from the kit (Krifmaz/kit design/tokens.json → kit-compose's theme), which holds Encore's
// palettes, accents and shapes exactly as they were here. These names stay for the rest of the app.

// The logo's colours (web/css/app.css :root[data-theme="encore"]): charcoal and cream.
object EncoreColors {
    val Background = KitPalettes.Encore.bg
    val Surface = KitPalettes.Encore.bg2
    val SurfaceContainer = KitPalettes.Encore.panel
    val SurfaceContainerHigh = KitPalettes.Encore.panel2
    val Outline = KitPalettes.Encore.border
    val Cream = KitPalettes.Encore.accent
    val CreamDeep = KitPalettes.Encore.accent2
    val Muted = KitPalettes.Encore.muted
    val Faint = KitPalettes.Encore.faint

    // app.css :root status colours (--ok, --warn, --err, --info); the same in every theme
    val Ok = KitStatusColors.Ok
    val Warn = KitStatusColors.Warn
    val Err = KitStatusColors.Err
    val Info = KitStatusColors.Info

    /** The accent: cream into gold, as on the logo (the web's --grad for this theme). */
    val AccentGradient: Brush = Brush.linearGradient(listOf(Cream, CreamDeep))
}

/**
 * The web's CSS variables for one theme (app.css `:root[data-theme=…]`), so screens can use the same tokens the
 * web player does: --bg, --bg-2, --panel, --panel-2, --border, --text, --muted, --faint, --accent / --accent-2.
 */
typealias EncorePalette = KitPalette

/** The Encore theme (the logo's charcoal and cream). */
val EncorePaletteDark: EncorePalette = KitPalettes.Encore

// app.css :root[data-theme="oled"]: true black, so pixels switch off.
val EncorePaletteOled: EncorePalette = KitPalettes.Oled

// app.css :root[data-theme="light"]; the accent turns charcoal so it reads on white.
val EncorePaletteLight: EncorePalette = KitPalettes.Light

// the web's :root (data-theme="dark"): the original Encore dark, blue-black
val EncorePaletteNight: EncorePalette = KitPalettes.Default

/** The current theme's palette (the kit's [LocalKitPalette]). */
val LocalEncorePalette = LocalKitPalette

/** The current theme's web tokens (`MaterialTheme.colorScheme` carries the same colours for Material widgets). */
val MaterialTheme.palette: EncorePalette
    @Composable
    @ReadOnlyComposable
    get() = LocalKitPalette.current

/** One accent of Settings → Appearance (web-rs/src/appearance.rs ACCENTS): the two colours of its gradient. */
data class Accent(val id: String, val name: String, val a: String, val b: String)

/** One preset of Settings → Appearance (appearance.rs PRESETS): a theme, an accent and a background. */
data class Preset(val id: String, val name: String, val theme: ThemeChoice, val accent: String, val custom: String?, val background: String)

object Appearance {
    val ACCENTS: List<Accent> = KitAppearance.ACCENTS.map { Accent(it.id, it.name, it.a, it.b) }

    val PRESETS: List<Preset> = KitAppearance.PRESETS.map { Preset(it.id, it.name, ThemeChoice.of(it.theme), it.accent, it.custom, it.background) }

    const val DEFAULT_CUSTOM = KitAppearance.DEFAULT_CUSTOM

    /** "#abc" / "abc" / "#aabbcc" as "#aabbcc"; null when it is not a hex colour (the web's /^#([0-9a-f]{3}|[0-9a-f]{6})$/i). */
    fun cleanHex(input: String): String? = AppearanceRules.cleanHex(input)

    fun colorOf(hex: String): Color = AppearanceRules.colorOf(hex)

    /** A custom colour gets a partner shifted around the colour wheel for the gradient (appearance.rs partner). */
    fun partner(hex: String): String = AppearanceRules.partner(hex)

    /** The accent's two colours as hex (a preset's, or the custom colour and its partner). */
    fun accentHex(accent: String, custom: String): Pair<String, String> = AppearanceRules.accentHex(accent, custom)

    /** Dark text on light accents, white on dark ones (appearance.rs apply: `--on-accent`). */
    fun onAccent(a: String, b: String): Color = AppearanceRules.onAccent(a, b)
}

/**
 * The palette for the account's look: the theme's tokens, the accent's colours (the logo's cream and gold keep the
 * theme's own accent, charcoal on Light) and, for the faded background, a faint wash of the accent in the page colours.
 */
fun paletteOf(settings: Settings, systemDark: Boolean): EncorePalette =
    kitPaletteOf(KitLook(KitTheme.of(settings.theme.id), settings.accent, settings.custom, settings.background), systemDark)

/** The theme a setting resolves to on this device (Match device follows the system). */
fun isDarkTheme(settings: Settings, systemDark: Boolean): Boolean = paletteOf(settings, systemDark).dark

/** app.css: --radius 14px for cards, 12px for tiles / menus, 10px for buttons and inputs, 8px for thumbs. */
val EncoreShapes: Shapes = KitShapes

/**
 * The app's theme: the account's palette, through the kit's [KitTheme] (Material 3 with Encore's colours, type and
 * shapes, the focus ring on every clickable for TV remotes and keyboards, and a root Surface for the content colour).
 *
 * @param settings the account's look (theme, accent, background) the palette is made from.
 * @param content the app's UI.
 */
@Composable
fun EncoreTheme(settings: Settings, content: @Composable () -> Unit) {
    val systemDark = isSystemInDarkTheme()
    val palette = remember(settings, systemDark) { paletteOf(settings, systemDark) }
    KitTheme(palette, content)
}
