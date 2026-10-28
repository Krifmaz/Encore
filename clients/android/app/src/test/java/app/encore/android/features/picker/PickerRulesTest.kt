package app.encore.android.features.picker

import app.encore.android.core.EncoreJson
import app.encore.android.data.repo.FeaturesApi
import app.encore.android.data.repo.PickSearch
import app.encore.android.data.repo.PickerOpts
import app.encore.android.data.repo.PickerSource
import app.encore.android.data.repo.pickSearchOf
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
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
    fun theServersFinalAnswerReads() {
        // package B's answer as the server sends it (R2): cached_at in whole seconds, found on the server's clock
        val kept = pickSearchOf(EncoreJson.parseToJsonElement(
            """{"items": [{"source": "slskd", "key": "alice|Music/Duumu/Illuminate EP", "title": "Duumu/Illuminate EP", "subtitle": "Music / Duumu / Illuminate EP",
                 "quality": "FLAC 16-bit", "lossless": true, "tracks": 2, "size": 60000000, "duration": null, "info": "alice · free slot · 2.0 MB/s", "score": 93.0,
                 "payload": {"username": "alice", "directory": "Music/Duumu/Illuminate EP", "format": "flac", "bitrate": 0, "files": []}}],
               "from": "cache", "cached_at": 1760875320, "found": "Found at 14:02", "query": "Illuminate EP", "can_search": true, "notice": null}""",
        ))
        assertEquals(1, kept.items.size)
        assertEquals("FLAC 16-bit", kept.items[0].quality)
        assertTrue(kept.kept)
        assertEquals(1760875320.0, kept.cachedAt, 0.001)
        assertEquals(FoundLine("Found at 14:02 for \"Illuminate EP\"", again = true), foundLineOf(kept))
        // B's final words: a kept search the box's words make comes back with the box's own words (brackets and
        // symbols included), so nothing follows "Found at"; only the request's other search ("Days Before Rodeo",
        // the album-only fallback) names its words
        val box = "Machine Time Moonlight Shadow (nightcore)"
        val own = kept.copy(query = box)
        assertEquals(FoundLine("Found at 14:02", again = true), foundLineOf(own, typed = box))
        assertEquals(FoundLine("Found at 14:02", again = true), foundLineOf(kept.copy(query = "Ke\$ha Tik Tok"), typed = "Ke\$ha Tik Tok"))
        assertEquals(FoundLine("Found at 14:02 for \"Days Before Rodeo\"", again = true), foundLineOf(kept.copy(query = "Days Before Rodeo"), typed = "Travis Scott Days Before Rodeo"))
        // kept rows while Soulseek isn't connected: no Search again, the server's reason instead
        val down = pickSearchOf(EncoreJson.parseToJsonElement(
            """{"items": [], "from": "cache", "cached_at": 1760875320, "found": "Found yesterday at 22:10", "query": "Duumu Illuminate EP",
               "can_search": false, "notice": "Soulseek isn't connected – next attempt at 14:20"}""",
        ))
        assertEquals(FoundLine("No results. · Found yesterday at 22:10 · Soulseek isn't connected – next attempt at 14:20", again = false), foundLineOf(down, empty = true))
        // a live answer of every other source: nothing to say
        val live = pickSearchOf(EncoreJson.parseToJsonElement("""{"items": [], "from": "live", "cached_at": null, "found": null, "query": "Duumu Illuminate EP", "can_search": true, "notice": null}"""))
        assertFalse(live.kept)
        assertNull(foundLineOf(live))
    }

    @Test
    fun theAudioCheckSwitch() {
        assertEquals("Don't check the audio", PickerRules.NO_CHECK)
        assertEquals("Imports what you pick even when the audio check says it is another song", PickerRules.NO_CHECK_HELP)
        assertTrue(PickerRules.note(false).startsWith("The file you pick is still fingerprint-checked"))
        assertTrue(PickerRules.note(true).startsWith("The audio check is off: the file you pick is imported even when the check says it is another song."))
        // remembered on this phone as the web keeps it ("1" / "0"); off by default
        assertTrue(PickerRules.noCheckOf("1"))
        assertFalse(PickerRules.noCheckOf("0"))
        assertFalse(PickerRules.noCheckOf(null))
        assertFalse(PickerRules.noCheckOf("true"))
        assertEquals("1", PickerRules.noCheckValue(true))
        assertEquals("0", PickerRules.noCheckValue(false))
        // only for those who may Keep anyway: a switch kept on by someone without Fix their own downloads isn't sent
        assertTrue(PickerRules.noCheckSent(noCheck = true, mayFix = true))
        assertFalse(PickerRules.noCheckSent(noCheck = true, mayFix = false))
        assertFalse(PickerRules.noCheckSent(noCheck = false, mayFix = true))
        // the pick sends no_check only with the switch on
        val payload = buildJsonObject { put("url", "https://music.youtube.com/watch?v=x") }
        val forRequest = PickerOpts(kind = "track", artist = "Machine Time", title = "Moonlight Shadow (nightcore)", requestId = 21)
        val on = FeaturesApi.pickBody("ytmusic", payload, "Moonlight Shadow", forRequest, noCheck = true)
        assertEquals(JsonPrimitive(true), on["no_check"])
        assertEquals(JsonPrimitive(21L), on["request_id"])
        assertNull(on["request"])
        val off = FeaturesApi.pickBody("ytmusic", payload, "Moonlight Shadow", forRequest)
        assertNull(off["no_check"])
        // a pick with no request yet asks for one
        val fresh = FeaturesApi.pickBody("deezer", payload, "Moonlight Shadow", PickerOpts(kind = "track", artist = "Machine Time", title = "Moonlight Shadow"), noCheck = true)
        assertEquals(JsonPrimitive(true), fresh["no_check"])
        assertEquals("track", (fresh["request"] as JsonObject)["kind"]?.jsonPrimitive?.content)
    }

    @Test
    fun emptyAnswersSayWhy() {
        val why = "Prowlarr's indexers found nothing for \"x\""
        // a live empty answer: its reason, in place of a bare "No results."
        assertEquals(why, PickerRules.emptyNote(PickSearch(notice = why)))
        assertEquals("No results – $why", PickerRules.noneText(why, kept = false))
        assertEquals(why, PickerRules.noneText(why, kept = true))
        assertEquals("No results.", PickerRules.noneText(null, kept = false))
        // none without a reason, nor where the kept line already shows the notice
        assertNull(PickerRules.emptyNote(PickSearch()))
        assertNull(PickerRules.emptyNote(PickSearch(from = "cache", canSearchFlag = false, notice = why)))
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
