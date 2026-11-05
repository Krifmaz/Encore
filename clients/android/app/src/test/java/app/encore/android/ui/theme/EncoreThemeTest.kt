package app.encore.android.ui.theme

import androidx.compose.ui.graphics.Color
import app.encore.android.data.session.Settings
import app.encore.android.data.session.ThemeChoice
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test

/** Encore's look on the kit's theme: its own colours where they differ, and its own accent ids and names. */
class EncoreThemeTest {
    @Test
    fun theThemesKeepEncoresColours() {
        // the default look is the Encore theme with the logo's own accent, as before the kit
        assertSame(EncorePaletteDark, paletteOf(Settings(), systemDark = true))
        assertEquals(Color(0xFF161617), EncorePaletteDark.onAccent)
        // Dark keeps the cream accent, OLED its cream text, Light its charcoal accent
        val night = paletteOf(Settings(theme = ThemeChoice.DARK), systemDark = true)
        assertEquals(Color(0xFF0B0B12), night.bg)
        assertEquals(Color(0xFFF0EAD6), night.accent)
        assertEquals(Color(0xFFF0EAD6), paletteOf(Settings(theme = ThemeChoice.OLED), systemDark = true).text)
        assertEquals(Color(0xFF2A2A2D), paletteOf(Settings(theme = ThemeChoice.LIGHT), systemDark = true).accent)
        // Match device follows the phone into Encore's Dark
        assertSame(EncorePaletteNight, paletteOf(Settings(theme = ThemeChoice.SYSTEM), systemDark = true))
    }

    @Test
    fun accentsKeepEncoresIdsAndNames() {
        assertEquals("logo" to "Logo", Appearance.ACCENTS[0].let { it.id to it.name })
        assertEquals("encore" to "Encore", Appearance.ACCENTS[2].let { it.id to it.name })
        assertEquals("logo", Appearance.PRESETS.first { it.id == "encore" }.accent)
        assertEquals("encore", Appearance.PRESETS.first { it.id == "pastel" }.accent)
        assertEquals("#f2a7e4" to "#9fd2fb", Appearance.accentHex("encore", "#000000"))
        // a chosen accent paints the theme: Encore's "encore" is the kit's pastel
        assertEquals(Color(0xFFF2A7E4), paletteOf(Settings(accent = "encore"), systemDark = true).accent)
    }
}
