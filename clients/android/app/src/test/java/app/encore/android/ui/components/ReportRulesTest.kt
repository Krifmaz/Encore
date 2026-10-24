package app.encore.android.ui.components

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What the Report a problem sheet offers and says (ReportRules). The owner (Oct 24): "why do i have to navigate
 * multiple menus to report a problem and i cant even report wrong lyrics" – the lyrics reasons keep the song.
 */
class ReportRulesTest {
    /** Songs get the two lyrics reasons (before Other); albums don't, lyrics are reported one song at a time. */
    @Test
    fun lyricsReasonsAreForSongsOnly() {
        val song = ReportRules.reasonsFor("track").map { it.first }
        assertTrue(ReportRules.WRONG_LYRICS in song && ReportRules.LYRICS_SYNC in song)
        assertEquals("other", song.last())
        val album = ReportRules.reasonsFor("album").map { it.first }
        assertFalse(album.any { ReportRules.isLyrics(it) })
        assertEquals(REPORT_REASONS.size - 2, album.size)
        assertFalse(ReportRules.isLyrics("wrong_song"))
    }

    /** A lyrics reason says the song stays; the others say the file goes to the trash. */
    @Test
    fun theExplanationFollowsTheReason() {
        val audio = ReportRules.explanation("track", "90210", "bad_audio")
        assertTrue(audio.contains("trash"))
        for (r in listOf(ReportRules.WRONG_LYRICS, ReportRules.LYRICS_SYNC)) {
            val words = ReportRules.explanation("track", "90210", r)
            assertTrue(words, words.contains("song itself stays") && !words.contains("trash") && words.contains("90210"))
        }
        assertTrue(ReportRules.explanation("album", "Rodeo", "").contains("files to the trash"))
    }

    /** The toast after sending names what the server did. */
    @Test
    fun doneToasts() {
        assertEquals("Reported – looking for a better copy", ReportRules.doneToast("replacing", ""))
        assertEquals("Reported – found other lyrics", ReportRules.doneToast("lyrics_found", "x"))
        assertTrue(ReportRules.doneToast("lyrics_none", "").contains("no other lyrics"))
        assertEquals("Replaced", ReportRules.doneToast("replaced", "Replaced"))
        assertEquals("Reported", ReportRules.doneToast("", ""))
    }
}
