package app.encore.android.data.api

import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.int
import app.encore.android.core.long
import app.encore.android.core.obj
import app.encore.android.core.objects
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.features.library.PlaylistShareRules
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

// Responses are read leniently (app.encore.android.core: Json.kt); requests we send are @Serializable classes.

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
    /** the song's loudness for volume normalization (the row's `gain`; null when the server doesn't know it) */
    val gain: TrackGain? = null,
    /** the file's sample rate in Hz and bit depth (0 when unknown: lossy files, servers before 0.31) */
    val sampleRate: Int = 0,
    val bitDepth: Int = 0,
    /** the request that brought the file in (0: none); members with `fix` may correct such songs */
    val requestId: Long = 0L,
    /**
     * the audio check's verdict: "verified", "mismatch" (wrong audio), "manual" (someone confirmed it is the right
     * song); "" when unchecked. [verifyDetail] says why ("Confirmed by Caz", what the fingerprint found).
     */
    val verifyStatus: String = "",
    val verifyDetail: String = "",
    /** who requested the song (only the album page's rows say; "" elsewhere) */
    val requestedBy: String = "",
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
            gain = TrackGain.from(o["gain"]),
            sampleRate = o.int("sample_rate"),
            bitDepth = o.int("bit_depth"),
            requestId = o.long("request_id"),
            verifyStatus = o.str("verify_status"),
            verifyDetail = o.str("verify_detail"),
            requestedBy = o.str("requested_by"),
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
    /** `counts`: every match per section before the limit (0.31); null from older servers */
    val counts: SearchCounts? = null,
) {
    val isEmpty: Boolean get() = songs.isEmpty() && albums.isEmpty() && artists.isEmpty()

    companion object {
        fun from(o: JsonObject): LibrarySearch {
            val top = o.obj("top")
            val kind = top?.str("kind") ?: ""
            val item = top?.obj("item")
            val counts = o.obj("counts")?.let { c -> SearchCounts(c.int("artists"), c.int("albums"), c.int("songs")) }
            return LibrarySearch(
                counts = counts,
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

/** GET /api/library/search `counts`: how many artists, albums and songs match in all ("Show all N"). */
data class SearchCounts(val artists: Int, val albums: Int, val songs: Int)

/**
 * GET /api/library/artists/{name}/tracks and /liked (0.31): one page of an artist's songs in the library and how
 * many there are in all.
 */
data class ArtistSongs(val total: Int, val tracks: List<Track>) {
    companion object {
        val EMPTY = ArtistSongs(0, emptyList())

        /** `{total, tracks}`; a bare array is read as the whole list. */
        fun from(e: JsonElement?): ArtistSongs {
            if (e is JsonArray) {
                val list = e.objectsOf().mapNotNull { Track.from(it) }
                return ArtistSongs(list.size, list)
            }
            val o = e as? JsonObject ?: return EMPTY
            val list = (o.arr("tracks") ?: o.arr("items")).objectsOf().mapNotNull { Track.from(it) }
            return ArtistSongs(if (o.containsKey("total")) o.int("total").coerceAtLeast(list.size) else list.size, list)
        }
    }
}

/** [sharePath]: the public link's path ("/s/<token>") while the playlist is shared, else "". */
data class Playlist(val id: Long, val name: String, val kind: String, val total: Int, val matched: Int, val coverUrl: String, val sharePath: String = "") {
    val isLiked: Boolean get() = kind == "liked"
    val isShared: Boolean get() = sharePath.isNotEmpty()

    companion object {
        fun from(o: JsonObject): Playlist? {
            val id = o.long("id")
            if (id <= 0L) return null
            return Playlist(id, o.str("name"), o.str("kind"), o.int("total"), o.int("matched"), o.str("cover_url"), PlaylistShareRules.sharePathOf(o))
        }
    }
}

/** A playlist with its rows; rows without a library match have `track == null`. */
data class PlaylistDetail(val playlist: Playlist, val rows: List<PlaylistRow>) {
    val tracks: List<Track> get() = rows.mapNotNull { it.track }
}

/** `requestStatus` is the linked request's status ("" when never requested or the request is gone). */
data class PlaylistRow(val title: String, val artist: String, val track: Track?, val requestStatus: String = "")

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
