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
import app.encore.android.features.library.PlaylistCollabRules
import app.encore.android.features.library.PlaylistLinkRules
import app.encore.android.features.library.PlaylistShareRules
import app.encore.android.features.library.SurroundRules
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
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
    /**
     * the Surround library (0.35): `class` ("surround" on its songs), their [quality] ("Dolby Atmos (E-AC-3 5.1)")
     * and stereo album; on a stereo album page's rows, the song's surround version ([surroundId], 0: none)
     */
    val trackClass: String = "",
    val quality: String = "",
    val atmos: Boolean = false,
    val stereoAlbumId: String = "",
    val surroundId: Long = 0L,
) {
    /** A song of the Surround library (by its class, or its id after a trip through the player): no likes or playlists. */
    val isSurround: Boolean get() = trackClass == "surround" || SurroundRules.isSurroundTrack(id)

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
            trackClass = o.str("class"),
            quality = o.str("quality"),
            atmos = o.bool("atmos"),
            stereoAlbumId = o.str("stereo_album_id"),
            surroundId = o.long("surround_id"),
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
    /** a Surround album's (0.35) format ("Dolby Atmos (E-AC-3 5.1)", "E-AC-3 5.1") and its stereo album ("" when none) */
    val atmos: Boolean = false,
    val quality: String = "",
    val stereoAlbumId: String = "",
) {
    fun shownTitle(): String = Translations.shown(title, titleTr)
    fun titleUnder(): String? = Translations.under(titleTr)
    fun shownArtist(): String = Translations.inline(artist, artistTr)

    /** An album of the Surround library (its id starts with "s"). */
    val isSurround: Boolean get() = SurroundRules.isSurroundAlbum(id)

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
                atmos = o.bool("atmos"),
                quality = o.str("quality"),
                stereoAlbumId = o.str("stereo_album_id"),
            )
        }
    }
}

data class AlbumDetail(val album: Album, val tracks: List<Track>, val durationSec: Double)

/** GET /api/surround/albums (0.35): a page of the Surround library's albums, and the library's totals. */
data class SurroundAlbums(val albums: List<Album>, val stats: SurroundStats) {
    companion object {
        fun from(o: JsonObject): SurroundAlbums {
            val s = o.obj("stats")
            val stats = SurroundStats(s?.int("albums") ?: 0, s?.int("atmos") ?: 0, s?.int("tracks") ?: 0, s?.long("size") ?: 0L, s?.dbl("duration") ?: 0.0)
            return SurroundAlbums(o.arr("albums").objectsOf().mapNotNull { Album.from(it) }, stats)
        }
    }
}

/** `stats` of GET /api/surround/albums: albums (of them in Dolby Atmos), songs, bytes, seconds. */
data class SurroundStats(val albums: Int, val atmos: Int, val tracks: Int, val size: Long, val durationSec: Double)

/** One surround version of a stereo album (`surround.have[]`): its album in the Surround library. */
data class SurroundVersion(val albumId: String, val tracks: Int, val quality: String)

/** The request fetching a Dolby Atmos version (`surround.request`), with its status ("pending", "downloading" …). */
data class SurroundRequest(val id: Long, val status: String)

/**
 * A stereo album's `surround` (0.35): its versions in the Surround library, the request fetching one, whether Apple
 * Music has one ([available]: null when never asked) and whether the server can fetch one (gamdl with wrapper-v2).
 */
data class SurroundState(val have: List<SurroundVersion>, val request: SurroundRequest?, val available: Boolean?, val canFetch: Boolean) {
    companion object {
        /** Null for an older server's album (no `surround`). */
        fun from(e: JsonElement?): SurroundState? {
            val o = e as? JsonObject ?: return null
            val have = o.arr("have").objectsOf().mapNotNull { h ->
                val id = h.str("album_id")
                if (id.isEmpty()) null else SurroundVersion(id, h.int("tracks"), h.str("quality"))
            }
            val request = o.obj("request")?.let { r -> if (r.long("id") > 0L) SurroundRequest(r.long("id"), r.str("status")) else null }
            val available = (o["available"] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.let { o.bool("available") }
            return SurroundState(have, request, available, o.bool("can_fetch"))
        }
    }
}

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
/** A page of the library's albums and how many the list has (GET /api/library/albums). */
data class AlbumsPage(val total: Int, val albums: List<Album>)

/**
 * GET /api/library/artists/{name}/one-offs (0.36): the artist's songs whose album is a One-off, with each song's
 * `release_label` ("Compilation", "Various artists", "Album" …) in [labels] (same order).
 */
data class OneOffs(val total: Int, val tracks: List<Track>, val labels: List<String>) {
    companion object {
        val EMPTY = OneOffs(0, emptyList(), emptyList())

        fun from(e: JsonElement?): OneOffs {
            val o = e as? JsonObject ?: return EMPTY
            val rows = o.arr("tracks").objectsOf().mapNotNull { r -> Track.from(r)?.let { it to r.str("release_label") } }
            val total = if (o.containsKey("total")) o.int("total").coerceAtLeast(rows.size) else rows.size
            return OneOffs(total, rows.map { it.first }, rows.map { it.second })
        }
    }
}

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

/**
 * [sharePath]: the public link's path ("/s/<token>") while the playlist is shared, else "". [addedBy]: how many people
 * added it to their playlists (the owner's count, never who).
 *
 * `kind: "linked"` is someone else's playlist added through its share link ([PlaylistLinkRules]): read-only
 * ([readOnly]), by [linkedOwner], live while [linkedActive], [linkedLiked] when it is the owner's Liked Songs.
 */
data class Playlist(
    val id: Long,
    val name: String,
    val kind: String,
    val total: Int,
    val matched: Int,
    val coverUrl: String,
    val sharePath: String = "",
    val readOnly: Boolean = false,
    val linkedOwner: String = "",
    val linkedActive: Boolean = false,
    val linkedLiked: Boolean = false,
    val addedBy: Int = 0,
    /** Collaborative (features/library/PlaylistCollabRules): on for the owner's, or someone else's the user joined. */
    val collab: PlaylistCollabRules.Collab = PlaylistCollabRules.Collab(false, "", "", 0),
) {
    val isLiked: Boolean get() = kind == "liked"
    val isShared: Boolean get() = sharePath.isNotEmpty()
    val isLinked: Boolean get() = kind == "linked"
    /** Someone else's collaborative playlist the user joined: its songs are theirs to change, the rest is the owner's. */
    val isMember: Boolean get() = collab.isMember

    companion object {
        fun from(o: JsonObject): Playlist? {
            val id = o.long("id")
            if (id <= 0L) return null
            val linked = PlaylistLinkRules.linkedOf(o)
            return Playlist(
                id, o.str("name"), o.str("kind"), o.int("total"), o.int("matched"), o.str("cover_url"), PlaylistShareRules.sharePathOf(o),
                readOnly = linked.readOnly,
                linkedOwner = linked.owner,
                linkedActive = linked.active,
                linkedLiked = linked.liked,
                addedBy = PlaylistShareRules.addedByOf(o),
                collab = PlaylistCollabRules.collabOf(o),
            )
        }
    }
}

/** A playlist with its rows; rows without a library match have `track == null`. */
data class PlaylistDetail(val playlist: Playlist, val rows: List<PlaylistRow>) {
    val tracks: List<Track> get() = rows.mapNotNull { it.track }
}

/**
 * `requestStatus` is the linked request's status ("" when never requested or the request is gone). [album] is the
 * playlist's own album text, so the playlist search finds rows that aren't in the library by their album too.
 */
data class PlaylistRow(val title: String, val artist: String, val track: Track?, val requestStatus: String = "", val album: String = "")

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
