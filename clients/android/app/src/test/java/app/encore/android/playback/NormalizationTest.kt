package app.encore.android.playback

import app.encore.android.data.api.TrackGain
import app.encore.android.data.prefs.NormPrefs
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NormalizationTest {
    private val eps = 1e-9

    @Test
    fun formulaIsTheWebs() {
        // applied dB = rg_gain + (target − (−18))
        assertEquals(-4.0, Normalization.appliedDb(-8.0, -14), eps)
        assertEquals(-8.0, Normalization.appliedDb(-8.0, -18), eps)
        assertEquals(-13.0, Normalization.appliedDb(-8.0, -23), eps)
        assertEquals(1.0, Normalization.linear(0.0), eps)
        assertEquals(0.5011872336, Normalization.linear(-6.0), 1e-9)
    }

    @Test
    fun offAndNoData() {
        val g = TrackGain(-8.0, -7.0, 0.9, 1.0)
        val off = Normalization.decide(NormPrefs(mode = "off"), g)
        assertFalse(off.on)
        assertEquals(1f, off.linear)
        assertEquals("", off.line())
        val none = Normalization.decide(NormPrefs(mode = "track"), null)
        assertTrue(none.noData)
        assertEquals(1f, none.linear)
        assertEquals("No loudness data for this song", none.line())
    }

    @Test
    fun trackAndAlbumWithFallbacks() {
        val g = TrackGain(-8.0, -6.5, 0.5, 0.6)
        val t = Normalization.decide(NormPrefs(mode = "track", target = -14), g)
        assertEquals("track", t.basis)
        assertEquals(-4.0, t.db, eps)
        val a = Normalization.decide(NormPrefs(mode = "album", target = -14), g)
        assertEquals("album", a.basis)
        assertEquals(-2.5, a.db, eps)
        assertEquals("−2.5 dB (album gain, target −14 LUFS)", a.line())
        // album mode without an album gain uses the track gain, and the other way round
        assertEquals("track", Normalization.decide(NormPrefs(mode = "album"), TrackGain(-8.0, null, null, null)).basis)
        assertEquals("album", Normalization.decide(NormPrefs(mode = "track"), TrackGain(null, -7.0, null, null)).basis)
    }

    @Test
    fun preventClipping() {
        // a quiet song (+7 dB at −14) whose peak is at half scale may rise only 6.02 dB
        val g = TrackGain(3.0, null, 0.5, null)
        val on = Normalization.decide(NormPrefs(mode = "track", target = -14, clip = true), g)
        assertTrue(on.limited)
        assertEquals(6.0206, on.db, 1e-4)
        assertEquals(7.0, on.requestedDb, eps)
        assertFalse(Normalization.decide(NormPrefs(mode = "track", target = -14, clip = true), TrackGain(1.0, null, 0.5, null)).limited)
        // no peak known: no boost above 0 dB; a cut is never limited
        val unknown = Normalization.decide(NormPrefs(mode = "track", target = -14, clip = true), TrackGain(2.0, null, null, null))
        assertEquals(0.0, unknown.db, eps)
        assertEquals(-4.0, Normalization.decide(NormPrefs(mode = "track", clip = true), TrackGain(-8.0, null, null, null)).db, eps)
        // clipping allowed: the full boost, within the +15 dB limit
        assertEquals(7.0, Normalization.decide(NormPrefs(mode = "track", target = -14, clip = false), g).db, eps)
        assertEquals(15.0, Normalization.decide(NormPrefs(mode = "track", target = -5, clip = false), TrackGain(20.0, null, null, null)).db, eps)
    }

    @Test
    fun prefsAndTexts() {
        assertEquals(NormPrefs("off", -23, false), NormPrefs("loud", -40, false).clean())
        assertEquals(-5, NormPrefs("album", 3, true).clean().target)
        assertEquals("+4.0 dB", Normalization.dbText(4.0))
        assertEquals("−3.2 dB", Normalization.dbText(-3.24))
        assertEquals("0.0 dB", Normalization.dbText(-0.01))
        assertEquals("−14 LUFS", Normalization.lufsText(-14))
    }

    @Test
    fun rowShapeRoundTrips() {
        val o = Json.parseToJsonElement("""{"track": -7.21, "album": null, "track_peak": 0.988}""") as JsonObject
        val g = TrackGain.from(o)!!
        assertEquals(-7.21, g.track!!, eps)
        assertNull(g.album)
        assertNull(g.albumPeak)
        assertEquals(g, TrackGain.fromArray(g.toArray()))
        assertEquals(g, TrackGain.from(g.toJson()))
        assertNull(TrackGain.from(Json.parseToJsonElement("{}")))
        assertNull(TrackGain.from(null))
    }
}
