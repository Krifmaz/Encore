package app.encore.android.data.repo

import app.encore.android.data.api.Track
import app.encore.android.data.api.arr
import app.encore.android.data.api.bool
import app.encore.android.data.api.dbl
import app.encore.android.data.api.int
import app.encore.android.data.api.long
import app.encore.android.data.api.obj
import app.encore.android.data.api.objectsOf
import app.encore.android.data.api.str
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.Date

// Rows of the Discover / For you / Requests / Picker / Account / Leaderboard pages (web/js/pages/*.js), read
// leniently from the server's JSON like data/api/Models.kt does.

/** A JSON array of strings (or nothing). */
internal fun JsonArray?.strings(): List<String> {
    if (this == null) return emptyList()
    val out = ArrayList<String>(size)
    for (e in this) if (e is JsonPrimitive && e !is JsonNull) out.add(e.content)
    return out
}

/** `str` that reads JSON null as "" and a missing key as "" (the server sends `null` for unknown fields). */
internal fun JsonObject.text(key: String): String {
    val v = this[key] ?: return ""
    if (v is JsonNull) return ""
    return if (v is JsonPrimitive) (if (v.isString) v.content else v.content.takeIf { it != "null" } ?: "") else v.toString()
}

// ---- web/js/ui.js STATUS / statusBadge ----------------------------------------------------------------------

/** The web's `STATUS`: request status → (badge kind, label). */
val REQUEST_STATUS: Map<String, Pair<String, String>> = mapOf(
    "pending" to ("warn" to "Awaiting approval"),
    "queued" to ("info" to "Queued"),
    "searching" to ("info" to "Searching"),
    "monitoring" to ("accent" to "Monitoring"),
    "downloading" to ("accent" to "Downloading"),
    "importing" to ("accent" to "Importing"),
    "completed" to ("ok" to "Available"),
    "failed" to ("err" to "Failed"),
    "denied" to ("err" to "Denied"),
    "cancelled" to ("" to "Cancelled"),
)

/** The statuses that are still moving (web/js/pages/common.js `active`). */
val ACTIVE_STATUSES: Set<String> = setOf("searching", "downloading", "importing", "monitoring")

/** Statuses a request can be cancelled in (common.js `requestRow`). */
val CANCELLABLE_STATUSES: Set<String> = setOf("pending", "queued", "searching", "downloading", "monitoring", "importing")

fun statusLabel(status: String): String = REQUEST_STATUS[status]?.second ?: status

fun statusKind(status: String): String = REQUEST_STATUS[status]?.first ?: ""

// ---- web/js/ui.js fmt.ago ------------------------------------------------------------------------------------

/** `fmt.ago`: "just now", "5m ago", "3h ago", "12d ago", else the date; "never" for nothing. */
fun formatAgo(epochSeconds: Double): String {
    if (epochSeconds <= 0.0) return "never"
    val d = System.currentTimeMillis() / 1000.0 - epochSeconds
    return when {
        d < 60 -> "just now"
        d < 3600 -> "${(d / 60).toInt()}m ago"
        d < 86400 -> "${(d / 3600).toInt()}h ago"
        d < 86400 * 30 -> "${(d / 86400).toInt()}d ago"
        else -> java.text.DateFormat.getDateInstance(java.text.DateFormat.SHORT).format(Date((epochSeconds * 1000).toLong()))
    }
}

// ---- the signed-in user (GET /api/auth/me) ---------------------------------------------------------------------

/** GET /api/auth/me with what the pages gate on (web/js/app.js `session.user`). */
data class Me(
    val id: Long,
    val username: String,
    val role: String,
    val email: String,
    val perms: Map<String, Boolean>,
    val requestLimit: Int,
    val requestsThisWeek: Int,
    val autoApprove: Boolean,
    val totpEnabled: Boolean,
    val avatarAt: Double,
    val createdAt: Double,
) {
    val isAdmin: Boolean get() = role == "admin" || role == "owner"

    /** `auth::can`: admins can do everything, others what their perms say. */
    fun can(perm: String): Boolean = isAdmin || (perms[perm] ?: false)

    companion object {
        fun from(o: JsonObject): Me {
            val perms = HashMap<String, Boolean>()
            o.obj("perms")?.forEach { (k, v) -> perms[k] = (v as? JsonPrimitive)?.content.let { it == "true" || it == "1" } }
            return Me(
                id = o.long("id"),
                username = o.str("username"),
                role = o.str("role"),
                email = o.text("email"),
                perms = perms,
                requestLimit = o.int("request_limit"),
                requestsThisWeek = o.int("requests_this_week"),
                autoApprove = o.bool("auto_approve"),
                totpEnabled = o.bool("totp_enabled"),
                avatarAt = o.dbl("avatar_at"),
                createdAt = o.dbl("created_at"),
            )
        }
    }
}

/** One row of GET /api/auth/sessions. */
data class SessionRow(
    val id: String,
    val current: Boolean,
    val createdAt: Double,
    val lastSeenAt: Double,
    val userAgent: String,
    val ip: String,
    val device: String,
    val kind: String,
    val deviceName: String,
) {
    /** The web's title: "Encore app · <device> · last used …" for apps, else the device name. */
    val title: String
        get() = if (kind == "app") {
            listOf("Encore app", deviceName, "last used " + formatAgo(if (lastSeenAt > 0) lastSeenAt else createdAt)).filter { it.isNotEmpty() }.joinToString(" · ")
        } else {
            deviceName.ifEmpty { device.ifEmpty { userAgent.ifEmpty { "Unknown device" } } }
        }

    companion object {
        fun from(o: JsonObject): SessionRow = SessionRow(
            id = o.str("id"),
            current = o.bool("current"),
            createdAt = o.dbl("created_at"),
            lastSeenAt = o.dbl("last_seen_at"),
            userAgent = o.text("user_agent"),
            ip = o.text("ip"),
            device = o.text("device"),
            kind = o.str("kind"),
            deviceName = o.text("device_name"),
        )
    }
}

/** POST /api/auth/2fa/setup. */
data class TotpSetup(val secret: String, val otpauthUrl: String, val qrBase64: String) {
    companion object {
        fun from(o: JsonObject): TotpSetup = TotpSetup(o.str("secret"), o.str("otpauth_url"), o.text("qr"))
    }
}

// ---- Discover (MusicBrainz) ------------------------------------------------------------------------------------

/** An artist from search (mb::search_artists) or a similar/For-you artist card. */
data class CatalogArtist(
    val id: String,
    val name: String,
    val type: String,
    val country: String,
    val disambiguation: String,
    val inLibrary: Boolean,
    val image: String,
) {
    companion object {
        fun from(o: JsonObject): CatalogArtist? {
            val name = o.text("name")
            if (name.isEmpty()) return null
            return CatalogArtist(
                id = o.text("id").ifEmpty { o.text("mbid") },
                name = name,
                type = o.text("type"),
                country = o.text("country"),
                disambiguation = o.text("disambiguation"),
                inLibrary = o.bool("in_library"),
                image = o.text("image"),
            )
        }
    }
}

/** A release group (search, discography) with the library annotations (`annotate_albums`). */
data class CatalogAlbum(
    val id: String,
    val title: String,
    val artist: String,
    val artistId: String,
    val type: String,
    val secondary: List<String>,
    val year: String,
    val date: String,
    val cover: String,
    val inLibrary: Boolean,
    val libraryAlbumId: String,
    val requestStatus: String,
    /** Deezer / Apple ids of an album only they know (search results): requested without a MusicBrainz id. */
    val ext: JsonObject? = null,
    /** The words the pages show for the request ("Requested", "Not found"…), when the server sent them. */
    val requestLabel: String = "",
) {
    companion object {
        fun from(o: JsonObject): CatalogAlbum? {
            val id = o.text("id")
            val title = o.text("title")
            if (id.isEmpty() && title.isEmpty()) return null
            return CatalogAlbum(
                id = id,
                title = title,
                artist = o.text("artist"),
                artistId = o.text("artist_id"),
                type = o.text("type"),
                secondary = o.arr("secondary").strings(),
                year = o.text("year").take(4),
                date = o.text("date"),
                cover = o.text("cover"),
                inLibrary = o.bool("in_library"),
                libraryAlbumId = o.text("library_album_id"),
                requestStatus = o.text("request_status"),
                ext = o.obj("ext"),
                requestLabel = o.text("request_label"),
            )
        }
    }
}

/** A recording from search (mb::search_recordings) with the library match. */
data class CatalogTrack(
    val id: String,
    val title: String,
    val artist: String,
    val artistId: String,
    val album: String,
    val releaseGroupId: String,
    val year: String,
    val lengthSec: Double,
    val cover: String,
    val inLibrary: Boolean,
    val library: Track?,
    val requestStatus: String,
    val isrc: String = "",
    val requestLabel: String = "",
) {
    /** The key request statuses are kept under (songs only Deezer / Apple know have no MusicBrainz id). */
    val key: String get() = id.ifEmpty { "ext:$artist/$title" }

    companion object {
        fun from(o: JsonObject): CatalogTrack? {
            val title = o.text("title")
            if (title.isEmpty()) return null
            return CatalogTrack(
                id = o.text("id"),
                title = title,
                artist = o.text("artist"),
                artistId = o.text("artist_id"),
                album = o.text("album"),
                releaseGroupId = o.text("release_group_id"),
                year = o.text("year").take(4),
                lengthSec = o.dbl("length"),
                cover = o.text("cover"),
                inLibrary = o.bool("in_library"),
                library = o.obj("library")?.let { Track.from(it) },
                requestStatus = o.text("request_status"),
                isrc = o.text("isrc"),
                requestLabel = o.text("request_label"),
            )
        }
    }
}

/** The Top result of a Discover search: one of [track], [album] or [artist], by [kind] (song | album | artist). */
data class CatalogTop(val kind: String, val track: CatalogTrack?, val album: CatalogAlbum?, val artist: CatalogArtist?) {
    companion object {
        fun from(o: JsonObject?): CatalogTop? {
            if (o == null) return null
            val kind = o.text("kind")
            val item = o.obj("item") ?: return null
            val top = CatalogTop(
                kind,
                if (kind == "song") CatalogTrack.from(item) else null,
                if (kind == "album") CatalogAlbum.from(item) else null,
                if (kind == "artist") CatalogArtist.from(item) else null,
            )
            return if (top.track == null && top.album == null && top.artist == null) null else top
        }
    }
}

data class SearchResults(
    val artists: List<CatalogArtist>,
    val albums: List<CatalogAlbum>,
    val tracks: List<CatalogTrack>,
    val top: CatalogTop? = null,
) {
    val isEmpty: Boolean get() = artists.isEmpty() && albums.isEmpty() && tracks.isEmpty()

    companion object {
        fun from(o: JsonObject): SearchResults = SearchResults(
            artists = o.arr("artists").objectsOf().mapNotNull { CatalogArtist.from(it) },
            albums = o.arr("albums").objectsOf().mapNotNull { CatalogAlbum.from(it) },
            tracks = o.arr("tracks").objectsOf().mapNotNull { CatalogTrack.from(it) },
            top = CatalogTop.from(o.obj("top")),
        )
    }
}

/** An album of this artist that is in the library (GET /api/discover/artist/{mbid} `library_albums[]`). */
data class LibraryAlbumRef(val albumId: String, val album: String, val albumArtist: String, val year: String, val rgid: String) {
    companion object {
        fun from(o: JsonObject): LibraryAlbumRef? {
            val id = o.text("album_id")
            if (id.isEmpty()) return null
            return LibraryAlbumRef(id, o.text("album").ifEmpty { "Unknown album" }, o.text("albumartist"), o.text("year").take(4), o.text("rgid"))
        }
    }
}

/** GET /api/discover/artist/{mbid}. */
data class ArtistPage(
    val id: String,
    val name: String,
    val type: String,
    val country: String,
    val begin: String,
    val disambiguation: String,
    val tags: List<String>,
    val links: List<Pair<String, String>>,
    val releaseGroups: List<CatalogAlbum>,
    val libraryAlbums: List<LibraryAlbumRef>,
    val libraryTracks: Int,
    val requestStatus: String,
    val requestLabel: String = "",
    /** What "Request discography" includes: singles too (server setting), Deezer / Apple Music releases too. */
    val discographySingles: Boolean = false,
    val catalogMerge: Boolean = false,
) {
    companion object {
        /** The links the web shows (discover.js `links`). */
        private val LINK_KINDS = listOf("official homepage", "bandcamp", "soundcloud", "youtube", "streaming", "free streaming", "wikidata", "discogs", "last.fm")

        fun from(o: JsonObject): ArtistPage {
            val links = ArrayList<Pair<String, String>>()
            o.obj("links")?.forEach { (k, v) ->
                if (k in LINK_KINDS && v is JsonPrimitive) links.add(k to v.content)
            }
            return ArtistPage(
                id = o.text("id"),
                name = o.text("name"),
                type = o.text("type"),
                country = o.text("country"),
                begin = o.text("begin"),
                disambiguation = o.text("disambiguation"),
                tags = o.arr("tags").strings(),
                links = links.take(6),
                releaseGroups = o.arr("release_groups").objectsOf().mapNotNull { CatalogAlbum.from(it) },
                libraryAlbums = o.arr("library_albums").objectsOf().mapNotNull { LibraryAlbumRef.from(it) },
                libraryTracks = o.int("library_tracks"),
                requestStatus = o.text("request_status"),
                requestLabel = o.text("request_label"),
                discographySingles = o.bool("discography_singles"),
                catalogMerge = o.bool("catalog_merge"),
            )
        }
    }
}

/** GET /api/discover/artist/{mbid}/extras `top[]` (Deezer's popular songs). */
data class TopTrack(val title: String, val version: String, val album: String, val cover: String, val durationSec: Double, val isrc: String, val library: Track?) {
    companion object {
        fun from(o: JsonObject): TopTrack? {
            val title = o.text("title")
            if (title.isEmpty()) return null
            return TopTrack(title, o.text("version"), o.text("album"), o.text("cover"), o.dbl("duration"), o.text("isrc"), o.obj("library")?.let { Track.from(it) })
        }
    }
}

data class ArtistExtras(val image: String, val bio: String, val bioUrl: String, val top: List<TopTrack>) {
    companion object {
        fun from(o: JsonObject): ArtistExtras = ArtistExtras(o.text("image"), o.text("bio"), o.text("bio_url"), o.arr("top").objectsOf().mapNotNull { TopTrack.from(it) })
    }
}

data class SimilarArtists(val source: String, val artists: List<CatalogArtist>) {
    companion object {
        fun from(o: JsonObject): SimilarArtists = SimilarArtists(o.text("source"), o.arr("artists").objectsOf().mapNotNull { CatalogArtist.from(it) })
    }
}

/** A release Deezer / Apple Music / Bandcamp / Discogs know that MusicBrainz doesn't (GET …/catalog `releases[]`). */
data class ExtRelease(
    val title: String,
    val year: String,
    val date: String,
    val type: String,
    val tracks: Int,
    val cover: String,
    val inLibrary: Boolean,
    val libraryAlbumId: String,
    val requestStatus: String,
    val sources: List<Pair<String, String>>,
    val harmony: String,
    val ext: JsonElement,
    val requestLabel: String = "",
) {
    companion object {
        fun from(o: JsonObject): ExtRelease? {
            val title = o.text("title")
            if (title.isEmpty()) return null
            val sources = o.arr("sources").objectsOf().map { it.text("name") to it.text("url") }.filter { it.first.isNotEmpty() }
            return ExtRelease(
                title = title,
                year = o.text("year").take(4),
                date = o.text("date"),
                type = o.text("type"),
                tracks = o.int("tracks"),
                cover = o.text("cover"),
                inLibrary = o.bool("in_library"),
                libraryAlbumId = o.text("library_album_id"),
                requestStatus = o.text("request_status"),
                sources = sources,
                harmony = o.text("harmony"),
                ext = o["ext"] ?: JsonNull,
                requestLabel = o.text("request_label"),
            )
        }
    }
}

/** A track of an external release (POST /api/discover/catalog/tracks), with its library copy (playable) and any request. */
data class ExtTrack(
    val track: Int,
    val title: String,
    val artist: String,
    val lengthSec: Double,
    val isrc: String = "",
    val inLibrary: Boolean = false,
    val library: Track? = null,
    val requestStatus: String = "",
    val requestLabel: String = "",
) {
    companion object {
        fun from(o: JsonObject): ExtTrack? {
            val title = o.text("title")
            if (title.isEmpty()) return null
            return ExtTrack(
                o.int("track"), title, o.text("artist"), o.dbl("length"), o.text("isrc"), o.bool("in_library"),
                o.obj("library")?.let { Track.from(it) }, o.text("request_status"), o.text("request_label"),
            )
        }
    }
}

/** One track of a MusicBrainz release (GET /api/discover/album/{rgid} `release.tracks[]`). */
data class ReleaseTrack(
    val disc: Int,
    val track: Int,
    val title: String,
    val artist: String,
    val lengthSec: Double,
    val recordingId: String,
    val inLibrary: Boolean,
    val library: Track?,
    val isrc: String = "",
    val requestStatus: String = "",
    val requestLabel: String = "",
) {
    companion object {
        fun from(o: JsonObject): ReleaseTrack? {
            val title = o.text("title")
            if (title.isEmpty()) return null
            return ReleaseTrack(
                disc = o.int("disc"),
                track = o.int("track"),
                title = title,
                artist = o.text("artist"),
                lengthSec = o.dbl("length"),
                recordingId = o.text("recording_id"),
                inLibrary = o.bool("in_library"),
                library = o.obj("library")?.let { Track.from(it) },
                isrc = o.text("isrc"),
                requestStatus = o.text("request_status"),
                requestLabel = o.text("request_label"),
            )
        }
    }
}

data class Release(val tracks: List<ReleaseTrack>, val discCount: Int, val label: String) {
    companion object {
        fun from(o: JsonObject): Release = Release(o.arr("tracks").objectsOf().mapNotNull { ReleaseTrack.from(it) }, o.int("disc_count"), o.text("label"))
    }
}

/** GET /api/discover/album/{rgid}: the release group plus the best release's track list. */
data class AlbumPage(
    val id: String,
    val title: String,
    val artist: String,
    val artistId: String,
    val type: String,
    val secondary: List<String>,
    val year: String,
    val cover: String,
    val tags: List<String>,
    val inLibrary: Boolean,
    val libraryAlbumId: String,
    val requestStatus: String,
    val release: Release?,
    val requestLabel: String = "",
    /** Songs of the album in the library (Play / Shuffle show when there is any). */
    val ownedTracks: Int = 0,
) {
    companion object {
        fun from(o: JsonObject): AlbumPage = AlbumPage(
            id = o.text("id"),
            title = o.text("title"),
            artist = o.text("artist"),
            artistId = o.text("artist_id"),
            type = o.text("type"),
            secondary = o.arr("secondary").strings(),
            year = o.text("year").take(4),
            cover = o.text("cover"),
            tags = o.arr("tags").strings(),
            inLibrary = o.bool("in_library"),
            libraryAlbumId = o.text("library_album_id"),
            requestStatus = o.text("request_status"),
            release = o.obj("release")?.let { Release.from(it) },
            requestLabel = o.text("request_label"),
            ownedTracks = o.int("owned_tracks"),
        )
    }
}

// ---- For you ---------------------------------------------------------------------------------------------------

/** A recommended song (annotate_tracks): the library match when there is one, else a request status. */
data class FyTrack(
    val mbid: String,
    val title: String,
    val artist: String,
    val artistMbid: String,
    val album: String,
    val cover: String,
    val lengthSec: Double,
    val isrc: String,
    val library: Track?,
    val requestStatus: String,
) {
    companion object {
        fun from(o: JsonObject): FyTrack? {
            val title = o.text("title")
            if (title.isEmpty()) return null
            return FyTrack(
                mbid = o.text("mbid"),
                title = title,
                artist = o.text("artist"),
                artistMbid = o.text("artist_mbid"),
                album = o.text("album"),
                cover = o.text("cover"),
                lengthSec = o.dbl("length"),
                isrc = o.text("isrc"),
                library = o.obj("library")?.let { Track.from(it) },
                requestStatus = o.text("request_status"),
            )
        }
    }

    /** The body of POST /api/requests for this song (foryou.js `reqBody`). */
    fun requestBody(): JsonObject = buildJsonObject {
        put("kind", "track")
        if (mbid.isNotEmpty()) put("mbid", mbid)
        put("artist", artist)
        put("title", title)
        if (album.isNotEmpty()) put("album", album)
        if (cover.isNotEmpty()) put("cover_url", cover)
        if (lengthSec > 0) put("length", lengthSec)
        if (isrc.isNotEmpty()) put("isrc", isrc)
    }
}

data class FyPlaylist(val id: String, val title: String, val kind: String, val tracks: List<FyTrack>) {
    companion object {
        fun from(o: JsonObject): FyPlaylist = FyPlaylist(o.text("id"), o.text("title"), o.text("kind"), o.arr("tracks").objectsOf().mapNotNull { FyTrack.from(it) })
    }
}

/** A Last.fm album pick ("Albums for you"). */
data class FyAlbum(val title: String, val artist: String, val cover: String, val year: String, val inLibrary: Boolean, val requestStatus: String) {
    companion object {
        fun from(o: JsonObject): FyAlbum? {
            val title = o.text("title")
            if (title.isEmpty()) return null
            return FyAlbum(title, o.text("artist"), o.text("cover"), o.text("year").take(4), o.bool("in_library"), o.text("request_status"))
        }
    }
}

data class LastFm(val user: String, val tracks: List<FyTrack>, val artists: List<CatalogArtist>, val albums: List<FyAlbum>) {
    companion object {
        fun from(o: JsonObject): LastFm = LastFm(
            user = o.text("user"),
            tracks = o.arr("tracks").objectsOf().mapNotNull { FyTrack.from(it) },
            artists = o.arr("artists").objectsOf().mapNotNull { CatalogArtist.from(it) },
            albums = o.arr("albums").objectsOf().mapNotNull { FyAlbum.from(it) },
        )
    }
}

/** GET /api/discover/for-you. */
data class ForYou(val connected: Boolean, val user: String, val playlists: List<FyPlaylist>, val lastfm: LastFm?, val errors: List<String>) {
    companion object {
        fun from(o: JsonObject): ForYou = ForYou(
            connected = o.bool("connected"),
            user = o.text("user"),
            playlists = o.arr("playlists").objectsOf().map { FyPlaylist.from(it) },
            lastfm = o.obj("lastfm")?.let { LastFm.from(it) },
            errors = o.arr("errors").strings(),
        )
    }
}

data class Tags(val library: List<String>, val top: List<String>) {
    companion object {
        fun from(o: JsonObject): Tags = Tags(o.arr("library").strings(), o.arr("top").strings())
    }
}

data class TagPage(val source: String, val summary: String, val artists: List<CatalogArtist>) {
    companion object {
        fun from(o: JsonObject): TagPage = TagPage(o.text("source"), o.text("summary"), o.arr("artists").objectsOf().mapNotNull { CatalogArtist.from(it) })
    }
}

data class SimilarTracks(val source: String, val tracks: List<FyTrack>) {
    companion object {
        fun from(o: JsonObject): SimilarTracks = SimilarTracks(o.text("source"), o.arr("tracks").objectsOf().mapNotNull { FyTrack.from(it) })
    }
}

// ---- Requests --------------------------------------------------------------------------------------------------

/** One row of GET /api/requests. */
data class RequestRow(
    val id: Long,
    val kind: String,
    val mbid: String,
    val artist: String,
    val title: String,
    val album: String,
    val url: String,
    val status: String,
    val message: String,
    val source: String,
    val progress: Int,
    val coverUrl: String,
    val createdAt: Double,
    val username: String,
    val parentId: Long,
    val lengthSec: Double,
) {
    val isActive: Boolean get() = status in ACTIVE_STATUSES

    /** The web's `what` line: "Title · Artist", "Artist · full discography", "Title · playlist import"… */
    val what: String
        get() = when (kind) {
            "album" -> title.ifEmpty { album }
            "artist" -> artist
            "url" -> title.ifEmpty { url }
            else -> title
        }

    val whatNote: String
        get() = when (kind) {
            "album" -> artist
            "artist" -> "full discography"
            "url" -> ""
            "playlist" -> "playlist import"
            else -> artist
        }

    companion object {
        fun from(o: JsonObject): RequestRow? {
            val id = o.long("id")
            if (id <= 0L) return null
            return RequestRow(
                id = id,
                kind = o.text("kind"),
                mbid = o.text("mbid"),
                artist = o.text("artist"),
                title = o.text("title"),
                album = o.text("album"),
                url = o.text("url"),
                status = o.text("status"),
                message = o.text("message"),
                source = o.text("source"),
                progress = o.int("progress"),
                coverUrl = o.text("cover_url"),
                createdAt = o.dbl("created_at"),
                username = o.text("username"),
                parentId = o.long("parent_id"),
                lengthSec = o.dbl("length"),
            )
        }
    }
}

// ---- Picker ----------------------------------------------------------------------------------------------------

data class PickerSource(val id: String, val name: String)

data class PickerInfo(val allowed: Boolean, val sources: List<PickerSource>) {
    companion object {
        fun from(o: JsonObject): PickerInfo = PickerInfo(
            allowed = o.bool("allowed"),
            sources = o.arr("sources").objectsOf().map { PickerSource(it.text("id"), it.text("name")) }.filter { it.id.isNotEmpty() },
        )
    }
}

/** One result of POST /api/picker/search. */
data class PickResult(
    val source: String,
    val title: String,
    val subtitle: String,
    val quality: String,
    val lossless: Boolean,
    val tracks: Int,
    val size: Long,
    val durationSec: Double,
    val info: String,
    val warning: String,
    val cover: String,
    val payload: JsonElement,
) {
    companion object {
        fun from(o: JsonObject): PickResult? {
            val title = o.text("title")
            if (title.isEmpty()) return null
            return PickResult(
                source = o.text("source"),
                title = title,
                subtitle = o.text("subtitle"),
                quality = o.text("quality"),
                lossless = o.bool("lossless"),
                tracks = o.int("tracks"),
                size = o.long("size"),
                durationSec = o.dbl("duration"),
                info = o.text("info"),
                warning = o.text("warning"),
                cover = o.text("cover"),
                payload = o["payload"] ?: JsonObject(emptyMap()),
            )
        }
    }
}

/** What the picker is looking for (picker.js `opts`). */
data class PickerOpts(
    val kind: String,
    val artist: String = "",
    val title: String = "",
    val album: String = "",
    val mbid: String = "",
    val artistMbid: String = "",
    val trackCount: Int = 0,
    val lengthSec: Double = 0.0,
    val year: String = "",
    val coverUrl: String = "",
    val isrc: String = "",
    val requestId: Long = 0L,
    val ext: JsonElement? = null,
)

// ---- Playlists -------------------------------------------------------------------------------------------------

/** One row of GET /api/playlists/{id} `items[]`, with the item id needed to remove and move it. */
data class PlaylistItem(
    val id: Long,
    val title: String,
    val artist: String,
    val album: String,
    val albumId: String,
    val trackId: Long,
    val requestId: Long,
    val requestStatus: String,
    val verifyStatus: String,
    val matchReason: String,
    val track: Track?,
) {
    companion object {
        fun from(o: JsonObject): PlaylistItem? {
            val id = o.long("id")
            if (id <= 0L) return null
            return PlaylistItem(
                id = id,
                title = o.text("title"),
                artist = o.text("artist"),
                album = o.text("album"),
                albumId = o.text("album_id"),
                trackId = o.long("track_id"),
                requestId = o.long("request_id"),
                requestStatus = o.text("request_status"),
                verifyStatus = o.text("verify_status"),
                matchReason = o.text("match_reason"),
                track = Track.fromPlaylistItem(o),
            )
        }
    }
}

data class PlaylistPage(
    val id: Long,
    val name: String,
    val kind: String,
    val source: String,
    val sourceUrl: String,
    val follow: Boolean,
    val total: Int,
    val matched: Int,
    val coverUrl: String,
    val items: List<PlaylistItem>,
) {
    val isLiked: Boolean get() = kind == "liked"

    companion object {
        fun from(o: JsonObject): PlaylistPage = PlaylistPage(
            id = o.long("id"),
            name = o.text("name"),
            kind = o.text("kind"),
            source = o.text("source"),
            sourceUrl = o.text("source_url"),
            follow = o.bool("follow"),
            total = o.int("total"),
            matched = o.int("matched"),
            coverUrl = o.text("cover_url"),
            items = o.arr("items").objectsOf().mapNotNull { PlaylistItem.from(it) },
        )
    }
}

/** POST /api/playlists/preview. */
data class ImportPreview(
    val name: String,
    val source: String,
    val count: Int,
    val inLibrary: Int,
    val duplicates: Int,
    val warning: String,
    val tracks: List<ImportTrack>,
) {
    companion object {
        fun from(o: JsonObject): ImportPreview = ImportPreview(
            name = o.text("name"),
            source = o.text("source"),
            count = o.int("count"),
            inLibrary = o.int("in_library"),
            duplicates = o.int("duplicates"),
            warning = o.text("warning"),
            tracks = o.arr("tracks").objectsOf().map { ImportTrack(it.text("artist"), it.text("title"), it.text("album"), it.text("isrc").isNotEmpty(), it.long("track_id") > 0, it.text("match_reason")) },
        )
    }
}

data class ImportTrack(val artist: String, val title: String, val album: String, val hasIsrc: Boolean, val inLibrary: Boolean, val matchReason: String)

/** POST /api/playlists/import. */
data class ImportResult(val id: Long, val added: Int, val duplicatesSkipped: Int, val requested: Int) {
    companion object {
        fun from(o: JsonObject): ImportResult = ImportResult(o.long("id"), o.int("added"), o.int("duplicates_skipped"), o.int("requested"))
    }
}

/** web/js/pages/playlists.js `SRC`: where a playlist came from. */
val PLAYLIST_SOURCES: Map<String, String> = mapOf(
    "spotify" to "Spotify", "ytmusic" to "YouTube Music", "deezer" to "Deezer", "tidal" to "Tidal", "qobuz" to "Qobuz", "apple" to "Apple Music",
    "ytdlp" to "YouTube / SoundCloud", "listenbrainz" to "ListenBrainz", "web" to "Web", "text" to "Pasted list", "merged" to "Merged",
    "manual" to "Encore", "library" to "Encore", "jellyfin" to "Jellyfin", "plex" to "Plex", "navidrome" to "Navidrome",
)

// ---- Leaderboard -----------------------------------------------------------------------------------------------

data class BoardRow(val username: String, val value: Double, val me: Boolean)

data class Board(val key: String, val label: String, val rows: List<BoardRow>)

data class TopSong(val title: String, val artist: String, val albumId: String, val plays: Long)

data class TopArtist(val artist: String, val plays: Long, val listeners: Int)

data class LibraryArtistStat(val artist: String, val albums: Int, val songs: Long)

data class Fact(val label: String, val value: String)

/** GET /api/stats/leaderboard. */
data class Leaderboard(
    val boards: List<Board>,
    val topSongs: List<TopSong>,
    val topArtists: List<TopArtist>,
    val thisWeek: List<Pair<String, Long>>,
    val libraryArtists: List<LibraryArtistStat>,
    val facts: List<Fact>,
) {
    companion object {
        fun from(o: JsonObject): Leaderboard {
            val boards = ArrayList<Board>()
            o.obj("boards")?.forEach { (k, v) ->
                if (v is JsonObject) {
                    boards.add(Board(k, v.text("label"), v.arr("rows").objectsOf().map { BoardRow(it.text("username"), it.dbl("value"), it.bool("me")) }))
                }
            }
            return Leaderboard(
                boards = boards,
                topSongs = o.arr("top_songs").objectsOf().map { TopSong(it.text("title"), it.text("artist"), it.text("album_id"), it.long("plays")) },
                topArtists = o.arr("top_artists").objectsOf().map { TopArtist(it.text("artist"), it.long("plays"), it.int("listeners")) },
                thisWeek = o.arr("this_week").objectsOf().map { it.text("username") to it.long("plays") },
                libraryArtists = o.arr("library_artists").objectsOf().map { LibraryArtistStat(it.text("artist"), it.int("albums"), it.long("songs")) },
                facts = o.arr("facts").objectsOf().map { Fact(it.text("label"), it.text("value")) },
            )
        }
    }
}
