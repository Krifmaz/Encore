package app.encore.android.data.api

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

// The server's rows come straight from SQLite (dynamic types: `year` may be text or a number, counts may be
// null), so responses are read leniently from JsonObject instead of strict generated decoders. Requests we
// send are @Serializable classes.

internal fun JsonObject.str(key: String): String {
    val v = this[key] ?: return ""
    if (v is JsonNull) return ""
    return if (v is JsonPrimitive) v.content else v.toString()
}

internal fun JsonObject.dbl(key: String): Double {
    val v = this[key] as? JsonPrimitive ?: return 0.0
    if (v is JsonNull) return 0.0
    return v.content.toDoubleOrNull() ?: 0.0
}

internal fun JsonObject.long(key: String): Long = dbl(key).toLong()

internal fun JsonObject.int(key: String): Int = dbl(key).toInt()

internal fun JsonObject.bool(key: String): Boolean {
    val v = this[key] as? JsonPrimitive ?: return false
    if (v is JsonNull) return false
    val c = v.content
    return c == "true" || (c.toDoubleOrNull() ?: 0.0) != 0.0
}

internal fun JsonObject.obj(key: String): JsonObject? = this[key] as? JsonObject

internal fun JsonObject.arr(key: String): JsonArray? = this[key] as? JsonArray

/** A list response: a bare array, or `{items: [...]}` (albums), as JSON objects. */
internal fun JsonElement?.objects(): List<JsonObject> {
    val list: JsonArray = when (this) {
        is JsonArray -> this
        is JsonObject -> this.arr("items") ?: return emptyList()
        else -> return emptyList()
    }
    val out = ArrayList<JsonObject>(list.size)
    for (e in list) if (e is JsonObject) out.add(e)
    return out
}

/**
 * One artist of a song (the server's `artists` on a song: "Travis Scott feat. The Weeknd" → two), with its
 * MusicBrainz id when known and whether it is an artist of the library.
 */
data class ArtistCredit(val name: String, val mbid: String = "", val inLibrary: Boolean = false) {
    companion object {
        fun fromJson(e: JsonElement?): List<ArtistCredit> {
            val arr = e as? JsonArray ?: return emptyList()
            val out = ArrayList<ArtistCredit>(arr.size)
            for (x in arr) {
                val o = x as? JsonObject ?: continue
                val name = o.str("name")
                if (name.isNotEmpty()) out.add(ArtistCredit(name, o.str("mbid"), o.bool("in_library")))
            }
            return out
        }
    }
}

/** One song as the library lists return it. */
data class Track(
    val id: Long,
    val title: String,
    val artist: String,
    val album: String,
    val albumArtist: String,
    val albumId: String,
    val trackNo: Int,
    val disc: Int,
    val year: String,
    val durationSec: Double,
    val format: String,
    val bitrate: Int,
    /** each artist of the song for links (empty: the whole artist text is one artist) */
    val artists: List<ArtistCredit> = emptyList(),
    /** translation / romanization of the title, artist and album (Translations.kt; empty when none) */
    val titleTr: Tr = Tr.NONE,
    val artistTr: Tr = Tr.NONE,
    val albumTr: Tr = Tr.NONE,
) {
    /** The artists to link: the server's split, or the artist text as one. */
    fun credits(): List<ArtistCredit> = artists.ifEmpty { if (artist.isNotEmpty()) listOf(ArtistCredit(artist)) else emptyList() }

    /** The title, artist and album as the listener asked to see them (Settings → Language). */
    fun shownTitle(): String = Translations.shown(title, titleTr)
    fun titleUnder(): String? = Translations.under(titleTr)
    fun shownArtist(): String = Translations.inline(artist, artistTr)
    fun shownAlbum(): String = Translations.inline(album, albumTr)

    companion object {
        fun from(o: JsonObject): Track? {
            val id = o.long("id")
            if (id <= 0L) return null
            return fromRow(o, id)
        }

        /** A playlist row: its matched library song is `track_id` (the row's own `id` is the playlist item). */
        fun fromPlaylistItem(o: JsonObject): Track? {
            val id = o.long("track_id")
            if (id <= 0L) return null
            return fromRow(o, id)
        }

        private fun fromRow(o: JsonObject, id: Long): Track = Track(
            id = id,
            title = o.str("title").ifEmpty { "Unknown title" },
            artist = o.str("artist"),
            album = o.str("album"),
            albumArtist = o.str("albumartist").ifEmpty { o.str("artist") },
            albumId = o.str("album_id"),
            trackNo = o.int("track"),
            disc = o.int("disc"),
            year = o.str("year"),
            durationSec = o.dbl("duration"),
            format = o.str("format").lowercase(),
            bitrate = o.int("bitrate"),
            artists = ArtistCredit.fromJson(o["artists"]),
            titleTr = Tr.of(o, "title"),
            artistTr = Tr.of(o, "artist"),
            albumTr = Tr.of(o, "album"),
        )
    }
}

data class Album(
    val id: String,
    val title: String,
    val artist: String,
    val year: String,
    val trackCount: Int,
    val formats: String,
    val titleTr: Tr = Tr.NONE,
    val artistTr: Tr = Tr.NONE,
) {
    fun shownTitle(): String = Translations.shown(title, titleTr)
    fun titleUnder(): String? = Translations.under(titleTr)
    fun shownArtist(): String = Translations.inline(artist, artistTr)

    companion object {
        fun from(o: JsonObject): Album? {
            val id = o.str("id")
            if (id.isEmpty()) return null
            return Album(
                id = id,
                title = o.str("album").ifEmpty { "Unknown album" },
                artist = o.str("albumartist"),
                year = o.str("year").take(4),
                trackCount = o.int("tracks"),
                formats = o.str("formats"),
                titleTr = Tr.of(o, "album"),
                artistTr = Tr.of(o, "albumartist"),
            )
        }
    }
}

data class AlbumDetail(val album: Album, val tracks: List<Track>, val durationSec: Double)

data class Artist(val name: String, val albums: Int, val tracks: Int, val coverAlbum: String, val nameTr: Tr = Tr.NONE) {
    fun shownName(): String = Translations.inline(name, nameTr)

    companion object {
        fun from(o: JsonObject): Artist? {
            val name = o.str("name")
            if (name.isEmpty()) return null
            return Artist(name, o.int("albums"), o.int("tracks"), o.str("cover_album"), Tr.of(o, "name"))
        }
    }
}

/**
 * GET /api/library/search: the library's answer to a query read as artist + title in any order (typos and
 * accents forgiven), with its Top result (a song, album or artist).
 */
data class LibrarySearch(
    val topKind: String,
    val topSong: Track?,
    val topAlbum: Album?,
    val topArtist: Artist?,
    val songs: List<Track>,
    val albums: List<Album>,
    val artists: List<Artist>,
) {
    val isEmpty: Boolean get() = songs.isEmpty() && albums.isEmpty() && artists.isEmpty()

    companion object {
        fun from(o: JsonObject): LibrarySearch {
            val top = o.obj("top")
            val kind = top?.str("kind") ?: ""
            val item = top?.obj("item")
            return LibrarySearch(
                topKind = kind,
                topSong = if (kind == "song") item?.let { Track.from(it) } else null,
                topAlbum = if (kind == "album") item?.let { Album.from(it) } else null,
                topArtist = if (kind == "artist") item?.let { Artist.from(it) } else null,
                songs = o.arr("songs").objects().mapNotNull { Track.from(it) },
                albums = o.arr("albums").objects().mapNotNull { Album.from(it) },
                artists = o.arr("artists").objects().mapNotNull { Artist.from(it) },
            )
        }
    }
}

data class Playlist(val id: Long, val name: String, val kind: String, val total: Int, val matched: Int, val coverUrl: String) {
    val isLiked: Boolean get() = kind == "liked"

    companion object {
        fun from(o: JsonObject): Playlist? {
            val id = o.long("id")
            if (id <= 0L) return null
            return Playlist(id, o.str("name"), o.str("kind"), o.int("total"), o.int("matched"), o.str("cover_url"))
        }
    }
}

/** A playlist with its rows; rows without a library match have `track == null`. */
data class PlaylistDetail(val playlist: Playlist, val rows: List<PlaylistRow>) {
    val tracks: List<Track> get() = rows.mapNotNull { it.track }
}

data class PlaylistRow(val title: String, val artist: String, val track: Track?)

/** GET /api/auth/info – what this server can do. */
data class AuthInfo(val appName: String, val version: String, val signupMode: String, val features: List<String>) {
    /** Server change S1: long-lived bearer tokens for apps. */
    val appTokens: Boolean get() = "app_tokens" in features

    companion object {
        fun from(o: JsonObject): AuthInfo {
            val features = o.arr("features")?.mapNotNull { (it as? JsonPrimitive)?.content } ?: emptyList()
            return AuthInfo(o.str("app_name").ifEmpty { "Encore" }, o.str("version"), o.str("signup_mode"), features)
        }
    }
}

data class User(val id: Long, val username: String, val isAdmin: Boolean) {
    companion object {
        fun from(o: JsonObject): User = User(o.long("id"), o.str("username"), o.bool("is_admin"))
    }
}

// ---- request bodies -----------------------------------------------------------------------------------

@Serializable
data class AppDevice(
    @SerialName("device_id") val deviceId: String,
    @SerialName("device_name") val deviceName: String,
)

/** POST /api/auth/login. `code` is the authenticator code (2FA); `app` asks for a bearer app token (S1). */
@Serializable
data class LoginBody(
    val username: String,
    val password: String,
    val code: String? = null,
    val app: AppDevice? = null,
)

@Serializable
data class PlayBody(
    @SerialName("track_id") val trackId: Long,
    val seconds: Double = 0.0,
)

@Serializable
data class ShuffleBody(val limit: Int, val exclude: List<Long>)

@Serializable
data class LinkBody(val url: String)

/** POST /api/playback/dsp/prepare – render a song ahead on the server's Cadenza. */
@Serializable
data class DspPrepareBody(
    @SerialName("track_id") val trackId: Long,
    val rate: Int,
    val filter: String,
    val dither: String,
)
