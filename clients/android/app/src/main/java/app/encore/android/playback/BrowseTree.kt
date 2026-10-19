package app.encore.android.playback

import android.net.Uri
import android.os.Bundle
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.session.MediaConstants
import app.encore.android.core.arr
import app.encore.android.core.int
import app.encore.android.core.obj
import app.encore.android.core.objects
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.Album
import app.encore.android.data.api.Artist
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.features.offline.OfflineIndex
import app.encore.android.features.offline.OfflineStore
import app.encore.android.ui.components.libraryIndex
import kotlinx.serialization.json.JsonPrimitive
import java.text.Normalizer
import java.util.concurrent.ConcurrentHashMap

/**
 * The library as a browse tree for Android Auto (and anything else that browses media apps), ported from the
 * Tauri app's CarService.Tree. Calls are blocking: the service runs them on its executor.
 *
 * Root (at most 4 tabs, PLAN §10): Home (recently added, recently played, made for you, shuffle everything),
 * Playlists (liked songs first), Library (albums, artists, songs), and Downloaded (the songs saved on this phone,
 * by album; read from the offline index, so it works without the server) when there are any. Every node id
 * CarService used still resolves.
 *
 * Android Auto stops scrolling after a few screens (the "speed bump", which an app can't turn off), so the long
 * lists are split into A–Z letter folders (only letters that have items, from GET /api/library/index, else
 * grouped here): a letter is one tap and a short list. Big song letters split once more by their first two
 * letters. Search results come grouped as Songs, Albums, Artists.
 */
class BrowseTree(private val api: EncoreApi, private val offline: OfflineStore? = null) {
    /** parent id → what we listed under it (playing a listed song plays its siblings). */
    private val children = ConcurrentHashMap<String, List<MediaItem>>()

    fun root(): MediaItem = folder(ROOT, "Encore", null, MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false)

    private fun tabs(): List<MediaItem> = listOfNotNull(
        folder("home", "Home", null, MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false),
        folder("playlists", "Playlists", null, MediaMetadata.MEDIA_TYPE_FOLDER_PLAYLISTS, grid = false),
        folder("library", "Library", null, MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false),
        if (saved().isNotEmpty()) downloadedRoot() else null,
    )

    /** The Downloaded node: also the root a browser gets when it asks for what plays offline. */
    fun downloadedRoot(): MediaItem = folder(DOWNLOADED, "Downloaded", null, MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS, grid = false)

    /** The signed-in server's songs saved on this phone. */
    private fun saved() = offline?.songsNow() ?: emptyList()

    /** Downloaded: shuffle them, all songs, then the albums (newest download first). */
    private fun downloaded(): List<MediaItem> {
        val songs = saved()
        if (songs.isEmpty()) return emptyList()
        val albums = OfflineIndex.group(songs)
        return listOf(
            action(DL_SHUFFLE, "Shuffle downloaded songs", plural(songs.size, "song")),
            folder(DL_SONGS, "All songs", plural(songs.size, "song"), MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false),
        ) + albums.map { a ->
            folder(DL_ALBUM + a.key, a.album.ifEmpty { "Unknown album" }, listOf(a.artist, plural(a.songs.size, "song")).filter { it.isNotEmpty() }.joinToString(" · "),
                MediaMetadata.MEDIA_TYPE_ALBUM, grid = true, art = a.albumId.takeIf { it.isNotEmpty() }?.let { MediaItems.coverUri(it) })
        }
    }

    private fun home(): List<MediaItem> = listOf(
        folder("recent", "Recently added", "New in your library", MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS, grid = true),
        folder("played", "Recently played", "Pick up where you left off", MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false),
        folder("mfy", "Made for you", "Your ListenBrainz playlists", MediaMetadata.MEDIA_TYPE_FOLDER_PLAYLISTS, grid = true),
        folder("shuffle", "Shuffle everything", "A random walk through the whole library", MediaMetadata.MEDIA_TYPE_PLAYLIST, grid = false),
    )

    private fun library(): List<MediaItem> = listOf(
        folder("albums", "Albums", null, MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS, grid = false),
        folder("artists", "Artists", null, MediaMetadata.MEDIA_TYPE_FOLDER_ARTISTS, grid = false),
        folder("songs", "Songs", null, MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false),
    )

    /** The children of a node, fetched from the server. */
    fun list(parent: String): List<MediaItem> {
        val out: List<MediaItem> = when {
            parent == ROOT -> tabs()
            parent == DOWNLOADED -> downloaded()
            parent == DL_SONGS -> saved().sortedBy { it.title.lowercase() }.map { track(OfflineStore.trackOf(it), parent) }
            parent == DL_SHUFFLE -> saved().shuffled().take(500).map { track(OfflineStore.trackOf(it), parent) }
            parent.startsWith(DL_ALBUM) -> {
                val key = parent.removePrefix(DL_ALBUM)
                OfflineIndex.group(saved()).firstOrNull { it.key == key }?.songs.orEmpty().map { track(OfflineStore.trackOf(it), parent) }
            }
            parent == "home" -> home()
            parent == "library" -> library()
            parent == "recent" -> albums(api.recentAlbums(60))
            parent == "albums" -> albumShelf.letters().map { (label, count) ->
                folder("$ALBUM_LETTER$label", label, count?.let { plural(it, "album") }, MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS, grid = true)
            }
            parent.startsWith(ALBUM_LETTER) -> albums(albumShelf.items(parent.removePrefix(ALBUM_LETTER)))
            parent == "artists" -> artistShelf.letters().map { (label, count) ->
                folder("$ARTIST_LETTER$label", label, count?.let { plural(it, "artist") }, MediaMetadata.MEDIA_TYPE_FOLDER_ARTISTS, grid = false)
            }
            parent.startsWith(ARTIST_LETTER) -> artistShelf.items(parent.removePrefix(ARTIST_LETTER)).map { artist(it, plural(it.albums, "album")) }
            parent.startsWith("artist:") -> {
                val name = parent.removePrefix("artist:")
                listOf(action("$ARTIST_SHUFFLE$name", "Shuffle $name", null)) + albums(api.albums(artist = name, limit = 200))
            }
            parent == "songs" -> listOf(action(SONGS_SHUFFLE, "Shuffle all songs", "A random walk through every song")) +
                songShelf.letters().map { (label, count) ->
                    folder("$SONG_LETTER$label", label, count?.let { plural(it, "song") }, MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false)
                }
            parent.startsWith(SONG_LETTER) -> songLetter(parent)
            parent == SONGS_SHUFFLE -> api.shuffle(100, emptyList()).map { track(it, parent) }
            parent.startsWith(ARTIST_SHUFFLE) -> api.artistTracks(parent.removePrefix(ARTIST_SHUFFLE)).shuffled().take(500).map { track(it, parent) }
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

    private fun artist(a: Artist, subtitle: String): MediaItem =
        folder("artist:${a.name}", a.name, subtitle, MediaMetadata.MEDIA_TYPE_ARTIST, grid = true,
            art = a.coverAlbum.takeIf { it.isNotEmpty() }?.let { MediaItems.coverUri(it) })

    fun track(t: Track, parent: String?): MediaItem = MediaItems.track(api, t, parent)

    /**
     * Library search (the car's search button and voice): the server's ranked results, grouped as Songs (30),
     * Albums, Artists. A result song plays with the other result songs as its queue.
     */
    fun search(q: String): List<MediaItem> {
        val key = "search:$q"
        // the server can't be reached: the saved songs that match
        val songs = try {
            api.tracks(q = q, limit = 30).map { track(it, key) }
        } catch (e: Exception) {
            val found = saved().filter { OfflineIndex.matches(it, q) }.take(30).map { track(OfflineStore.trackOf(it), key) }
            if (found.isEmpty()) throw e
            children[key] = found
            return found.map { grouped(it, "Songs") }
        }
        children[key] = songs
        val out = ArrayList<MediaItem>()
        out += songs.map { grouped(it, "Songs") }
        out += albums(api.albums(q = q, limit = 12)).map { grouped(it, "Albums") }
        out += api.artists(q).take(6).map { grouped(artist(it, "Artist"), "Artists") }
        return out
    }

    /** The item with Android Auto's group title (results with the same title are shown under one heading). */
    private fun grouped(item: MediaItem, group: String): MediaItem {
        val md = item.mediaMetadata
        val extras = Bundle(md.extras ?: Bundle.EMPTY)
        extras.putString(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_GROUP_TITLE, group)
        return item.buildUpon().setMediaMetadata(md.buildUpon().setExtras(extras).build()).build()
    }

    // ---- A–Z letter folders ------------------------------------------------------------------------------

    /**
     * A library list split by letter: the server's index (sections with the offset of their first row in the
     * list's own order) and the rows of one letter fetched by offset. Servers without the index: the first
     * [FALLBACK_ROWS] rows grouped here.
     */
    private class Section(val label: String, val offset: Int, val count: Int?)

    private inner class Shelf<T>(
        private val kind: String,
        private val sort: String,
        private val pageSize: Int,
        private val page: (offset: Int, limit: Int) -> List<T>,
        private val name: (T) -> String,
    ) {
        @Volatile private var sections: List<Section> = emptyList()
        @Volatile private var loadedAt = 0L
        /** without the server index: every row we fetched, in list order */
        @Volatile private var rows: List<T>? = null

        private fun load(): List<Section> {
            val now = System.currentTimeMillis()
            if (sections.isNotEmpty() && now - loadedAt < INDEX_TTL_MS) return sections
            val index = try { api.libraryIndex(kind, sort) } catch (_: Exception) { emptyList() }
            if (index.isNotEmpty()) {
                rows = null
                sections = index.mapIndexed { i, s ->
                    val next = index.getOrNull(i + 1)?.offset
                    Section(s.label, s.offset, next?.let { (it - s.offset).coerceAtLeast(0) })
                }
            } else {
                val all = ArrayList<T>()
                while (all.size < FALLBACK_ROWS) {
                    val got = page(all.size, pageSize)
                    all.addAll(got)
                    if (got.size < pageSize) break
                }
                rows = all
                val out = ArrayList<Section>()
                var i = 0
                while (i < all.size) {
                    val label = letterOf(name(all[i]))
                    var j = i + 1
                    while (j < all.size && letterOf(name(all[j])) == label) j++
                    // a letter that comes back later (names sorted by another rule) joins its first section
                    if (out.none { it.label == label }) out.add(Section(label, i, j - i))
                    i = j
                }
                sections = out
            }
            loadedAt = now
            return sections
        }

        /** (label, how many rows when known) for every letter that has rows. */
        fun letters(): List<Pair<String, Int?>> = load().map { it.label to it.count }

        /** The rows of one letter (at most [LETTER_ROWS]). */
        fun items(label: String): List<T> {
            val s = load().firstOrNull { it.label == label } ?: return emptyList()
            val all = rows
            if (all != null) return all.drop(s.offset).take(s.count ?: LETTER_ROWS).filter { letterOf(name(it)) == label }
            val want = minOf(s.count ?: LETTER_ROWS, LETTER_ROWS)
            val out = ArrayList<T>()
            var at = s.offset
            while (out.size < want) {
                val n = minOf(pageSize, want - out.size)
                val got = page(at, n)
                out.addAll(got)
                at += got.size
                if (got.size < n) break
            }
            return out
        }
    }

    private val albumShelf = Shelf<Album>(
        "albums", "album", 500,
        { offset, limit -> api.get("/api/library/albums?sort=album&offset=$offset&limit=$limit").objects().mapNotNull { Album.from(it) } },
        { it.title },
    )

    private val artistShelf = Shelf<Artist>(
        "artists", "name", 2000,
        { offset, limit -> api.get("/api/library/artists?sort=name&offset=$offset&limit=$limit").objects().mapNotNull { Artist.from(it) } },
        { it.name },
    )

    private val songShelf = Shelf<Track>(
        "tracks", "title", 1000,
        { offset, limit -> api.tracks(offset = offset, limit = limit, sort = "title") },
        { it.title },
    )

    /** The songs of one letter folder (`songs:letter:S`, or `songs:letter:S|Sa` for a part of a big letter). */
    private val letterSongs = ConcurrentHashMap<String, Pair<Long, List<Track>>>()

    private fun songsOfLetter(label: String): List<Track> {
        val now = System.currentTimeMillis()
        letterSongs[label]?.let { (at, list) -> if (now - at < INDEX_TTL_MS) return list }
        val list = songShelf.items(label)
        letterSongs[label] = now to list
        return list
    }

    /**
     * A letter folder: its songs in title order, or – more than [SPLIT_AT] songs – one folder per first two
     * letters ("Sa", "Se", …), so no list gets long.
     */
    private fun songLetter(id: String): List<MediaItem> {
        val rest = id.removePrefix(SONG_LETTER)
        val label = rest.substringBefore("|")
        val part = if (rest.contains("|")) rest.substringAfter("|") else null
        val songs = songsOfLetter(label)
        if (part != null) return songs.filter { prefixOf(it.title, label) == part }.map { track(it, id) }
        if (songs.size <= SPLIT_AT) return songs.map { track(it, id) }
        val parts = LinkedHashMap<String, Int>()
        for (t in songs) { val p = prefixOf(t.title, label); parts[p] = (parts[p] ?: 0) + 1 }
        return parts.map { (p, n) -> folder("$SONG_LETTER$label|$p", p, plural(n, "song"), MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false) }
    }

    /** "Sa" for "Sabotage" under "S"; the letter alone when the second character isn't a letter. */
    private fun prefixOf(title: String, label: String): String {
        val second = fold(title.trim()).getOrNull(1)
        return if (second != null && second.isLetter()) label + second.lowercaseChar() else label
    }

    /** Every song a playable folder stands for (a whole letter, not its parts). */
    private fun songsFor(folderId: String): List<MediaItem>? = when {
        folderId.startsWith(SONG_LETTER) -> {
            val rest = folderId.removePrefix(SONG_LETTER)
            if (rest.contains("|")) songLetter(folderId)
            else songsOfLetter(rest).map { track(it, folderId) }.also { children[folderId] = it }
        }
        else -> null
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
            // a song saved on this phone needs no server
            val t = try { api.track(tid) } catch (e: Exception) { offline?.record(tid)?.let { OfflineStore.trackOf(it) } ?: throw e }
                ?: return emptyList<MediaItem>() to 0
            return listOf(track(t, null)) to 0
        }
        val songs = (songsFor(mediaId) ?: list(mediaId)).filter { it.mediaId.startsWith("track:") }
        return songs to 0
    }

    /** Voice requests without the server: the saved songs that match ("play music": all of them, shuffled). */
    fun downloadedFor(q: String): List<MediaItem> {
        val songs = saved()
        val found = if (q.isBlank()) songs.shuffled() else songs.filter { OfflineIndex.matches(it, q) }
        return found.take(500).map { track(OfflineStore.trackOf(it), null) }
    }

    /** A playable entry that isn't a folder ("Shuffle all songs", "Shuffle <artist>"): tapping it plays. */
    private fun action(id: String, title: String, subtitle: String?): MediaItem {
        val md = MediaMetadata.Builder()
            .setTitle(title)
            .setSubtitle(subtitle)
            .setIsBrowsable(false)
            .setIsPlayable(true)
            .setMediaType(MediaMetadata.MEDIA_TYPE_PLAYLIST)
        return MediaItem.Builder().setMediaId(id).setMediaMetadata(md.build()).build()
    }

    private fun folder(id: String, title: String, subtitle: String?, type: Int, grid: Boolean, art: Uri? = null): MediaItem {
        val extras = Bundle().apply {
            putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_BROWSABLE,
                if (grid) MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_GRID_ITEM else MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM)
            putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_PLAYABLE, MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM)
        }
        val playable = id == "shuffle" || id == "liked" || id.startsWith("playlist:") || id.startsWith("album:") || id.startsWith("mfy:") ||
            id.startsWith(SONG_LETTER) || id == DL_SONGS || id.startsWith(DL_ALBUM)
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

        /** Root extras: Android Auto shows its on-screen search button (androidx.media's BROWSER_SERVICE_EXTRAS_KEY_SEARCH_SUPPORTED). */
        const val EXTRAS_SEARCH_SUPPORTED = "android.media.browse.SEARCH_SUPPORTED"

        private const val ALBUM_LETTER = "albums:letter:"
        private const val ARTIST_LETTER = "artists:letter:"
        private const val SONG_LETTER = "songs:letter:"
        private const val SONGS_SHUFFLE = "songs:shuffle"
        private const val ARTIST_SHUFFLE = "artist-shuffle:"
        /** the songs saved on this phone (features/offline) */
        private const val DOWNLOADED = "downloaded"
        private const val DL_SONGS = "downloaded:songs"
        private const val DL_SHUFFLE = "downloaded:shuffle"
        private const val DL_ALBUM = "downloaded:album:"

        private const val INDEX_TTL_MS = 10 * 60_000L
        /** without the server index: how many rows of a list we group here */
        private const val FALLBACK_ROWS = 5000
        /** most rows one letter folder lists */
        private const val LETTER_ROWS = 3000
        /** a song letter with more songs than this is split by its first two letters */
        private const val SPLIT_AT = 150

        private val MARKS = Regex("\\p{Mn}+")

        private fun fold(s: String): String = Normalizer.normalize(s, Normalizer.Form.NFD).replace(MARKS, "")

        /** The server index's letter rule (routes/library_index.rs): A–Z, "#" for digits and symbols, accents folded. */
        fun letterOf(name: String): String {
            val c = name.firstOrNull() ?: return "#"
            if (c.code < 128) return if (c.isLetter()) c.uppercaseChar().toString() else "#"
            val f = fold(c.toString()).firstOrNull()?.lowercaseChar() ?: return "#"
            return if (f in 'a'..'z') f.uppercaseChar().toString() else "#"
        }
    }
}
