package app.encore.android.features.library

import app.encore.android.data.api.Album
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Toasts
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// Singles (0.42.11): a release with one song in the library is shown and played as a song, not given an album page.
// A tap on its tile (Library, Home, search, an artist's page and discography) plays the song in the full player; the
// album page stays reachable for what only it has (song ⋯ → Go to album: Tag this album, Change type …). The web's
// common::is_single / library::play_single do the same; the server keeps every single as a release.

/** The pure rules of singles: what makes a release a single and how its tile describes it. */
object SingleRules {
    /** The word a single's tile starts its subtitle with, as on the web ("Song · artist · year"). */
    const val SONG = "Song"

    /** Whether an album with [songs] songs in the library is a single: exactly one song. */
    fun isSingle(songs: Int): Boolean = songs == 1

    /**
     * The subtitle of a library album tile: "artist · year", led by [SONG] for a single. Empty parts are left out.
     */
    fun subtitle(artist: String, year: String, songs: Int): String =
        listOf(if (isSingle(songs)) SONG else "", artist, year).filter { it.isNotEmpty() }.joinToString(" · ")
}

/** Opens a library album tile: a single ([SingleRules.isSingle]) plays as a song, any other album opens its page. */
fun Nav.openAlbum(a: Album) = openLibraryAlbum(a.id, a.trackCount)

/**
 * Opens library album [id] that has [songs] songs (0: not known): a single plays its song in the full player
 * ([playSingle]), anything else opens the album page.
 */
fun Nav.openLibraryAlbum(id: String, songs: Int) {
    if (SingleRules.isSingle(songs)) playSingle(id) else open(Screen.Album(id))
}

/**
 * Plays single [albumId]'s song (the album's songs as loaded, which is that one song) and opens the full player on
 * Now playing. Without a scope (never in the app) or when the album can't be loaded it opens the album page instead,
 * so a tap always leads somewhere.
 */
fun Nav.playSingle(albumId: String) {
    val sc = scope ?: return open(Screen.Album(albumId))
    sc.launch {
        val songs = try {
            withContext(Dispatchers.IO) { graph.api.album(albumId).tracks }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Toasts.show(e.message ?: "Couldn't load the album", "err")
            return@launch
        }
        if (songs.isEmpty()) {
            open(Screen.Album(albumId))
            return@launch
        }
        player.play(songs)
        openPlayer()
    }
}
