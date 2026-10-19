package app.encore.android.features.offline

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class OfflineIndexTest {
    private fun rec(id: Long, album: String = "Album", albumId: String = "a1", track: Int = 1, artist: String = "Band", savedAt: Double = 100.0, title: String = "Song $id") =
        OfflineRecord(id = id, title = title, artist = artist, album = album, albumId = albumId, ext = "flac", track = track, savedAt = savedAt,
            sourceServer = "https://music.example.com", file = "Band/Album/0$track - $title.flac")

    @Test
    fun theSharedRecordReadsBack() {
        val r = OfflineRecord(
            id = 1234, title = "So What", artist = "Miles Davis", album = "Kind of Blue", albumId = "a1b2c3", ext = "flac",
            bytes = 61234567, quality = "FLAC 24/96", cover = true, lyrics = false, savedAt = 1792310400.0,
            sourceServer = "https://music.example.com", duration = 562.4, track = 1, disc = 1, format = "flac",
            file = "Miles Davis/Kind of Blue/01 - So What.flac", art = "covers/0a1b2c3d-a1b2c3.jpg",
        )
        val back = OfflineIndex.parse(OfflineIndex.write(listOf(r)))
        assertEquals(listOf(r), back)
        assertTrue(OfflineIndex.write(listOf(r)).startsWith("[{"))
    }

    @Test
    fun readersAreLenient() {
        assertEquals(1, OfflineIndex.parse("""{"songs":[{"id":1,"ext":"mp3"}]}""").size)
        assertEquals(1, OfflineIndex.parse("""{"tracks":[{"id":"7","ext":"mp3"}]}""").size)
        assertEquals(1, OfflineIndex.parse("""{"records":[{"id":7.0,"ext":"mp3"}]}""").size)
        assertEquals(emptyList<OfflineRecord>(), OfflineIndex.parse("not json"))
        assertEquals(emptyList<OfflineRecord>(), OfflineIndex.parse(null))
        // without a usable id a record is skipped
        assertEquals(0, OfflineIndex.parse("""[{"title":"x"},{"id":"x1"},{"id":0},{"id":-3},{"id":1.5},{"id":null}]""").size)
        // numbers written as strings read as numbers
        val r = OfflineIndex.parse("""[{"id":"12","bytes":"300","duration":"61.5","track":"3","album_id":42}]""").single()
        assertEquals(12L, r.id)
        assertEquals(300L, r.bytes)
        assertEquals(61.5, r.duration, 0.0)
        assertEquals(3, r.track)
        assertEquals("42", r.albumId)
        assertEquals("", OfflineIndex.parse("""[{"id":1,"album_id":null}]""").single().albumId)
    }

    @Test
    fun aRecordThatAppearsTwiceKeepsItsLastEntry() {
        val text = """[{"id":1,"title":"old","source_server":"https://a"},{"id":2,"source_server":"https://a"},
            {"id":1,"title":"new","source_server":"https://a"},{"id":1,"title":"other server","source_server":"https://b"}]"""
        val list = OfflineIndex.parse(text)
        assertEquals(3, list.size)
        assertEquals("new", list.first { it.id == 1L && it.sourceServer == "https://a" }.title)
    }

    @Test
    fun theClassicAppsShapeIsUpgraded() {
        val r = OfflineIndex.parse(
            """[{"id":"55","title":"T","artist":"A","album":"B","ext":"flac",
               "file":"https://asset.localhost/data/Music%20Box/55.flac","art":"asset://localhost/x/cover%2B1.jpg"}]""",
        ).single()
        assertEquals(55L, r.id)
        assertEquals("55.flac", r.file)
        assertEquals("cover+1.jpg", r.art)
        assertTrue(r.cover)
        assertFalse(r.lyrics)
        assertEquals("FLAC", r.quality)
        assertEquals("flac", r.format)
        assertEquals("", r.sourceServer)
        assertEquals(0.0, r.savedAt, 0.0)
        assertFalse(r.converted)
    }

    @Test
    fun desktop027sShapeKeepsItsQualityAndFraction() {
        val r = OfflineIndex.parse("""[{"id":9,"ext":"mp3","quality":"MP3 320","saved_at":1760000000.75,"source_server":"http://192.168.1.10:8080/"}]""").single()
        assertEquals("MP3 320", r.quality)
        assertEquals(1760000000.75, r.savedAt, 0.0)
        assertEquals("http://192.168.1.10:8080", r.sourceServer)
        // written as a whole number
        assertTrue(OfflineIndex.write(listOf(r)).contains("\"saved_at\":1760000000,"))
    }

    @Test
    fun theExtensionComesFromTheFileThenTheFormat() {
        assertEquals("ogg", OfflineIndex.parse("""[{"id":1,"file":"a/b/01 - x.ogg"}]""").single().ext)
        assertEquals("opus", OfflineIndex.parse("""[{"id":1,"format":"Opus"}]""").single().ext)
        val none = OfflineIndex.parse("""[{"id":1}]""").single()
        assertEquals("audio", none.ext)
        assertEquals("", none.quality)
        assertEquals("", none.format)
    }

    @Test
    fun convertedCopiesAreMarkedByTheirLabelOnly() {
        assertTrue(OfflineIndex.isConverted("FLAC (converted)"))
        assertTrue(OfflineIndex.isConverted("AAC 256k (converted)  "))
        assertFalse(OfflineIndex.isConverted("FLAC 16/44.1"))
        assertFalse(OfflineIndex.isConverted(""))
    }

    @Test
    fun theQualityLabelReadsLikeThePlayer() {
        assertEquals("FLAC 16/44.1", OfflineIndex.qualityLabel("flac", true, 16, 44100.0, 900.0))
        assertEquals("FLAC 24/96", OfflineIndex.qualityLabel("flac", false, 24, 96000.0, 0.0))
        assertEquals("ALAC 48 kHz", OfflineIndex.qualityLabel("alac", true, 0, 48000.0, 0.0))
        assertEquals("WAV 24-bit", OfflineIndex.qualityLabel("wav", true, 24, 0.0, 0.0))
        assertEquals("FLAC", OfflineIndex.qualityLabel("flac", true, 0, 0.0, 0.0))
        assertEquals("MP3 320k", OfflineIndex.qualityLabel("mp3", false, 0, 44100.0, 320.0))
        assertEquals("AAC 256k", OfflineIndex.qualityLabel("aac", false, 0, 44100.0, 255.6))
        assertEquals("Vorbis 192k", OfflineIndex.qualityLabel("ogg vorbis", false, 0, 44100.0, 192.0))
        assertEquals("Opus", OfflineIndex.qualityLabel("opus", false, 0, 48000.0, 0.0))
        assertEquals("44.1", OfflineIndex.khz(44100.0))
        assertEquals("96", OfflineIndex.khz(96000.0))
        assertEquals("88.2", OfflineIndex.khz(88200.0))
        assertEquals("vorbis", OfflineIndex.codecOf("", "ogg"))
        assertEquals("aac", OfflineIndex.codecOf("", "m4a"))
        assertEquals("alac", OfflineIndex.codecOf("ALAC", "m4a"))
        assertEquals("vorbis", OfflineIndex.codecOf("ogg vorbis", "ogg"))
    }

    @Test
    fun namesFromTheServerStayInsideTheirFolder() {
        assertEquals("AC-DC", OfflineIndex.safeName("AC/DC", "x"))
        assertEquals("a-b-c", OfflineIndex.safeName("a\\b:c", "x"))
        assertEquals("x", OfflineIndex.safeName("..", "x"))
        assertEquals("x", OfflineIndex.safeName(" ... ", "x"))
        assertEquals("x", OfflineIndex.safeName("", "x"))
        assertEquals("hidden", OfflineIndex.safeName(".hidden", "x"))
        assertEquals("What_", OfflineIndex.safeName("What?", "x"))
        assertEquals("a b", OfflineIndex.safeName("a\u0000\nb", "x"))
        assertEquals("Fin", OfflineIndex.safeName("Fin.", "x"))
        for (bad in listOf("../../etc", "..\\..\\x", "/abs/path", "a/../../b", "\u202Eevil")) {
            val n = OfflineIndex.safeName(bad, "x")
            assertFalse(n, n.contains('/') || n.contains('\\') || n.startsWith(".") || n == "..")
        }
        // long names are cut to 120 bytes without splitting a character
        val long = OfflineIndex.safeName("é".repeat(200) + "😀".repeat(10), "x")
        assertTrue(long.toByteArray(Charsets.UTF_8).size <= 120)
        assertFalse(long.last().isHighSurrogate())
        val emoji = OfflineIndex.safeName("😀".repeat(40), "x")
        assertTrue(emoji.toByteArray(Charsets.UTF_8).size <= 120)
        assertEquals(0, emoji.length % 2)
        // invisible characters go: bidi controls, zero-width space, BOM, soft hyphen; line separators become spaces
        assertEquals("ab", OfflineIndex.safeName("a\u200Bb", "x"))
        assertEquals("evil", OfflineIndex.safeName("\u202Eevil\u2069", "x"))
        assertEquals("abc", OfflineIndex.safeName("\uFEFFa\u00ADb\u200Dc", "x"))
        assertEquals("a b", OfflineIndex.safeName("a\u2028b", "x"))
        // the joiners stay inside words of other scripts (Persian, emoji)
        assertEquals("\u0645\u06CC\u200C\u062E\u0648\u0627\u0645", OfflineIndex.safeName("\u0645\u06CC\u200C\u062E\u0648\u0627\u0645", "x"))
        assertEquals("\uD83D\uDC69\u200D\uD83D\uDCBB", OfflineIndex.safeName("\uD83D\uDC69\u200D\uD83D\uDCBB", "x"))
    }

    @Test
    fun deviceNamesWindowsReservesGetAnUnderscore() {
        assertEquals("nul_", OfflineIndex.safeName("nul", "x"))
        assertEquals("CON_", OfflineIndex.safeName("CON", "x"))
        assertEquals("Com1_.live", OfflineIndex.safeName("Com1.live", "x"))
        assertEquals("Aux_", OfflineIndex.safeName("Aux.", "x"))
        assertEquals("Console", OfflineIndex.safeName("Console", "x"))
        assertEquals("Nul_/Aux_", OfflineIndex.albumDir("Nul", "Aux"))
        assertEquals("Con_.flac.flac", OfflineIndex.fileName(0, "Con.flac", "flac"))
        assertEquals("prn_.mp3", OfflineIndex.fileName(0, "prn", "mp3"))
        assertEquals("01 - Nul.flac", OfflineIndex.fileName(1, "Nul", "flac"))
    }

    @Test
    fun filesAreNamedNnTitleExt() {
        assertEquals("01 - So What.flac", OfflineIndex.fileName(1, "So What", "flac"))
        assertEquals("12 - a-b.flac", OfflineIndex.fileName(12, "a/b", "FLAC"))
        assertEquals("Intro.mp3", OfflineIndex.fileName(0, "Intro", "mp3"))
        assertEquals("Unknown song.ogg", OfflineIndex.fileName(3, "", "ogg").removePrefix("03 - "))
        assertEquals("AC-DC/Unknown album", OfflineIndex.albumDir("AC/DC", ""))
        assertEquals("Unknown artist/Back in Black", OfflineIndex.albumDir("  ", "Back in Black"))
        assertEquals("01 - X (2).flac", OfflineIndex.numbered("01 - X.flac", 2))
        assertEquals("noext (3)", OfflineIndex.numbered("noext", 3))
    }

    @Test
    fun pathsFromTheIndexAreUsedOnlyInsideItsFolder() {
        assertEquals("a/b.flac", OfflineIndex.safeRelative("a/b.flac"))
        assertNull(OfflineIndex.safeRelative("../x.flac"))
        assertNull(OfflineIndex.safeRelative("a/../../x.flac"))
        assertNull(OfflineIndex.safeRelative("/sdcard/x.flac"))
        assertNull(OfflineIndex.safeRelative("a//b.flac"))
        assertNull(OfflineIndex.safeRelative("a/./b.flac"))
        assertNull(OfflineIndex.safeRelative("a\\b.flac"))
        assertNull(OfflineIndex.safeRelative(""))
        assertEquals("abc-12_x", OfflineIndex.cleanId("abc-12_x/../"))
    }

    @Test
    fun theDownloadedListGroupsByAlbum() {
        val list = listOf(
            rec(1, album = "Old", albumId = "o", track = 2, savedAt = 10.0),
            rec(2, album = "Old", albumId = "o", track = 1, savedAt = 11.0),
            rec(3, album = "New", albumId = "n", track = 1, savedAt = 50.0, artist = "A"),
            rec(4, album = "New", albumId = "n", track = 2, savedAt = 50.0, artist = "B"),
            rec(5, album = "New", albumId = "n", track = 3, savedAt = 50.0, artist = "C"),
            rec(6, album = "No id", albumId = "", track = 1, savedAt = 5.0),
        )
        val g = OfflineIndex.group(list)
        assertEquals(listOf("New", "Old", "No id"), g.map { it.album })
        assertEquals(listOf(2L, 1L), g[1].songs.map { it.id })
        assertEquals("Various artists", g[0].artist)
        assertEquals("Band", g[1].artist)
        assertEquals("name:no id", g[2].key)
        assertEquals(listOf("Old"), OfflineIndex.group(list, "song 1 band").map { it.album })
    }

    @Test
    fun theWordsMatchTheWeb() {
        assertEquals("Downloading 3 of 12 · Song", OfflineIndex.progressText(2, 12, " Song "))
        assertEquals("Downloading 1 of 1", OfflineIndex.progressText(0, 1, ""))
        assertEquals("12 songs downloaded" to "ok", OfflineIndex.summaryText(12, 0, 0, null))
        assertEquals("1 song downloaded" to "ok", OfflineIndex.summaryText(1, 3, 0, null))
        assertEquals("Already downloaded" to "ok", OfflineIndex.summaryText(0, 2, 0, null))
        assertEquals("Nothing to download" to "ok", OfflineIndex.summaryText(0, 0, 0, null))
        assertEquals("2 songs couldn't be downloaded – Not connected to the server" to "err", OfflineIndex.summaryText(0, 0, 2, OfflineIndex.statusWords(0)))
        assertEquals("10 downloaded, 2 failed – The song is not on the server any more" to "warn", OfflineIndex.summaryText(10, 0, 2, OfflineIndex.statusWords(404)))
        assertEquals("Not signed in to the server", OfflineIndex.statusWords(401))
        assertEquals("The server answered 500", OfflineIndex.statusWords(500))
        assertEquals("Downloaded again in original quality" to "ok", OfflineIndex.againText(1, 0))
        assertEquals(OfflineIndex.STILL_CONVERTED to "warn", OfflineIndex.againText(1, 1))
        assertEquals(OfflineIndex.STILL_CONVERTED_ALL to "warn", OfflineIndex.againText(3, 3))
        assertTrue(OfflineIndex.againText(3, 1).first.startsWith("2 songs downloaded again in original quality; 1 stays converted"))
    }
}
