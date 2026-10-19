package app.encore.android.features.picker

import app.encore.android.core.EncoreJson
import app.encore.android.data.repo.PickSearch
import app.encore.android.data.repo.PickerSource
import app.encore.android.data.repo.pickSearchOf
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Calendar
import java.util.TimeZone

class PickerRulesTest {
    private val utc = TimeZone.getTimeZone("UTC")

    /** Unix seconds of a UTC time on Oct 19, 2026. */
    private fun at(day: Int, hour: Int, minute: Int): Double =
        Calendar.getInstance(utc).apply { clear(); set(2026, Calendar.OCTOBER, day, hour, minute, 0) }.timeInMillis / 1000.0

    @Test
    fun todaysBareArrayIsALiveAnswer() {
        val r = pickSearchOf(EncoreJson.parseToJsonElement("""[{"source": "deezer", "title": "Illuminate EP", "quality": "FLAC (Deezer HiFi)", "lossless": true, "tracks": 4, "payload": {"id": 1}}, {"source": "deezer"}]"""))
        assertEquals(1, r.items.size)   // a row without a title is left out
        assertEquals("Illuminate EP", r.items[0].title)
        assertFalse(r.kept)
        assertTrue(r.canSearch)
        assertNull(foundLineOf(r))
    }

    @Test
    fun theKeptAnswersFieldsAreRead() {
        val r = pickSearchOf(EncoreJson.parseToJsonElement(
            """{"items": [{"source": "slskd", "title": "Illuminate EP", "quality": "FLAC", "lossless": true, "tracks": 4, "payload": {}}],
               "from": "cache", "cached_at": ${at(19, 14, 2)}, "found": "Found at 14:02", "query": "Duumu Illuminate EP",
               "can_search": true, "notice": null}""",
        ))
        assertEquals(1, r.items.size)
        assertTrue(r.kept)
        assertEquals(at(19, 14, 2), r.cachedAt, 0.001)
        assertEquals("Found at 14:02", r.found)
        assertEquals("Duumu Illuminate EP", r.query)
        assertTrue(r.canSearch)
        assertEquals("", r.notice)
        // a live object answer, and one with every extra field missing
        assertFalse(pickSearchOf(EncoreJson.parseToJsonElement("""{"items": [], "from": "live", "cached_at": null}""")).kept)
        val bare = pickSearchOf(EncoreJson.parseToJsonElement("""{"items": []}"""))
        assertFalse(bare.kept)
        assertTrue(bare.canSearch)
        // can_search false, as 0 or false
        assertFalse(pickSearchOf(EncoreJson.parseToJsonElement("""{"from": "cache", "can_search": false}""")).canSearch)
        assertFalse(pickSearchOf(EncoreJson.parseToJsonElement("""{"from": "cache", "can_search": 0}""")).canSearch)
        // anything else is an empty answer
        assertEquals(0, pickSearchOf(null).items.size)
        assertEquals(0, pickSearchOf(EncoreJson.parseToJsonElement("\"oops\"")).items.size)
    }

    private fun foundLineOf(f: PickSearch?, typed: String = "Duumu Illuminate EP", empty: Boolean = false) =
        PickerRules.foundLine(f, typed, empty, at(19, 15, 0), utc)

    @Test
    fun keptLinesUseTheDesignsWords() {
        val kept = PickSearch(from = "cache", cachedAt = at(19, 14, 2), found = "Found at 14:02", query = "Duumu Illuminate EP")
        assertEquals(FoundLine("Found at 14:02", again = true), foundLineOf(kept))
        // a kept answer that found nothing
        assertEquals(FoundLine("No results. · Found at 14:02", again = true), foundLineOf(kept, empty = true))
        // searched with other words than the search box has (the worker's album-only search)
        assertEquals(FoundLine("Found at 14:02 for \"Illuminate EP\"", again = true), foundLineOf(kept.copy(query = "Illuminate EP")))
        // the same words in another order or case are the same search
        assertEquals(FoundLine("Found at 14:02", again = true), foundLineOf(kept, typed = "illuminate ep, DUUMU"))
        // Soulseek not connected: no Search again, the reason instead
        val down = kept.copy(canSearchFlag = false, notice = "Soulseek isn't connected – next attempt at 14:20")
        assertEquals(FoundLine("Found at 14:02 · Soulseek isn't connected – next attempt at 14:20", again = false), foundLineOf(down))
        assertEquals(FoundLine("No results. · Found at 14:02 · Soulseek isn't connected – next attempt at 14:20", again = false), foundLineOf(down, empty = true))
        // a live answer says nothing
        assertNull(foundLineOf(kept.copy(from = "live")))
        assertNull(foundLineOf(null))
    }

    @Test
    fun foundTimeIsMadeHereWhenTheServerSendsNone() {
        val today = PickSearch(from = "cache", cachedAt = at(19, 14, 2))
        assertEquals("Found at 14:02", PickerRules.foundText(today, at(19, 15, 0), utc))
        assertEquals("Found yesterday at 22:10", PickerRules.foundText(PickSearch(from = "cache", cachedAt = at(18, 22, 10)), at(19, 9, 0), utc))
        assertEquals("Found at 16/10 08:05", PickerRules.foundText(PickSearch(from = "cache", cachedAt = at(16, 8, 5)), at(19, 9, 0), utc))
        assertEquals("", PickerRules.foundText(PickSearch(from = "cache"), at(19, 9, 0), utc))
        // the server's words win (its clock is the one of "next attempt at …")
        assertEquals("Found at 13:02", PickerRules.foundText(today.copy(found = "Found at 13:02"), at(19, 15, 0), utc))
    }

    @Test
    fun whichSourcesAreAsked() {
        assertTrue(PickerRules.searches(PickerSource(id = "deezer", state = "ready")))
        // Soulseek while not connected: a kept search can still answer
        assertTrue(PickerRules.searches(PickerSource(id = "slskd", state = "not_connected")))
        assertFalse(PickerRules.searches(PickerSource(id = "slskd", state = "off")))
        assertFalse(PickerRules.searches(PickerSource(id = "torznab", state = "not_connected")))
        assertFalse(PickerRules.searches(PickerSource(id = "qobuz", state = "sign_in")))
        assertEquals("sign in", PickerRules.stateWord("sign_in"))
        assertEquals("not connected", PickerRules.stateWord("not_connected"))
        assertEquals("", PickerRules.stateWord("ready"))
    }

    @Test
    fun searchingWords() {
        assertEquals("Searching Soulseek… (Soulseek takes a few seconds)", PickerRules.searchingText("slskd", "Soulseek"))
        assertEquals("Searching Deezer…", PickerRules.searchingText("deezer", "Deezer"))
        assertEquals("Searching Soulseek again…", PickerRules.searchingAgainText("Soulseek"))
        assertEquals("Waiting for another Soulseek search, then searching…", PickerRules.WAITING)
        // Copy takes the source's name and its whole error (picker.rs)
        assertEquals("Tidal: Tidal API 400 INVALID_RESOURCE_ID, pointer data/id", PickerRules.errorCopy("Tidal", "Tidal API 400 INVALID_RESOURCE_ID, pointer data/id"))
    }
}
