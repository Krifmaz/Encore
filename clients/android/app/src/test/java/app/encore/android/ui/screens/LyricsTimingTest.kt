package app.encore.android.ui.screens

import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Test

/** The lyrics view's timing nudge (LyricsTiming) and the shifted lines Lyrics hands the player. */
class LyricsTimingTest {
    private val t = LyricsTiming

    /** A tap moves a quarter second; the nudge stays within ±30 s. */
    @Test fun stepsAndLimits() {
        assertEquals(250L, t.step(0L, later = true))
        assertEquals(-250L, t.step(0L, later = false))
        assertEquals(t.MAX_MS, t.step(t.MAX_MS, later = true))
        assertEquals(-t.MAX_MS, t.clamp(-99_999L))
    }

    /** "In sync", or the signed seconds without trailing zeros. */
    @Test fun labels() {
        assertEquals("In sync", t.label(0L))
        assertEquals("+0.25 s", t.label(250L))
        assertEquals("−1.5 s", t.label(-1500L))
        assertEquals("+2 s", t.label(2000L))
    }

    /** The server's `shift_ms` moves every line later (negative: earlier); the stamps themselves stay in [Lyrics.raw]. */
    @Test fun shiftMovesTheLines() {
        val o = buildJsonObject {
            put("lyrics", "[00:01.00] one\n[00:02.50] two")
            put("synced", true)
            put("source", "file")
            put("shift_ms", JsonPrimitive(-500))
        }
        val l = Lyrics.from(o)
        assertEquals(listOf(LyricLine(0.5, "one"), LyricLine(2.0, "two")), l.lines)
        assertEquals(1.0, l.raw[0].timeSec, 0.0)
        assertEquals(listOf(1.25, 2.75), l.withShift(250L).lines.map { it.timeSec })
        // plain lyrics (and answers saved with older downloads) have no nudge
        assertEquals(0L, Lyrics.from(buildJsonObject { put("lyrics", "one"); put("synced", false); put("shift_ms", 900) }).shiftMs)
    }
}
