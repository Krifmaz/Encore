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
import dev.krifmaz.kit.compose.theme.KitPaletteSet
import dev.krifmaz.kit.compose.theme.KitPalettes
import dev.krifmaz.kit.compose.theme.KitShapes
import dev.krifmaz.kit.compose.theme.KitStatusColors
import dev.krifmaz.kit.compose.theme.KitTheme
import dev.krifmaz.kit.compose.theme.LocalKitPalette
import dev.krifmaz.kit.compose.theme.kitPaletteOf

// Encore's look comes from the kit (Krifmaz/kit design/tokens.json → kit-compose's theme): its themes, accents, shapes
// and colour maths. Where Encore's colours differ from the kit's neutral ones (the Dark theme's cream accent, the OLED
// theme's cream text, the Encore theme's text on the accent) and where its account settings use other accent ids
// ("logo", "encore"), Encore keeps its own here, so nothing looks different. These names stay for the rest of the app.

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

/** The Encore theme (the logo's charcoal and cream); text on the accent is the theme's own charcoal. */
val EncorePaletteDark: EncorePalette = KitPalettes.Encore.copy(onAccent = Color(0xFF161617))

/** app.css :root[data-theme="oled"]: true black, so pixels switch off; cream text, black on the accent. */
val EncorePaletteOled: EncorePalette = KitPalettes.Oled.copy(text = Color(0xFFF0EAD6), onAccent = Color(0xFF000000))

/** app.css :root[data-theme="light"]; the accent turns charcoal so it reads on white. */
val EncorePaletteLight: EncorePalette = KitPalettes.Light

/** The web's data-theme="dark": the original Encore dark, blue-black, with the logo's cream accent. */
val EncorePaletteNight: EncorePalette =
    KitPalettes.Dark.copy(accent = Color(0xFFF0EAD6), accent2 = Color(0xFFD6C49A), onAccent = Color(0xFF16161F))

/** Encore's palette for each theme, for the kit's palette maths ([kitPaletteOf]). */
val EncorePalettes: KitPaletteSet = KitPaletteSet(encore = EncorePaletteDark, dark = EncorePaletteNight, oled = EncorePaletteOled, light = EncorePaletteLight)

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

/** Settings → Appearance: the kit's accents and presets under Encore's ids and names, and the kit's colour maths. */
object Appearance {
    /**
     * Encore's accent id (kept in the account's settings, shared with the web) for each kit accent id that differs:
     * the kit's Cream is Encore's Logo and its Pastel is Encore's Encore.
     */
    private val ENCORE_IDS: Map<String, Pair<String, String>> = mapOf("cream" to ("logo" to "Logo"), "pastel" to ("encore" to "Encore"))

    /** The kit accent id for one of Encore's ([ENCORE_IDS] the other way round); any other id is the same in both. */
    fun kitAccent(id: String): String = ENCORE_IDS.entries.firstOrNull { it.value.first == id }?.key ?: id

    /** The accents, in the kit's order, with Encore's ids and names. */
    val ACCENTS: List<Accent> = KitAppearance.ACCENTS.map {
        val (id, name) = ENCORE_IDS[it.id] ?: (it.id to it.name)
        Accent(id, name, it.a, it.b)
    }

    /** The presets, with Encore's accent ids. */
    val PRESETS: List<Preset> = KitAppearance.PRESETS.map {
        Preset(it.id, it.name, ThemeChoice.of(it.theme), ENCORE_IDS[it.accent]?.first ?: it.accent, it.custom, it.background)
    }

    /** The default custom accent colour. */
    const val DEFAULT_CUSTOM = KitAppearance.DEFAULT_CUSTOM

    /** "#abc" / "abc" / "#aabbcc" as "#aabbcc"; null when it is not a hex colour (the web's /^#([0-9a-f]{3}|[0-9a-f]{6})$/i). */
    fun cleanHex(input: String): String? = AppearanceRules.cleanHex(input)

    /** A "#rrggbb" colour as a Compose colour. */
    fun colorOf(hex: String): Color = AppearanceRules.colorOf(hex)

    /** A custom colour gets a partner shifted around the colour wheel for the gradient (appearance.rs partner). */
    fun partner(hex: String): String = AppearanceRules.partner(hex)

    /** The accent's two colours as hex (a preset's, by Encore's id, or the custom colour and its partner). */
    fun accentHex(accent: String, custom: String): Pair<String, String> = AppearanceRules.accentHex(kitAccent(accent), custom)

    /** Dark text on light accents, white on dark ones (appearance.rs apply: `--on-accent`). */
    fun onAccent(a: String, b: String): Color = AppearanceRules.onAccent(a, b)
}

/**
 * The palette for the account's look: the theme's tokens, the accent's colours (the logo's cream and gold keep the
 * theme's own accent, charcoal on Light) and, for the faded background, a faint wash of the accent in the page colours.
 */
fun paletteOf(settings: Settings, systemDark: Boolean): EncorePalette {
    // "logo" is the theme's own accent (cream and gold, charcoal on Light), which the kit calls ""
    val accent = if (settings.accent == "logo") "" else Appearance.kitAccent(settings.accent)
    return kitPaletteOf(KitLook(KitTheme.of(settings.theme.id), accent, settings.custom, settings.background), systemDark, EncorePalettes)
}

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
