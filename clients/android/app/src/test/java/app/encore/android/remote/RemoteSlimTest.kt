package app.encore.android.remote

import app.encore.android.data.api.Track
import app.encore.android.data.api.TrackGain
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

/** A queue entry sent to another device keeps the song's loudness data and file facts (client-shapes §5.1). */
class RemoteSlimTest {
    private val song = Track(
        id = 7, title = "Crazy", artist = "Gnarls Barkley", album = "St. Elsewhere", albumArtist = "Gnarls Barkley",
        albumId = "a1", trackNo = 1, disc = 1, year = "2006", durationSec = 178.0, format = "flac", bitrate = 0,
        gain = TrackGain(track = -7.2, album = -6.5, trackPeak = 0.98, albumPeak = 1.0), sampleRate = 96000, bitDepth = 24,
    )

    @Test
    fun aQueueEntryKeepsItsLoudnessAndFacts() {
        val back = Track.from(RemoteDevice.slim(song))!!
        assertEquals(song.gain, back.gain)
        assertEquals(96000, back.sampleRate)
        assertEquals(24, back.bitDepth)
        assertEquals("flac", back.format)
    }

    @Test
    fun aSongWithNothingKnownCarriesNoneOfThem() {
        val o = RemoteDevice.slim(song.copy(gain = null, sampleRate = 0, bitDepth = 0))
        assertFalse(o.containsKey("gain"))
        assertFalse(o.containsKey("sample_rate"))
        assertFalse(o.containsKey("bit_depth"))
    }
}
