package app.encore.android.playback

import app.encore.android.playback.LyricsStripRules.Btn.ARTIST
import app.encore.android.playback.LyricsStripRules.Btn.LIKE
import app.encore.android.playback.LyricsStripRules.Btn.NEXT
import app.encore.android.playback.LyricsStripRules.Btn.PLAY
import app.encore.android.playback.LyricsStripRules.Btn.PREVIOUS
import app.encore.android.playback.LyricsStripRules.Btn.REPEAT
import app.encore.android.playback.LyricsStripRules.Btn.SHUFFLE
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Car buttons → Encore Lyrics screen (LyricsStripRules): the Encore Lyrics car app's control strip. */
class LyricsStripRulesTest {
    private val r = LyricsStripRules

    @Test fun defaultIsWhatTheAppHadBefore() {
        assertEquals(listOf(PREVIOUS, PLAY, NEXT, ARTIST), r.DEFAULT)
        assertEquals(r.DEFAULT, r.decode(null))
    }

    @Test fun encodeDecodeRoundTrip() {
        val s = listOf(PLAY, LIKE, SHUFFLE)
        assertEquals("v1|play,like,shuffle", r.encode(s))
        assertEquals(s, r.decode(r.encode(s)))
    }

    @Test fun emptyStripIsKept() {
        assertEquals("v1|", r.encode(emptyList()))
        assertEquals(emptyList<LyricsStripRules.Btn>(), r.decode("v1|"))
    }

    @Test fun badValuesFallBackOrAreCleaned() {
        assertEquals(r.DEFAULT, r.decode(""))
        assertEquals(r.DEFAULT, r.decode("v9|play"))
        assertEquals(r.DEFAULT, r.decode("play,next"))
        assertEquals(listOf(PLAY, NEXT), r.decode("v1|play, nope ,next,play"))
        assertEquals(listOf(PREVIOUS, PLAY, NEXT, LIKE), r.decode("v1|previous,play,next,like,shuffle"))
    }

    @Test fun toggleAddsToTheEndAndRemoves() {
        assertEquals(listOf(PREVIOUS, PLAY, NEXT), r.toggle(r.DEFAULT, ARTIST))
        assertEquals(listOf(PREVIOUS, PLAY, NEXT, LIKE), r.toggle(listOf(PREVIOUS, PLAY, NEXT), LIKE))
    }

    @Test fun fullStripTakesNoMore() {
        assertEquals(r.DEFAULT, r.toggle(r.DEFAULT, SHUFFLE))
        assertFalse(r.canAdd(r.DEFAULT, SHUFFLE))
        assertTrue(r.canAdd(r.DEFAULT, PLAY))
        assertTrue(r.canAdd(listOf(PLAY), SHUFFLE))
    }

    @Test fun upAndDownMoveOnePlace() {
        assertEquals(listOf(PLAY, PREVIOUS, NEXT, ARTIST), r.up(r.DEFAULT, PLAY))
        assertEquals(r.DEFAULT, r.up(r.DEFAULT, PREVIOUS))
        assertEquals(listOf(PREVIOUS, PLAY, ARTIST, NEXT), r.down(r.DEFAULT, NEXT))
        assertEquals(r.DEFAULT, r.down(r.DEFAULT, ARTIST))
        assertEquals(r.DEFAULT, r.up(r.DEFAULT, REPEAT))
    }

    @Test fun rowsListTheStripFirstThenTheRest() {
        assertEquals(listOf(LIKE, PLAY, PREVIOUS, NEXT, ARTIST, SHUFFLE, REPEAT), r.rows(listOf(LIKE, PLAY)))
        assertEquals(7, r.rows(emptyList()).size)
    }

    @Test fun lines() {
        assertEquals("Previous · Play / Pause · Next · More by the artist", r.summary(r.DEFAULT))
        assertEquals("No buttons", r.summary(emptyList()))
        assertEquals("All 4 spots used. Turn one off to add another.", r.roomLine(r.DEFAULT))
        assertEquals("1 spot left", r.roomLine(listOf(PLAY, NEXT, LIKE)))
        assertEquals("3 spots left", r.roomLine(listOf(PLAY)))
    }
}
