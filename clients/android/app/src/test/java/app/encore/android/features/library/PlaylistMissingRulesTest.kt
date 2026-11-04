package app.encore.android.features.library

import app.encore.android.data.api.PlaylistRow
import org.junit.Assert.assertEquals
import org.junit.Test

/** The rows that can't be requested, and the list they are copied as (the web's `cant_request_list`). */
class PlaylistMissingRulesTest {
    private val why = "Track needs a MusicBrainz id or artist + title"

    @Test
    fun onlyMarkedMissingRowsAreListed() {
        val rows = listOf(
            PlaylistRow("Lonely", "", null, cantRequest = why),
            PlaylistRow("Song", "Band", null, album = "LP", cantRequest = why),
            PlaylistRow("Animal", "Neon Trees", null),
            PlaylistRow("", "Only artist", null, cantRequest = why),
        )
        val cant = PlaylistMissingRules.cantRequest(rows)
        assertEquals(3, cant.size)
        assertEquals("Lonely\nBand - Song (LP)\nOnly artist", PlaylistMissingRules.listText(cant))
        assertEquals("324 can't be requested · Copy list", PlaylistMissingRules.buttonText(324))
    }
}
