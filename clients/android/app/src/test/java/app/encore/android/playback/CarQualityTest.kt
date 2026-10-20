package app.encore.android.playback

import app.encore.android.data.api.ArtistSongs
import app.encore.android.data.api.LibrarySearch
import app.encore.android.data.api.SearchCounts
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The car's quality badge (CarQuality) and the 0.31 response shapes the phone reads
 * (docs/contracts/client-shapes-0.31-0.33.md, §2.1 and §5.4).
 */
class CarQualityTest {
    private fun q(format: String, bitrate: Int = 0, rate: Int = 0, bits: Int = 0, kind: String = "", phoneRate: Int = 0, local: String = "") =
        CarQuality.of(format, bitrate, rate, bits, kind, phoneRate, local)

    @Test
    fun losslessAndHiResFromTheFile() {
        assertEquals(CarQuality(CarQuality.Kind.LOSSLESS, "FLAC 16/44.1"), q("flac", 900, 44100, 16, "original"))
        assertEquals(CarQuality(CarQuality.Kind.HIRES, "FLAC 24/96"), q("flac", 2800, 96000, 24, "original"))
        // 24-bit at 48 kHz is hi-res, 16-bit at 48 kHz is not
        assertEquals(CarQuality.Kind.HIRES, q("flac", 0, 48000, 24)?.kind)
        assertEquals(CarQuality.Kind.LOSSLESS, q("wav", 0, 48000, 16)?.kind)
        assertEquals(CarQuality(CarQuality.Kind.HIRES, "DSF DSD64"), q("dsf", 5644, 2822400, 1))
        // converted to FLAC by the server: still the file's own quality
        assertEquals(CarQuality(CarQuality.Kind.LOSSLESS, "APE 16/44.1"), q("ape", 700, 44100, 16, "flac"))
    }

    @Test
    fun m4aIsAlacWithABitDepthElseAac() {
        assertEquals(CarQuality(CarQuality.Kind.LOSSLESS, "ALAC 16/44.1"), q("m4a", 800, 44100, 16))
        assertEquals(CarQuality(CarQuality.Kind.LOSSY, "M4A 256k"), q("m4a", 256, 44100, 0))
        assertEquals(CarQuality(CarQuality.Kind.LOSSY, "MP3 320k"), q("mp3", 320, 44100))
        assertEquals(CarQuality(CarQuality.Kind.LOSSY, "OPUS"), q("opus"))
    }

    @Test
    fun theStreamWinsOverTheFile() {
        assertEquals(CarQuality(CarQuality.Kind.LOSSY, "AAC 96k"), q("flac", 2800, 96000, 24, "aac-96"))
        assertEquals(CarQuality(CarQuality.Kind.CADENZA, "Cadenza 24/192"), q("flac", 900, 44100, 16, "cadenza-192000"))
        assertEquals(CarQuality(CarQuality.Kind.CADENZA, "Cadenza 88.2 kHz"), q("flac", 900, 44100, 16, "original", phoneRate = 88200))
        assertEquals(CarQuality(CarQuality.Kind.HIRES, "FLAC 24/96"), q("flac", kind = "local", local = "FLAC 24/96"))
        assertEquals(CarQuality(CarQuality.Kind.LOSSY, "AAC 256k"), q("flac", kind = "local", local = "AAC 256k"))
        assertEquals(CarQuality(CarQuality.Kind.LOSSLESS, "FLAC (converted)"), q("dsf", kind = "local", local = "FLAC (converted)"))
    }

    @Test
    fun olderServersGiveTheFormatAlone() {
        assertEquals(CarQuality(CarQuality.Kind.LOSSLESS, "FLAC"), q("flac", 1000))
        assertNull(q(""))
        assertNull(q("xyz"))
        assertEquals("Hi-res: FLAC 24/96", q("flac", 0, 96000, 24)?.name)
    }

    @Test
    fun artistSongsAndSearchCountsRead() {
        val json = Json { ignoreUnknownKeys = true }
        val page = ArtistSongs.from(json.parseToJsonElement("""{"total": 143, "offset": 0, "tracks": [{"id": 5, "title": "A", "sample_rate": 96000, "bit_depth": 24}]}"""))
        assertEquals(143, page.total)
        assertEquals(96000, page.tracks.single().sampleRate)
        assertEquals(24, page.tracks.single().bitDepth)
        // a bare array, and nothing
        assertEquals(2, ArtistSongs.from(json.parseToJsonElement("""[{"id": 1}, {"id": 2}]""")).total)
        assertEquals(0, ArtistSongs.from(null).total)
        val s = LibrarySearch.from(json.parseToJsonElement("""{"songs": [], "albums": [], "artists": [], "counts": {"artists": 4, "albums": 17, "songs": 230}}""") as JsonObject)
        assertEquals(SearchCounts(4, 17, 230), s.counts)
        assertNull(LibrarySearch.from(JsonObject(emptyMap())).counts)
    }
}
