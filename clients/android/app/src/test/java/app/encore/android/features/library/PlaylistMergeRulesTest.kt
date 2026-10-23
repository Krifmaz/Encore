package app.encore.android.features.library

import app.encore.android.data.api.Playlist
import org.junit.Assert.assertEquals
import org.junit.Test

class PlaylistMergeRulesTest {
    private val all = listOf(
        Playlist(2, "Imported list", "playlist", 3100, 100, ""),
        Playlist(3, "Theirs", "linked", 5, 5, "", readOnly = true),
        Playlist(1, "Liked Songs", "liked", 40, 40, ""),
        Playlist(4, "Road trip", "playlist", 10, 8, ""),
    )

    @Test
    fun targetsTakeSongsLikedFirstAndSkipTheSource() {
        assertEquals(listOf(1L, 4L), PlaylistMergeRules.targets(all, skip = 2).map { it.id })
        assertEquals(listOf("♥ Liked Songs", "Road trip"), PlaylistMergeRules.targets(all, skip = 2).map(PlaylistMergeRules::label))
        assertEquals(listOf(1L, 2L, 4L), PlaylistMergeRules.targets(all).map { it.id })
    }

    @Test
    fun theToastSaysWhatWasAdded() {
        assertEquals("Added 3000 of 3100 tracks (100 already there)", PlaylistMergeRules.mergedText(3000, 3100, 100))
    }
}
