package app.encore.android.data.repo

import app.encore.android.core.Checked
import app.encore.android.core.Items
import app.encore.android.core.LBool
import app.encore.android.core.LBoolOrNull
import app.encore.android.core.LDouble
import app.encore.android.core.LInt
import app.encore.android.core.LLong
import app.encore.android.core.LObject
import app.encore.android.core.LString
import app.encore.android.core.LTexts
import app.encore.android.core.LYear
import app.encore.android.core.Nested
import app.encore.android.core.arr
import app.encore.android.core.decodeModel
import app.encore.android.core.obj
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.Track
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.Transient
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonDecoder
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.IOException
import java.util.Date

// Rows of the Discover / For you / Requests / Picker / Account / Leaderboard pages (web/js/pages/*.js), as
// @Serializable models read leniently (app.encore.android.core, Json.kt): the `L…` field types turn a missing, null
// or odd value into the empty value, [Items] lists skip rows that don't decode, and [Checked] leaves out rows
// without what they need (a title, an id) and fills fields that fall back on others. FeaturesApi reads answers with
// [page] and [rows].

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

// ---- reading the answers ------------------------------------------------------------------------------------

/** A page's answer as its model (every field lenient: only a broken structure fails, as a server error would). */
internal fun <T> JsonObject.page(serializer: KSerializer<T>): T =
    decodeModel(serializer) ?: throw IOException("Unexpected reply from the server")

/** A list answer's rows as models; a row that doesn't decode, or that its model leaves out, is skipped. */
internal fun <T> List<JsonObject>.rows(serializer: KSerializer<T>): List<T> = mapNotNull { it.decodeModel(serializer) }

/** A library song inside a row (`library`), or null when it is anything but a song of the library. */
internal object LibraryTrack : KSerializer<Track?> {
    override val descriptor: SerialDescriptor = JsonObject.serializer().descriptor
    override fun deserialize(decoder: Decoder): Track? {
        val o = (decoder as? JsonDecoder)?.decodeJsonElement() as? JsonObject ?: return null
        return Track.from(o)
    }
    override fun serialize(encoder: Encoder, value: Track?) = throw UnsupportedOperationException("read only")
}

private typealias LTrack = @Serializable(with = LibraryTrack::class) Track?

/** `perms` of GET /api/auth/me: permission → granted (`true` or 1, as the server's SQLite flags come). */
internal object Perms : KSerializer<Map<String, Boolean>> {
    override val descriptor: SerialDescriptor = JsonObject.serializer().descriptor
    override fun deserialize(decoder: Decoder): Map<String, Boolean> {
        val o = (decoder as? JsonDecoder)?.decodeJsonElement() as? JsonObject ?: return emptyMap()
        val out = HashMap<String, Boolean>()
        for ((k, v) in o) out[k] = (v as? JsonPrimitive)?.content.let { it == "true" || it == "1" }
        return out
    }
    override fun serialize(encoder: Encoder, value: Map<String, Boolean>) = throw UnsupportedOperationException("read only")
}

// ---- the signed-in user (GET /api/auth/me) ---------------------------------------------------------------------

/** GET /api/auth/me with what the pages gate on (web/js/app.js `session.user`). */
@Serializable
data class Me(
    val id: LLong = 0L,
    val username: LString = "",
    val role: LString = "",
    val email: LString = "",
    @Serializable(with = Perms::class) val perms: Map<String, Boolean> = emptyMap(),
    @SerialName("request_limit") val requestLimit: LInt = 0,
    @SerialName("requests_this_week") val requestsThisWeek: LInt = 0,
    @SerialName("auto_approve") val autoApprove: LBool = false,
    @SerialName("totp_enabled") val totpEnabled: LBool = false,
    @SerialName("avatar_at") val avatarAt: LDouble = 0.0,
    @SerialName("created_at") val createdAt: LDouble = 0.0,
) {
    val isAdmin: Boolean get() = role == "admin" || role == "owner"

    /** `auth::can`: admins can do everything, others what their perms say. */
    fun can(perm: String): Boolean = isAdmin || (perms[perm] ?: false)
}

/** One row of GET /api/auth/sessions. */
@Serializable
data class SessionRow(
    val id: LString = "",
    val current: LBool = false,
    @SerialName("created_at") val createdAt: LDouble = 0.0,
    @SerialName("last_seen_at") val lastSeenAt: LDouble = 0.0,
    @SerialName("user_agent") val userAgent: LString = "",
    val ip: LString = "",
    val device: LString = "",
    val kind: LString = "",
    @SerialName("device_name") val deviceName: LString = "",
) {
    /** The web's title: "Encore app · <device> · last used …" for apps, else the device name. */
    val title: String
        get() = if (kind == "app") {
            listOf("Encore app", deviceName, "last used " + formatAgo(if (lastSeenAt > 0) lastSeenAt else createdAt)).filter { it.isNotEmpty() }.joinToString(" · ")
        } else {
            deviceName.ifEmpty { device.ifEmpty { userAgent.ifEmpty { "Unknown device" } } }
        }
}

/** POST /api/auth/2fa/setup. */
@Serializable
data class TotpSetup(
    val secret: LString = "",
    @SerialName("otpauth_url") val otpauthUrl: LString = "",
    @SerialName("qr") val qrBase64: LString = "",
)

// ---- Discover (MusicBrainz) ------------------------------------------------------------------------------------

/** An artist from search (mb::search_artists) or a similar/For-you artist card (those send the id as `mbid`). */
@Serializable
data class CatalogArtist(
    val id: LString = "",
    val name: LString = "",
    val type: LString = "",
    val country: LString = "",
    val disambiguation: LString = "",
    @SerialName("in_library") val inLibrary: LBool = false,
    val image: LString = "",
    /** the id as similar / For-you cards send it; [id] falls back on it */
    val mbid: LString = "",
) : Checked<CatalogArtist> {
    override fun checked(): CatalogArtist? = when {
        name.isEmpty() -> null
        id.isEmpty() -> copy(id = mbid)
        else -> this
    }
}

/** A release group (search, discography) with the library annotations (`annotate_albums`). */
@Serializable
data class CatalogAlbum(
    val id: LString = "",
    val title: LString = "",
    val artist: LString = "",
    @SerialName("artist_id") val artistId: LString = "",
    val type: LString = "",
    val secondary: LTexts = emptyList(),
    val year: LYear = "",
    val date: LString = "",
    val cover: LString = "",
    @SerialName("in_library") val inLibrary: LBool = false,
    @SerialName("library_album_id") val libraryAlbumId: LString = "",
    @SerialName("request_status") val requestStatus: LString = "",
    /** Deezer / Apple ids of an album only they know (search results): requested without a MusicBrainz id. */
    val ext: LObject = null,
    /** The words the pages show for the request ("Requested", "Not found"…), when the server sent them. */
    @SerialName("request_label") val requestLabel: LString = "",
    /** The group the server files it under ("Album", "Live", "Compilation" …; 0.26), "" from older servers. */
    val kind: LString = "",
    /** Whether Settings → Release types wants it (0.26: `allowed`; older servers: no `skipped` reason). */
    @Transient val allowed: Boolean = true,
    /** Why the release types leave it out ("Live", "Single" …), or "". */
    val skipped: LString = "",
    /** `allowed` as sent (null: an older server, decided by [skipped]) */
    @SerialName("allowed") val allowedSent: LBoolOrNull = null,
) : Checked<CatalogAlbum> {
    override fun checked(): CatalogAlbum? =
        if (id.isEmpty() && title.isEmpty()) null else copy(allowed = allowedSent ?: skipped.isEmpty())
}

/** A recording from search (mb::search_recordings) with the library match. */
@Serializable
data class CatalogTrack(
    val id: LString = "",
    val title: LString = "",
    val artist: LString = "",
    @SerialName("artist_id") val artistId: LString = "",
    val album: LString = "",
    @SerialName("release_group_id") val releaseGroupId: LString = "",
    val year: LYear = "",
    @SerialName("length") val lengthSec: LDouble = 0.0,
    val cover: LString = "",
    @SerialName("in_library") val inLibrary: LBool = false,
    val library: LTrack = null,
    @SerialName("request_status") val requestStatus: LString = "",
    val isrc: LString = "",
    @SerialName("request_label") val requestLabel: LString = "",
) : Checked<CatalogTrack> {
    /** The key request statuses are kept under (songs only Deezer / Apple know have no MusicBrainz id). */
    val key: String get() = id.ifEmpty { "ext:$artist/$title" }

    override fun checked(): CatalogTrack? = if (title.isEmpty()) null else this
}

/** The Top result of a Discover search: one of [track], [album] or [artist], by [kind] (song | album | artist). */
data class CatalogTop(val kind: String, val track: CatalogTrack?, val album: CatalogAlbum?, val artist: CatalogArtist?) {
    /** `{kind, item}` of the search answer. */
    internal object Reader : KSerializer<CatalogTop?> {
        override val descriptor: SerialDescriptor = JsonObject.serializer().descriptor
        override fun deserialize(decoder: Decoder): CatalogTop? {
            val o = (decoder as? JsonDecoder)?.decodeJsonElement() as? JsonObject ?: return null
            val kind = o.str("kind")
            val item = o.obj("item") ?: return null
            val top = CatalogTop(
                kind,
                if (kind == "song") item.decodeModel(CatalogTrack.serializer()) else null,
                if (kind == "album") item.decodeModel(CatalogAlbum.serializer()) else null,
                if (kind == "artist") item.decodeModel(CatalogArtist.serializer()) else null,
            )
            return if (top.track == null && top.album == null && top.artist == null) null else top
        }
        override fun serialize(encoder: Encoder, value: CatalogTop?) = throw UnsupportedOperationException("read only")
    }
}

@Serializable
data class SearchResults(
    @Serializable(with = Items::class) val artists: List<CatalogArtist> = emptyList(),
    @Serializable(with = Items::class) val albums: List<CatalogAlbum> = emptyList(),
    @Serializable(with = Items::class) val tracks: List<CatalogTrack> = emptyList(),
    @Serializable(with = CatalogTop.Reader::class) val top: CatalogTop? = null,
) {
    val isEmpty: Boolean get() = artists.isEmpty() && albums.isEmpty() && tracks.isEmpty()
}

/** An album of this artist that is in the library (GET /api/discover/artist/{mbid} `library_albums[]`). */
@Serializable
data class LibraryAlbumRef(
    @SerialName("album_id") val albumId: LString = "",
    val album: LString = "",
    @SerialName("albumartist") val albumArtist: LString = "",
    val year: LYear = "",
    val rgid: LString = "",
) : Checked<LibraryAlbumRef> {
    override fun checked(): LibraryAlbumRef? = when {
        albumId.isEmpty() -> null
        album.isEmpty() -> copy(album = "Unknown album")
        else -> this
    }
}

/** One box of the release-type panel (`release_types.kinds[]` / `.skip[]` on the artist page). */
@Serializable
data class ReleaseTypeOption(val key: LString = "", val label: LString = "", val on: LBool = false) : Checked<ReleaseTypeOption> {
    override fun checked(): ReleaseTypeOption? = when {
        key.isEmpty() -> null
        label.isEmpty() -> copy(label = key)
        else -> this
    }
}

/** The artist page's `release_types`: the Request discography panel's boxes, ticked as Settings → Release types has them. */
@Serializable
data class ReleaseTypes(
    @Serializable(with = Items::class) val kinds: List<ReleaseTypeOption> = emptyList(),
    @Serializable(with = Items::class) val skip: List<ReleaseTypeOption> = emptyList(),
) {
    internal object OrNull : Nested<ReleaseTypes>(serializer())
}

/** The links the web shows on an artist page (discover.js `links`): (kind, url), at most six. */
internal object ArtistLinks : KSerializer<List<Pair<String, String>>> {
    private val KINDS = listOf("official homepage", "bandcamp", "soundcloud", "youtube", "streaming", "free streaming", "wikidata", "discogs", "last.fm")
    override val descriptor: SerialDescriptor = JsonObject.serializer().descriptor
    override fun deserialize(decoder: Decoder): List<Pair<String, String>> {
        val o = (decoder as? JsonDecoder)?.decodeJsonElement() as? JsonObject ?: return emptyList()
        val links = ArrayList<Pair<String, String>>()
        for ((k, v) in o) if (k in KINDS && v is JsonPrimitive && v !is JsonNull) links.add(k to v.content)
        return links.take(6)
    }
    override fun serialize(encoder: Encoder, value: List<Pair<String, String>>) = throw UnsupportedOperationException("read only")
}

/** GET /api/discover/artist/{mbid}. */
@Serializable
data class ArtistPage(
    val id: LString = "",
    val name: LString = "",
    val type: LString = "",
    val country: LString = "",
    val begin: LString = "",
    val disambiguation: LString = "",
    val tags: LTexts = emptyList(),
    @Serializable(with = ArtistLinks::class) val links: List<Pair<String, String>> = emptyList(),
    @SerialName("release_groups") @Serializable(with = Items::class) val releaseGroups: List<CatalogAlbum> = emptyList(),
    @SerialName("library_albums") @Serializable(with = Items::class) val libraryAlbums: List<LibraryAlbumRef> = emptyList(),
    @SerialName("library_tracks") val libraryTracks: LInt = 0,
    @SerialName("request_status") val requestStatus: LString = "",
    @SerialName("request_label") val requestLabel: LString = "",
    /** What "Request discography" includes: singles too (server setting), Deezer / Apple Music releases too. */
    @SerialName("discography_singles") val discographySingles: LBool = false,
    @SerialName("catalog_merge") val catalogMerge: LBool = false,
    @SerialName("release_types") @Serializable(with = ReleaseTypes.OrNull::class) val releaseTypes: ReleaseTypes? = null,
) {
    /** The Request discography panel's boxes (Settings → Release types, ticked as set): kinds, then extras to skip. */
    val releaseKinds: List<ReleaseTypeOption> get() = releaseTypes?.kinds ?: emptyList()
    val releaseSkip: List<ReleaseTypeOption> get() = releaseTypes?.skip ?: emptyList()
}

/** GET /api/discover/artist/{mbid}/extras `top[]` (Deezer's popular songs). */
@Serializable
data class TopTrack(
    val title: LString = "",
    val version: LString = "",
    val album: LString = "",
    val cover: LString = "",
    @SerialName("duration") val durationSec: LDouble = 0.0,
    val isrc: LString = "",
    val library: LTrack = null,
) : Checked<TopTrack> {
    override fun checked(): TopTrack? = if (title.isEmpty()) null else this
}

@Serializable
data class ArtistExtras(
    val image: LString = "",
    val bio: LString = "",
    @SerialName("bio_url") val bioUrl: LString = "",
    @Serializable(with = Items::class) val top: List<TopTrack> = emptyList(),
)

@Serializable
data class SimilarArtists(val source: LString = "", @Serializable(with = Items::class) val artists: List<CatalogArtist> = emptyList())

/** Where an outside release is listed: (source name, url) of its `sources[]`, those with a name. */
internal object SourceLinks : KSerializer<List<Pair<String, String>>> {
    override val descriptor: SerialDescriptor = JsonArray.serializer().descriptor
    override fun deserialize(decoder: Decoder): List<Pair<String, String>> {
        val a = (decoder as? JsonDecoder)?.decodeJsonElement() as? JsonArray
        return a.objectsOf().map { it.str("name") to it.str("url") }.filter { it.first.isNotEmpty() }
    }
    override fun serialize(encoder: Encoder, value: List<Pair<String, String>>) = throw UnsupportedOperationException("read only")
}

/** A release Deezer / Apple Music / Bandcamp / Discogs know that MusicBrainz doesn't (GET …/catalog `releases[]`). */
@Serializable
data class ExtRelease(
    val title: LString = "",
    val year: LYear = "",
    val date: LString = "",
    val type: LString = "",
    val tracks: LInt = 0,
    val cover: LString = "",
    @SerialName("in_library") val inLibrary: LBool = false,
    @SerialName("library_album_id") val libraryAlbumId: LString = "",
    @SerialName("request_status") val requestStatus: LString = "",
    @Serializable(with = SourceLinks::class) val sources: List<Pair<String, String>> = emptyList(),
    val harmony: LString = "",
    val ext: JsonElement = JsonNull,
    @SerialName("request_label") val requestLabel: LString = "",
    /** The server's "ext-…" id (Deezer / Apple ids), the one a discography request names it by; "" from older servers. */
    val id: LString = "",
    /** The group the server files it under ("Album", "Compilation", "Live" …; 0.26), "" from older servers. */
    val kind: LString = "",
    /** Whether Settings → Release types wants it (0.26: `allowed`; older servers: no `skipped` reason). */
    @Transient val allowed: Boolean = true,
    /** Why the release types leave it out ("Live", "Single" …), or "". */
    val skipped: LString = "",
    /** `allowed` as sent (null: an older server, decided by [skipped]) */
    @SerialName("allowed") val allowedSent: LBoolOrNull = null,
) : Checked<ExtRelease> {
    override fun checked(): ExtRelease? = if (title.isEmpty()) null else copy(allowed = allowedSent ?: skipped.isEmpty())
}

/** A track of an external release (POST /api/discover/catalog/tracks), with its library copy (playable) and any request. */
@Serializable
data class ExtTrack(
    val track: LInt = 0,
    val title: LString = "",
    val artist: LString = "",
    @SerialName("length") val lengthSec: LDouble = 0.0,
    val isrc: LString = "",
    @SerialName("in_library") val inLibrary: LBool = false,
    val library: LTrack = null,
    @SerialName("request_status") val requestStatus: LString = "",
    @SerialName("request_label") val requestLabel: LString = "",
) : Checked<ExtTrack> {
    override fun checked(): ExtTrack? = if (title.isEmpty()) null else this
}

/** One track of a MusicBrainz release (GET /api/discover/album/{rgid} `release.tracks[]`). */
@Serializable
data class ReleaseTrack(
    val disc: LInt = 0,
    val track: LInt = 0,
    val title: LString = "",
    val artist: LString = "",
    @SerialName("length") val lengthSec: LDouble = 0.0,
    @SerialName("recording_id") val recordingId: LString = "",
    @SerialName("in_library") val inLibrary: LBool = false,
    val library: LTrack = null,
    val isrc: LString = "",
    @SerialName("request_status") val requestStatus: LString = "",
    @SerialName("request_label") val requestLabel: LString = "",
) : Checked<ReleaseTrack> {
    override fun checked(): ReleaseTrack? = if (title.isEmpty()) null else this
}

@Serializable
data class Release(
    @Serializable(with = Items::class) val tracks: List<ReleaseTrack> = emptyList(),
    @SerialName("disc_count") val discCount: LInt = 0,
    val label: LString = "",
) {
    internal object OrNull : Nested<Release>(serializer())
}

/** GET /api/discover/album/{rgid}: the release group plus the best release's track list. */
@Serializable
data class AlbumPage(
    val id: LString = "",
    val title: LString = "",
    val artist: LString = "",
    @SerialName("artist_id") val artistId: LString = "",
    val type: LString = "",
    val secondary: LTexts = emptyList(),
    val year: LYear = "",
    val cover: LString = "",
    val tags: LTexts = emptyList(),
    @SerialName("in_library") val inLibrary: LBool = false,
    @SerialName("library_album_id") val libraryAlbumId: LString = "",
    @SerialName("request_status") val requestStatus: LString = "",
    @Serializable(with = Release.OrNull::class) val release: Release? = null,
    @SerialName("request_label") val requestLabel: LString = "",
    /** Songs of the album in the library (Play / Shuffle show when there is any). */
    @SerialName("owned_tracks") val ownedTracks: LInt = 0,
)

// ---- For you ---------------------------------------------------------------------------------------------------

/** A recommended song (annotate_tracks): the library match when there is one, else a request status. */
@Serializable
data class FyTrack(
    val mbid: LString = "",
    val title: LString = "",
    val artist: LString = "",
    @SerialName("artist_mbid") val artistMbid: LString = "",
    val album: LString = "",
    val cover: LString = "",
    @SerialName("length") val lengthSec: LDouble = 0.0,
    val isrc: LString = "",
    val library: LTrack = null,
    @SerialName("request_status") val requestStatus: LString = "",
) : Checked<FyTrack> {
    override fun checked(): FyTrack? = if (title.isEmpty()) null else this

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

@Serializable
data class FyPlaylist(
    val id: LString = "",
    val title: LString = "",
    val kind: LString = "",
    @Serializable(with = Items::class) val tracks: List<FyTrack> = emptyList(),
)

/** A Last.fm album pick ("Albums for you"). */
@Serializable
data class FyAlbum(
    val title: LString = "",
    val artist: LString = "",
    val cover: LString = "",
    val year: LYear = "",
    @SerialName("in_library") val inLibrary: LBool = false,
    @SerialName("request_status") val requestStatus: LString = "",
) : Checked<FyAlbum> {
    override fun checked(): FyAlbum? = if (title.isEmpty()) null else this
}

@Serializable
data class LastFm(
    val user: LString = "",
    @Serializable(with = Items::class) val tracks: List<FyTrack> = emptyList(),
    @Serializable(with = Items::class) val artists: List<CatalogArtist> = emptyList(),
    @Serializable(with = Items::class) val albums: List<FyAlbum> = emptyList(),
) {
    internal object OrNull : Nested<LastFm>(serializer())
}

/** GET /api/discover/for-you. */
@Serializable
data class ForYou(
    val connected: LBool = false,
    val user: LString = "",
    @Serializable(with = Items::class) val playlists: List<FyPlaylist> = emptyList(),
    @Serializable(with = LastFm.OrNull::class) val lastfm: LastFm? = null,
    val errors: LTexts = emptyList(),
)

@Serializable
data class Tags(val library: LTexts = emptyList(), val top: LTexts = emptyList())

@Serializable
data class TagPage(val source: LString = "", val summary: LString = "", @Serializable(with = Items::class) val artists: List<CatalogArtist> = emptyList())

@Serializable
data class SimilarTracks(val source: LString = "", @Serializable(with = Items::class) val tracks: List<FyTrack> = emptyList())

// ---- Requests --------------------------------------------------------------------------------------------------

/** One row of GET /api/requests. */
@Serializable
data class RequestRow(
    val id: LLong = 0L,
    val kind: LString = "",
    val mbid: LString = "",
    val artist: LString = "",
    val title: LString = "",
    val album: LString = "",
    val url: LString = "",
    val status: LString = "",
    val message: LString = "",
    val source: LString = "",
    val progress: LInt = 0,
    @SerialName("cover_url") val coverUrl: LString = "",
    @SerialName("created_at") val createdAt: LDouble = 0.0,
    val username: LString = "",
    @SerialName("parent_id") val parentId: LLong = 0L,
    @SerialName("length") val lengthSec: LDouble = 0.0,
    /** How many times the song was requested or retried on this row (1 for a first request). */
    val attempts: LInt = 1,
) : Checked<RequestRow> {
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

    override fun checked(): RequestRow? = if (id <= 0L) null else if (attempts < 1) copy(attempts = 1) else this
}

// ---- Picker ----------------------------------------------------------------------------------------------------

/**
 * One source of GET /api/picker/sources. `state`: ready | sign_in | not_connected | not_installed | off; `hint`
 * says why when not ready, `settings` is the web UI's settings link (`#/settings?s=…`) with its label.
 */
@Serializable
data class PickerSource(
    val id: LString = "",
    val name: LString = "",
    val state: LString = "ready",
    val hint: LString = "",
    val settings: LString = "",
    @SerialName("settings_label") val settingsLabel: LString = "",
) : Checked<PickerSource> {
    /** Only a ready source is searched; the others show their hint under their tab. */
    val ready: Boolean get() = state.isEmpty() || state == "ready"

    override fun checked(): PickerSource? = when {
        id.isEmpty() -> null
        state.isEmpty() -> copy(state = "ready")
        else -> this
    }
}

@Serializable
data class PickerInfo(val allowed: LBool = false, @Serializable(with = Items::class) val sources: List<PickerSource> = emptyList())

/** One result of POST /api/picker/search. */
@Serializable
data class PickResult(
    val source: LString = "",
    val title: LString = "",
    val subtitle: LString = "",
    val quality: LString = "",
    val lossless: LBool = false,
    val tracks: LInt = 0,
    val size: LLong = 0L,
    @SerialName("duration") val durationSec: LDouble = 0.0,
    val info: LString = "",
    val warning: LString = "",
    val cover: LString = "",
    val payload: JsonElement = JsonObject(emptyMap()),
) : Checked<PickResult> {
    override fun checked(): PickResult? = if (title.isEmpty()) null else this
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
@Serializable
data class PlaylistItem(
    val id: LLong = 0L,
    val title: LString = "",
    val artist: LString = "",
    val album: LString = "",
    @SerialName("album_id") val albumId: LString = "",
    @SerialName("track_id") val trackId: LLong = 0L,
    @SerialName("request_id") val requestId: LLong = 0L,
    @SerialName("request_status") val requestStatus: LString = "",
    @SerialName("verify_status") val verifyStatus: LString = "",
    @SerialName("match_reason") val matchReason: LString = "",
    /** the matched library song (the row itself carries its fields, see [Rows]) */
    @Transient val track: Track? = null,
) {
    /** The rows: each is a playlist item and, when matched, the library song (`track_id`) at once. */
    internal object Rows : KSerializer<List<PlaylistItem>> {
        override val descriptor: SerialDescriptor = JsonArray.serializer().descriptor
        override fun deserialize(decoder: Decoder): List<PlaylistItem> {
            val a = (decoder as? JsonDecoder)?.decodeJsonElement() as? JsonArray
            return a.objectsOf().mapNotNull { o ->
                o.decodeModel(serializer())?.takeIf { it.id > 0L }?.copy(track = Track.fromPlaylistItem(o))
            }
        }
        override fun serialize(encoder: Encoder, value: List<PlaylistItem>) = throw UnsupportedOperationException("read only")
    }
}

@Serializable
data class PlaylistPage(
    val id: LLong = 0L,
    val name: LString = "",
    val kind: LString = "",
    val source: LString = "",
    @SerialName("source_url") val sourceUrl: LString = "",
    val follow: LBool = false,
    val total: LInt = 0,
    val matched: LInt = 0,
    @SerialName("cover_url") val coverUrl: LString = "",
    @Serializable(with = PlaylistItem.Rows::class) val items: List<PlaylistItem> = emptyList(),
) {
    val isLiked: Boolean get() = kind == "liked"
}

/** POST /api/playlists/preview. */
@Serializable
data class ImportPreview(
    val name: LString = "",
    val source: LString = "",
    val count: LInt = 0,
    @SerialName("in_library") val inLibrary: LInt = 0,
    val duplicates: LInt = 0,
    val warning: LString = "",
    @Serializable(with = Items::class) val tracks: List<ImportTrack> = emptyList(),
)

@Serializable
data class ImportTrack(
    val artist: LString = "",
    val title: LString = "",
    val album: LString = "",
    val isrc: LString = "",
    @SerialName("track_id") val trackId: LLong = 0L,
    @SerialName("match_reason") val matchReason: LString = "",
) {
    val hasIsrc: Boolean get() = isrc.isNotEmpty()
    val inLibrary: Boolean get() = trackId > 0
}

/** POST /api/playlists/import. */
@Serializable
data class ImportResult(
    val id: LLong = 0L,
    val added: LInt = 0,
    @SerialName("duplicates_skipped") val duplicatesSkipped: LInt = 0,
    val requested: LInt = 0,
)

/** A song "Download missing" couldn't request, and why. */
@Serializable
data class MissingFailure(val title: LString = "", val reason: LString = "")

/** POST /api/playlists/{id}/request-missing: what "Download missing" did, song by song. */
@Serializable
data class MissingReport(
    val requested: LInt = 0,
    @SerialName("in_progress") val inProgress: LInt = 0,
    @Serializable(with = Items::class) val failed: List<MissingFailure> = emptyList(),
) {
    /** The toast, the web's words: "Requested 120 · 600 already in progress · 64 couldn't be requested (why)". */
    fun toastText(): String {
        val sb = StringBuilder("Requested $requested")
        if (inProgress > 0) sb.append(" · $inProgress already in progress")
        if (failed.isNotEmpty()) {
            val reasons = failed.map { it.reason }.filter { it.isNotEmpty() }.distinct()
            val why = reasons.take(3).joinToString("; ") + (if (reasons.size > 3) "; ${reasons.size - 3} more" else "")
            sb.append(" · ${failed.size} couldn't be requested")
            if (why.isNotEmpty()) sb.append(" ($why)")
        }
        return sb.toString()
    }
}

/** web/js/pages/playlists.js `SRC`: where a playlist came from. */
val PLAYLIST_SOURCES: Map<String, String> = mapOf(
    "spotify" to "Spotify", "ytmusic" to "YouTube Music", "deezer" to "Deezer", "tidal" to "Tidal", "qobuz" to "Qobuz", "apple" to "Apple Music", "amazon" to "Amazon Music",
    "ytdlp" to "YouTube / SoundCloud", "listenbrainz" to "ListenBrainz", "web" to "Web", "text" to "Pasted list", "merged" to "Merged",
    "manual" to "Encore", "library" to "Encore", "jellyfin" to "Jellyfin", "plex" to "Plex", "navidrome" to "Navidrome",
)

// ---- Leaderboard -----------------------------------------------------------------------------------------------

@Serializable
data class BoardRow(val username: LString = "", val value: LDouble = 0.0, val me: LBool = false)

/** One board of `boards` (an object by key): its label and rows. */
data class Board(val key: String, val label: String, val rows: List<BoardRow>) {
    /** `boards`: `{key: {label, rows}}` in the server's order. */
    internal object All : KSerializer<List<Board>> {
        override val descriptor: SerialDescriptor = JsonObject.serializer().descriptor
        override fun deserialize(decoder: Decoder): List<Board> {
            val o = (decoder as? JsonDecoder)?.decodeJsonElement() as? JsonObject ?: return emptyList()
            return o.mapNotNull { (k, v) ->
                if (v !is JsonObject) null else Board(k, v.str("label"), v.arr("rows").objectsOf().rows(BoardRow.serializer()))
            }
        }
        override fun serialize(encoder: Encoder, value: List<Board>) = throw UnsupportedOperationException("read only")
    }
}

@Serializable
data class TopSong(val title: LString = "", val artist: LString = "", @SerialName("album_id") val albumId: LString = "", val plays: LLong = 0L)

@Serializable
data class TopArtist(val artist: LString = "", val plays: LLong = 0L, val listeners: LInt = 0)

@Serializable
data class LibraryArtistStat(val artist: LString = "", val albums: LInt = 0, val songs: LLong = 0L)

@Serializable
data class Fact(val label: LString = "", val value: LString = "")

/** A row of `this_week`: who played how many songs. */
@Serializable
data class WeekRow(val username: LString = "", val plays: LLong = 0L)

/** GET /api/stats/leaderboard. */
@Serializable
data class Leaderboard(
    @Serializable(with = Board.All::class) val boards: List<Board> = emptyList(),
    @SerialName("top_songs") @Serializable(with = Items::class) val topSongs: List<TopSong> = emptyList(),
    @SerialName("top_artists") @Serializable(with = Items::class) val topArtists: List<TopArtist> = emptyList(),
    @SerialName("this_week") @Serializable(with = Items::class) val weekRows: List<WeekRow> = emptyList(),
    @SerialName("library_artists") @Serializable(with = Items::class) val libraryArtists: List<LibraryArtistStat> = emptyList(),
    @Serializable(with = Items::class) val facts: List<Fact> = emptyList(),
) {
    /** (username, songs played this week), most first. */
    val thisWeek: List<Pair<String, Long>> get() = weekRows.map { it.username to it.plays }
}
