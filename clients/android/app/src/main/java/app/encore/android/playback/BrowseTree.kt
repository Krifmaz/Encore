package app.encore.android.playback

import android.net.Uri
import android.os.Bundle
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.session.MediaConstants
import app.encore.android.data.api.Album
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.data.api.arr
import app.encore.android.data.api.int
import app.encore.android.data.api.obj
import app.encore.android.data.api.objectsOf
import app.encore.android.data.api.str
import kotlinx.serialization.json.JsonPrimitive
import java.util.concurrent.ConcurrentHashMap

/**
 * The library as a browse tree for Android Auto (and anything else that browses media apps), ported from the
 * Tauri app's CarService.Tree. Calls are blocking: the service runs them on its executor.
 *
 * Root (at most 4 tabs, PLAN §10): Home (recently added, recently played, made for you, shuffle everything),
 * Playlists (liked songs first), Library (albums, artists). Every node id CarService used still resolves.
 */
class BrowseTree(private val api: EncoreApi) {
    /** parent id → what we listed under it (playing a listed song plays its siblings). */
    private val children = ConcurrentHashMap<String, List<MediaItem>>()

    fun root(): MediaItem = folder(ROOT, "Encore", null, MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false)

    private fun tabs(): List<MediaItem> = listOf(
        folder("home", "Home", null, MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false),
        folder("playlists", "Playlists", null, MediaMetadata.MEDIA_TYPE_FOLDER_PLAYLISTS, grid = false),
        folder("library", "Library", null, MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false),
    )

    private fun home(): List<MediaItem> = listOf(
        folder("recent", "Recently added", "New in your library", MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS, grid = true),
        folder("played", "Recently played", "Pick up where you left off", MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false),
        folder("mfy", "Made for you", "Your ListenBrainz playlists", MediaMetadata.MEDIA_TYPE_FOLDER_PLAYLISTS, grid = true),
        folder("shuffle", "Shuffle everything", "A random walk through the whole library", MediaMetadata.MEDIA_TYPE_PLAYLIST, grid = false),
    )

    private fun library(): List<MediaItem> = listOf(
        folder("albums", "Albums", null, MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS, grid = true),
        folder("artists", "Artists", null, MediaMetadata.MEDIA_TYPE_FOLDER_ARTISTS, grid = false),
    )

    /** The children of a node, fetched from the server. */
    fun list(parent: String): List<MediaItem> {
        val out: List<MediaItem> = when {
            parent == ROOT -> tabs()
            parent == "home" -> home()
            parent == "library" -> library()
            parent == "recent" -> albums(api.recentAlbums(60))
            parent == "albums" -> albums(api.albums(sort = "artist", limit = 500))
            parent == "artists" -> api.artists().map { a ->
                folder("artist:${a.name}", a.name, plural(a.albums, "album"), MediaMetadata.MEDIA_TYPE_ARTIST, grid = true,
                    art = a.coverAlbum.takeIf { it.isNotEmpty() }?.let { MediaItems.coverUri(it) })
            }
            parent.startsWith("artist:") -> albums(api.albums(artist = parent.removePrefix("artist:"), limit = 200))
            parent.startsWith("album:") -> api.album(parent.removePrefix("album:")).tracks.map { track(it, parent) }
            parent == "playlists" -> {
                val all = api.playlists()
                val liked = all.firstOrNull { it.isLiked }
                val head = if (liked != null) listOf(folder("liked", "Liked Songs", plural(liked.total, "song"), MediaMetadata.MEDIA_TYPE_PLAYLIST, grid = false)) else emptyList()
                head + all.filter { !it.isLiked }.map { p ->
                    folder("playlist:${p.id}", p.name, plural(p.total, "song"), MediaMetadata.MEDIA_TYPE_PLAYLIST, grid = true,
                        art = p.coverUrl.takeIf { it.startsWith("http") }?.let { Uri.parse(it) })
                }
            }
            parent == "liked" -> {
                val liked = api.playlists().firstOrNull { it.isLiked }
                if (liked == null) emptyList() else api.playlist(liked.id).tracks.map { track(it, parent) }
            }
            parent.startsWith("playlist:") -> {
                val id = parent.removePrefix("playlist:").toLongOrNull()
                if (id == null) emptyList() else api.playlist(id).tracks.map { track(it, parent) }
            }
            parent == "mfy" -> api.madeForYou().map { p ->
                folder("mfy:${p.str("kind")}", p.str("name"), "${p.int("owned")} of ${p.int("total")} in your library",
                    MediaMetadata.MEDIA_TYPE_PLAYLIST, grid = true,
                    art = p.arr("covers")?.firstOrNull()?.let { (it as? JsonPrimitive)?.content }
                        ?.takeIf { it.isNotEmpty() }?.let { c -> mfyArt(c) })
            }
            parent.startsWith("mfy:") -> {
                val kind = parent.removePrefix("mfy:")
                val p = api.madeForYou().firstOrNull { it.str("kind") == kind }
                p?.arr("tracks").objectsOf().mapNotNull { row -> row.obj("library")?.let { Track.from(it) } }.map { track(it, parent) }
            }
            parent == "played" -> api.recentPlays(60).map { track(it, parent) }
            parent == "shuffle" -> api.shuffle(100, emptyList()).map { track(it, parent) }
            else -> emptyList()
        }
        children[parent] = out
        return out
    }

    /**
     * A made-for-you cover: our own covers (`/api/library/albums/{id}/cover`) go through [CoverProvider], since
     * Android Auto loads artwork without our session; Cover Art Archive URLs are public and used as they are.
     */
    private fun mfyArt(c: String): Uri? = when {
        c.startsWith("/api/library/albums/") -> {
            val id = c.removePrefix("/api/library/albums/").substringBefore("/cover")
            if (id.isEmpty()) null else MediaItems.coverUri(id)
        }
        c.startsWith("http") -> Uri.parse(c)
        else -> null
    }

    private fun albums(list: List<Album>): List<MediaItem> = list.map { a ->
        val sub = listOf(a.artist, a.year).filter { it.isNotEmpty() }.joinToString(" · ")
        folder("album:${a.id}", a.title, sub, MediaMetadata.MEDIA_TYPE_ALBUM, grid = true, art = MediaItems.coverUri(a.id))
    }

    fun track(t: Track, parent: String?): MediaItem = MediaItems.track(api, t, parent)

    /** Library search for voice commands: songs, then albums, then artists whose names match. */
    fun search(q: String): List<MediaItem> {
        val key = "search:$q"
        val out = ArrayList<MediaItem>()
        out += api.tracks(q = q, limit = 20).map { track(it, key) }
        out += albums(api.albums(q = q, limit = 10))
        out += api.artists(q).take(5).map { a ->
            folder("artist:${a.name}", a.name, "Artist", MediaMetadata.MEDIA_TYPE_ARTIST, grid = true,
                art = a.coverAlbum.takeIf { it.isNotEmpty() }?.let { MediaItems.coverUri(it) })
        }
        children[key] = out.filter { it.mediaId.startsWith("track:") }
        return out
    }

    /** The playable list an item stands for: a song (with its listed siblings), or every song under a folder. */
    fun resolve(mediaId: String): Pair<List<MediaItem>, Int> {
        val tid = MediaItems.trackId(mediaId)
        if (tid != null) {
            val parent = MediaItems.parentOf(mediaId)
            if (parent != null) {
                val siblings = children[parent] ?: try { list(parent) } catch (_: Exception) { null }
                if (siblings != null) {
                    val songs = siblings.filter { it.mediaId.startsWith("track:") }
                    val at = songs.indexOfFirst { MediaItems.trackId(it.mediaId) == tid }
                    if (at >= 0) return songs to at
                }
            }
            val t = api.track(tid) ?: return emptyList<MediaItem>() to 0
            return listOf(track(t, null)) to 0
        }
        val songs = list(mediaId).filter { it.mediaId.startsWith("track:") }
        return songs to 0
    }

    private fun folder(id: String, title: String, subtitle: String?, type: Int, grid: Boolean, art: Uri? = null): MediaItem {
        val extras = Bundle().apply {
            putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_BROWSABLE,
                if (grid) MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_GRID_ITEM else MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM)
            putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_PLAYABLE, MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM)
        }
        val playable = id == "shuffle" || id == "liked" || id.startsWith("playlist:") || id.startsWith("album:") || id.startsWith("mfy:")
        val md = MediaMetadata.Builder()
            .setTitle(title)
            .setSubtitle(subtitle)
            .setIsBrowsable(true)
            .setIsPlayable(playable)
            .setMediaType(type)
            .setExtras(extras)
        if (art != null) md.setArtworkUri(art)
        return MediaItem.Builder().setMediaId(id).setMediaMetadata(md.build()).build()
    }

    private fun plural(n: Int, what: String) = "$n $what${if (n == 1) "" else "s"}"

    companion object {
        const val ROOT = "root"
    }
}
