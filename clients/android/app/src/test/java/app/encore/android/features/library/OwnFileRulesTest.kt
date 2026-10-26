package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import app.encore.android.data.repo.Me
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Bring your own music's rules: who may, the words of what changed, the Signal path note and the batch's choices. */
class OwnFileRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json) as JsonObject

    @Test
    fun whoMayReplaceASongsFile() {
        assertTrue(OwnFileRules.allowed(Me(role = "admin")))
        assertTrue(OwnFileRules.allowed(Me(role = "user", perms = mapOf("own_files" to true))))
        assertFalse(OwnFileRules.allowed(Me(role = "user", perms = mapOf("own_files" to false))))
        assertFalse(OwnFileRules.allowed(Me(role = "user")))
        assertFalse(OwnFileRules.allowed(null))
    }

    @Test
    fun whatChangedInWords() {
        val v = obj("""{"before": {"format": "mp3", "bitrate": 320}, "after": {"format": "flac", "bit_depth": 24, "sample_rate": 44100},
            "warnings": ["Your file is 12s longer than the one it replaced – check it is the same version"]}""")
        assertEquals("MP3 · 320 kbps → FLAC · 24-bit / 44.1 kHz. Your file is 12s longer than the one it replaced – check it is the same version", OwnFileRules.resultText(v))
        assertEquals("FLAC · 16-bit / 48 kHz", OwnFileRules.qualityText(obj("""{"format": "flac", "bit_depth": 16, "sample_rate": 48000}""")))
    }

    @Test
    fun theSignalPathSaysWhoseFileItIs() {
        assertEquals("", OwnFileRules.note(null) { "" })
        val note = OwnFileRules.note(obj("""{"by": "alex", "at": 1790000000, "old_format": "mp3"}""")) { "24 Oct 2026" }
        assertEquals("Your own file – uploaded by alex on 24 Oct 2026, replacing an MP3", note)
        assertEquals("a FLAC", OwnFileRules.article("FLAC"))
        assertEquals("a WAV", OwnFileRules.article("WAV"))
    }

    @Test
    fun aSureMatchIsChosenAndTheRestLeftOut() {
        val sure = obj("""{"token": "t1", "match": {"track": {"id": 7}}, "candidates": [{"track": {"id": 7, "artist": "A", "title": "B", "album": "C"}}]}""")
        assertEquals(7L, OwnFileRules.defaultPick(sure))
        assertEquals(listOf(OwnFileRules.Choice(null, "Leave out"), OwnFileRules.Choice(7L, "A – B (C)")), OwnFileRules.choices(sure))
        assertNull(OwnFileRules.defaultPick(obj("""{"match": null, "candidates": []}""")))
        assertEquals("Replace 1 song", OwnFileRules.applyLabel(1))
        assertEquals("Replace 3 songs", OwnFileRules.applyLabel(3))
        val answer = obj("""{"replaced": [{"token": "t1", "artist": "A", "title": "B", "format": "flac", "warnings": []}], "failed": [{"token": "t2", "reason": "Song not found"}]}""")
        assertEquals("Replaced A – B – now FLAC" to true, OwnFileRules.outcome(answer, "t1"))
        assertEquals("Song not found" to false, OwnFileRules.outcome(answer, "t2"))
        assertNull(OwnFileRules.outcome(answer, "t3"))
    }
}
