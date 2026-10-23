package app.encore.android.ui.screens

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** When the full player turns sideways and how big its sideways cover is (PlayerLayout.kt). */
class PlayerLayoutTest {
    @Test
    fun sidewaysOnlyWhenWiderThanTall() {
        // a phone on its side, upright, and a square window
        assertTrue(PlayerLayout.isWide(915, 412))
        assertFalse(PlayerLayout.isWide(412, 915))
        assertFalse(PlayerLayout.isWide(600, 600))
    }

    @Test
    fun coverTakesTheHeightTheControlsLeave() {
        // a phone on its side: ~450 dp wide half, ~330 dp tall under the top bar
        assertEquals(330f - PlayerLayout.UNDER_ART, PlayerLayout.landscapeArt(450f, 330f), 0.01f)
    }

    @Test
    fun coverStaysBetweenItsLimits() {
        // a very short window keeps a readable cover (the half scrolls), a tablet doesn't become all cover
        assertEquals(PlayerLayout.MIN_ART, PlayerLayout.landscapeArt(450f, 180f), 0.01f)
        assertEquals(PlayerLayout.MAX_ART, PlayerLayout.landscapeArt(800f, 900f), 0.01f)
        // a narrow half leaves the song's lines room beside the cover
        assertEquals(200f * 0.45f, PlayerLayout.landscapeArt(200f, 600f), 0.01f)
    }
}
