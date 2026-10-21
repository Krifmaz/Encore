package app.encore.android.features.localfolders

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalFoldersTest {
    @Test
    fun audioFilesAreToldApartByNameOrType() {
        assertTrue(LocalFolders.isAudio("01 Song.FLAC", ""))
        assertTrue(LocalFolders.isAudio("track", "audio/mpeg"))
        assertFalse(LocalFolders.isAudio("cover.jpg", "image/jpeg"))
        assertFalse(LocalFolders.isAudio("list.m3u", "audio/x-mpegurl"))
    }

    @Test
    fun trackNumbersReadTheirFirstPart() {
        assertEquals(3, LocalFolders.leadingNumber("3/12"))
        assertEquals(7, LocalFolders.leadingNumber(" 07 "))
        assertEquals(0, LocalFolders.leadingNumber(""))
    }

    @Test
    fun aLocalSongNeverHasALibraryId() {
        val a = LocalSong(uri = "content://x/tree/a/document/1", name = "Song.flac")
        val b = LocalSong(uri = "content://x/tree/a/document/2", name = "Other.flac")
        assertTrue(a.localId < 0)
        assertNotEquals(a.localId, b.localId)
        assertEquals("Song", a.asTrack().title)
        assertFalse(a.asTrack().isSurround)
    }

    @Test
    fun theSummarySaysWhatIsInTheLibrary() {
        assertEquals("No music found in these folders yet.", summary(0, 0, false))
        assertEquals("1 song", summary(1, 0, false))
        assertEquals("5 songs · 2 already in your library, played from these files", summary(3, 2, false))
        assertEquals("Looking for music…", summary(3, 2, true))
    }
}
