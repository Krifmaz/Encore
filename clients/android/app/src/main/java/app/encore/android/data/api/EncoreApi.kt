package app.encore.android.data.api

import app.encore.android.core.EncoreJson
import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.int
import app.encore.android.core.long
import app.encore.android.core.obj
import app.encore.android.core.objects
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.session.AuthKind
import app.encore.android.data.session.Session
import app.encore.android.data.session.SessionStore
import app.encore.android.features.library.PlaylistLinkRules
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException
import java.net.URLEncoder

/** An HTTP error from the server, with its `detail` message and the JSON body (e.g. `{two_factor: true}`). */
class ApiException(val code: Int, message: String, val body: JsonObject?) : IOException(message)

sealed interface LoginResult {
    data class Success(val session: Session, val user: User) : LoginResult
    /** The account has 2FA: ask for the authenticator code and send it again. */
    data class NeedsCode(val message: String) : LoginResult
    data class Failed(val message: String) : LoginResult
}

/**
 * The Encore REST API (server/src/routes). Every call is blocking: callers run it on Dispatchers.IO or on the
 * playback service's executor. Responses are parsed leniently (see Models.kt).
 */
class EncoreApi(private val client: OkHttpClient, private val store: SessionStore) {
    val json: Json = EncoreJson
    private val jsonType = "application/json; charset=utf-8".toMediaType()

    /** The signed-in server ("" when signed out). */
    val base: String get() = store.current?.server ?: ""

    fun url(path: String): String = base + path

    /**
     * An album cover at a thumbnail size (the server snaps to 96, 200, 400, 600, 1000: list rows 96/200, grids 400,
     * the full player 1000). With the album's known `cover_v` the URL is versioned and cached for good.
     */
    fun coverUrl(albumId: String, size: Int = 600): String {
        val v = CoverVersions.of(albumId)
        return url("/api/library/albums/${enc(albumId)}/cover?s=$size" + if (v > 0L) "&v=$v" else "")
    }

    /** Told about each page of albums a list loads, so their covers can be fetched ahead of the scroll. */
    @Volatile var onAlbumPage: ((List<String>) -> Unit)? = null

    /** The stream of a song (PLAN §6.2): the original file when Android decodes it, FLAC made by the server otherwise. */
    fun streamUrl(trackId: Long, format: String): String =
        url("/api/library/tracks/$trackId/stream" + if (needsTranscode(format)) "?transcode=true" else "")

    // ---- server picker / sign-in ------------------------------------------------------------------------

    /** Finds the server behind what the user typed: https first, then http; checks /api/health. */
    fun probe(input: String): String {
        val raw = input.trim().trimEnd('/')
        if (raw.isEmpty()) throw IOException("Enter your server's address")
        val candidates = if (raw.startsWith("http://") || raw.startsWith("https://")) listOf(raw) else listOf("https://$raw", "http://$raw")
        var last: Exception? = null
        for (c in candidates) {
            // OkHttp's form (lower-case host, no default port) so AuthInterceptor matches later requests
            val norm = c.toHttpUrlOrNull()?.toString()?.trimEnd('/')
            if (norm == null) {
                last = IOException("That isn't a valid address")
                continue
            }
            try {
                val o = call("GET", norm + "/api/health", null) as? JsonObject
                if (o != null && (o.bool("ok") || o.str("version").isNotEmpty())) return norm
                last = IOException("That isn't an Encore server")
            } catch (e: Exception) {
                last = e
            }
        }
        throw IOException(last?.message ?: "Can't reach that server")
    }

    fun info(server: String): AuthInfo = AuthInfo.from(call("GET", "$server/api/auth/info", null) as? JsonObject ?: JsonObject(emptyMap()))

    /**
     * Signs in. With the server's `app_tokens` feature the body carries `app: {device_id, device_name}` and the
     * reply is `{user, token}` (a long-lived bearer token); otherwise the web session cookie is kept.
     */
    fun login(server: String, info: AuthInfo, username: String, password: String, code: String?, deviceId: String, deviceName: String): LoginResult {
        val body = LoginBody(
            username = username.trim(),
            password = password,
            code = code?.replace(" ", "")?.takeIf { it.isNotEmpty() },
            app = if (info.appTokens) AppDevice(deviceId, deviceName) else null,
        )
        val req = Request.Builder()
            .url("$server/api/auth/login")
            .post(json.encodeToString(LoginBody.serializer(), body).toRequestBody(jsonType))
            .header("Accept", "application/json")
            .build()
        try {
            client.newCall(req).execute().use { res ->
                val text = res.body.string()
                val parsed = parseObject(text)
                if (!res.isSuccessful) {
                    val detail = parsed?.str("detail")?.ifEmpty { null } ?: "Sign-in failed (HTTP ${res.code})"
                    return if (res.code == 401 && parsed?.bool("two_factor") == true) LoginResult.NeedsCode(detail) else LoginResult.Failed(detail)
                }
                val token = parsed?.str("token") ?: ""
                if (token.isNotEmpty()) {
                    val user = User.from(parsed?.obj("user") ?: JsonObject(emptyMap()))
                    return LoginResult.Success(Session(server, token, AuthKind.BEARER, user.username.ifEmpty { username.trim() }), user)
                }
                // cookie session: the reply is the user, the token is in Set-Cookie
                val cookie = res.headers("Set-Cookie").firstOrNull { it.trim().startsWith("encore_session=") }
                    ?.trim()?.removePrefix("encore_session=")?.substringBefore(';')?.trim() ?: ""
                if (cookie.isEmpty()) return LoginResult.Failed("The server didn't start a session")
                val user = User.from(parsed ?: JsonObject(emptyMap()))
                return LoginResult.Success(Session(server, cookie, AuthKind.COOKIE, user.username.ifEmpty { username.trim() }), user)
            }
        } catch (e: IOException) {
            return LoginResult.Failed(e.message ?: "Can't reach the server")
        }
    }

    fun logout() {
        try { post("/api/auth/logout", JsonObject(emptyMap())) } catch (_: Exception) {}
    }

    fun me(): User = User.from(getObject("/api/auth/me"))

    /** GET /api/auth/me as it comes (perms, prefs.playback …). */
    fun meRaw(): JsonObject = getObject("/api/auth/me")

    /** PUT /api/auth/prefs {playback}: the account keeps the playback settings (web/js/playback.js save()). */
    fun putPlaybackPrefs(playback: JsonObject) {
        call("PUT", url("/api/auth/prefs"), JsonObject(mapOf("playback" to playback)))
    }

    // ---- playback (server-side Cadenza, signal path, lyrics, share) --------------------------------------

    /** GET /api/playback/dsp: whether the server upsamples streams for this account, and the choices on offer. */
    fun dspInfo(): app.encore.android.data.prefs.DspInfo = app.encore.android.data.prefs.DspInfo.from(getObject("/api/playback/dsp"))

    /** POST /api/playback/dsp/prepare: render a song ahead; returns `cached | rendering | queued | busy` ("" if unknown). */
    fun dspPrepare(trackId: Long, rate: Int, filter: String, dither: String): String {
        val body = json.encodeToJsonElement(DspPrepareBody.serializer(), DspPrepareBody(trackId, rate, filter, dither))
        return (post("/api/playback/dsp/prepare", body) as? JsonObject)?.str("state") ?: ""
    }

    /** GET /api/library/tracks/{id}/info: codec, sample rate, bit depth, channels, bitrate, lossless (the signal path's Source row). */
    fun trackInfo(trackId: Long): JsonObject = getObject("/api/library/tracks/$trackId/info")

    /** GET /api/library/tracks/{id}/lyrics: `{lyrics, synced, source, instrumental?}` (looked up on LRCLIB and saved when missing). */
    fun lyrics(trackId: Long, force: Boolean = false): JsonObject = getObject("/api/library/tracks/$trackId/lyrics" + if (force) "?force=true" else "")

    /** POST /api/library/tracks/{id}/share → the public link (`base + path`, as the web player builds it). */
    fun shareLink(trackId: Long): String {
        val o = post("/api/library/tracks/$trackId/share", JsonObject(emptyMap())) as? JsonObject ?: throw IOException("Couldn't create the link")
        val path = o.str("path")
        return if (path.isNotEmpty()) url(path) else o.str("url")
    }

    /** POST /api/library/albums/{album_id}/share → the album's public link (same reply as a song: {token, path, url}). */
    fun shareAlbumLink(albumId: String): String {
        val o = post("/api/library/albums/${enc(albumId)}/share", JsonObject(emptyMap())) as? JsonObject ?: throw IOException("Couldn't create the link")
        val path = o.str("path")
        return if (path.isNotEmpty()) url(path) else o.str("url")
    }

    /** POST /api/library/artists/share {name} → the artist's public link (same reply as a song: {token, path, url}). */
    fun shareArtistLink(name: String): String {
        val body = JsonObject(mapOf("name" to kotlinx.serialization.json.JsonPrimitive(name)))
        val o = post("/api/library/artists/share", body) as? JsonObject ?: throw IOException("Couldn't create the link")
        val path = o.str("path")
        return if (path.isNotEmpty()) url(path) else o.str("url")
    }

    /**
     * POST /api/playlists/{id}/share → the playlist's public link (same reply as a song: {token, path, url}). Liked
     * Songs is a playlist too; asking again gives the same link.
     */
    fun sharePlaylistLink(id: Long): String {
        val o = post("/api/playlists/$id/share", JsonObject(emptyMap())) as? JsonObject ?: throw IOException("Couldn't create the link")
        val path = o.str("path")
        return if (path.isNotEmpty()) url(path) else o.str("url")
    }

    // ---- library ---------------------------------------------------------------------------------------

    /** [dir] "asc" / "desc", or "" for the sort's own default (as GET /api/library/index takes it). */
    fun albums(sort: String = "name", offset: Int = 0, limit: Int = 120, q: String = "", artist: String = "", dir: String = ""): List<Album> {
        val qs = StringBuilder("/api/library/albums?sort=").append(enc(sort)).append("&limit=").append(limit).append("&offset=").append(offset)
        if (dir.isNotEmpty()) qs.append("&dir=").append(enc(dir))
        if (q.isNotEmpty()) qs.append("&q=").append(enc(q))
        if (artist.isNotEmpty()) qs.append("&artist=").append(enc(artist))
        val page = get(qs.toString()).objects().mapNotNull { Album.from(it) }
        if (offset > 0 || page.size > 24) onAlbumPage?.invoke(page.map { it.id })
        return page
    }

    fun album(id: String): AlbumDetail {
        val o = getObject("/api/library/albums/${enc(id)}")
        val tracks = o.arr("tracks").objectsOf().mapNotNull { Track.from(it) }
        val album = Album(
            id = id,
            title = o.str("album").ifEmpty { "Unknown album" },
            artist = o.str("albumartist"),
            year = o.str("year").take(4),
            trackCount = tracks.size,
            formats = tracks.map { it.format }.distinct().joinToString(","),
        )
        return AlbumDetail(album, tracks, o.dbl("duration"))
    }

    /** sort: name | added | albums | tracks; [dir] "" for the sort's own default. */
    fun artists(q: String = "", sort: String = "name", dir: String = ""): List<Artist> =
        get("/api/library/artists?sort=" + enc(sort) + (if (dir.isNotEmpty()) "&dir=${enc(dir)}" else "") + if (q.isNotEmpty()) "&q=${enc(q)}" else "")
            .objects().mapNotNull { Artist.from(it) }

    /** The Search tab: songs, albums and artists for a query, with a Top result (GET /api/library/search). */
    fun search(q: String, limit: Int = 30): LibrarySearch = LibrarySearch.from(getObject("/api/library/search?q=${enc(q)}&limit=$limit"))

    fun tracks(q: String = "", offset: Int = 0, limit: Int = 200, sort: String = "artist", dir: String = ""): List<Track> {
        val qs = StringBuilder("/api/library/tracks?sort=").append(enc(sort)).append("&limit=").append(limit).append("&offset=").append(offset)
        if (dir.isNotEmpty()) qs.append("&dir=").append(enc(dir))
        if (q.isNotEmpty()) qs.append("&q=").append(enc(q))
        return get(qs.toString()).objects().mapNotNull { Track.from(it) }
    }

    fun track(id: Long): Track? = Track.from(getObject("/api/library/tracks/$id"))

    /** GET /api/library/artist-tracks?name=: every song of a library artist (merged spellings, aliases, collaborations), max 5000. */
    fun artistTracks(name: String): List<Track> =
        get("/api/library/artist-tracks?name=" + enc(name)).objects().mapNotNull { Track.from(it) }

    /**
     * GET /api/library/artists/{name}/liked (0.31): the songs of the artist the listener liked, newest like first.
     * [mbid] (optional) lets the server also count songs credited to that MusicBrainz id. Older servers answer 404.
     */
    fun artistLiked(name: String, mbid: String = "", limit: Int = 200): ArtistSongs =
        ArtistSongs.from(get(artistRoute(name, "liked") + "&limit=$limit" + if (mbid.isNotEmpty()) "&mbid=${enc(mbid)}" else ""))

    /**
     * GET /api/library/artists/{name}/tracks (0.31): a page of the artist's songs in the library (album year, then
     * disc and track) with the total. Older servers answer 404.
     */
    fun artistSongs(name: String, mbid: String = "", offset: Int = 0, limit: Int = 50): ArtistSongs =
        ArtistSongs.from(get(artistRoute(name, "tracks") + "&offset=$offset&limit=$limit" + if (mbid.isNotEmpty()) "&mbid=${enc(mbid)}" else ""))

    /**
     * `/api/library/artists/{name}/<tail>?name=<name>` (client-shapes §3.2): the name always goes in `name=` too, which
     * the server prefers ([artistSegment] for the path).
     */
    private fun artistRoute(name: String, tail: String): String = artistPath(name, tail)

    /** GET /api/library/tracks?ids=1,2,3: these songs in this order (unknown ids left out), 300 per call. */
    fun tracksByIds(ids: List<Long>): List<Track> {
        val out = ArrayList<Track>(ids.size)
        for (chunk in ids.distinct().chunked(300)) {
            out.addAll(get("/api/library/tracks?ids=" + chunk.joinToString(",")).objects().mapNotNull { Track.from(it) })
        }
        return out
    }

    fun recentAlbums(limit: Int = 30): List<Album> = get("/api/library/recent?limit=$limit").objects().mapNotNull { Album.from(it) }

    fun recentPlays(limit: Int = 50): List<Track> = get("/api/library/plays/recent?limit=$limit").objects().mapNotNull { Track.from(it) }

    fun shuffle(limit: Int, exclude: List<Long>): List<Track> {
        val body = json.encodeToJsonElement(ShuffleBody.serializer(), ShuffleBody(limit, exclude.takeLast(5000)))
        return post("/api/library/shuffle", body).objects().mapNotNull { Track.from(it) }
    }

    /** Made for you (ListenBrainz playlists): raw rows, `tracks[].library` is the library match. */
    fun madeForYou(): List<JsonObject> = getObject("/api/discover/made-for-you").arr("playlists").objectsOf()

    // ---- play log (same rules and endpoints as web/js/player.js) -----------------------------------------

    fun nowPlaying(trackId: Long) {
        post("/api/library/now-playing", json.encodeToJsonElement(PlayBody.serializer(), PlayBody(trackId)))
    }

    fun logPlay(trackId: Long, seconds: Double) {
        post("/api/library/plays", json.encodeToJsonElement(PlayBody.serializer(), PlayBody(trackId, seconds)))
    }

    // ---- likes and playlists ---------------------------------------------------------------------------

    fun likes(): Set<Long> {
        val out = HashSet<Long>()
        val arr = get("/api/likes")
        if (arr is kotlinx.serialization.json.JsonArray) {
            for (e in arr) {
                val v = (e as? kotlinx.serialization.json.JsonPrimitive)?.content?.toDoubleOrNull()
                if (v != null) out.add(v.toLong())
            }
        }
        return out
    }

    fun like(trackId: Long) { post("/api/likes/$trackId", JsonObject(emptyMap())) }

    fun unlike(trackId: Long) { call("DELETE", url("/api/likes/$trackId"), null) }

    fun playlists(): List<Playlist> = get("/api/playlists").objects().mapNotNull { Playlist.from(it)?.withFullCover() }

    fun playlist(id: Long): PlaylistDetail {
        val o = getObject("/api/playlists/$id")
        val pl = (Playlist.from(o) ?: Playlist(id, o.str("name"), o.str("kind"), 0, 0, "")).withFullCover()
        val rows = (o.arr("items") ?: o.arr("tracks")).objectsOf().map { r ->
            PlaylistRow(r.str("title"), r.str("artist"), Track.fromPlaylistItem(r), r.str("request_status"))
        }
        return PlaylistDetail(pl, rows)
    }

    /**
     * An added playlist's picture is the link's public mosaic, a server path ("/api/share/<token>/cover"): made a full
     * URL, so the tiles, the page and Android Auto load it like any other cover.
     */
    private fun Playlist.withFullCover(): Playlist {
        val full = PlaylistLinkRules.absoluteCover(coverUrl) { url(it) }
        return if (full == coverUrl) this else copy(coverUrl = full)
    }

    // ---- pick up where you left off (server/src/routes/remote.rs) -----------------------------------------

    /** GET /api/remote/spot: the account's last playback spot with its song, or null (none, or an older server). */
    fun spot(): PlaybackSpot? = (get("/api/remote/spot") as? JsonObject)?.let { PlaybackSpot.from(it) }

    /** DELETE /api/remote/spot */
    fun clearSpot() { call("DELETE", url("/api/remote/spot"), null) }

    /** POST /api/remote/{device}/command, e.g. `{action: "pause"}` (the web's remote commands). */
    fun remoteCommand(deviceId: String, action: String) {
        post("/api/remote/${enc(deviceId)}/command", JsonObject(mapOf("action" to kotlinx.serialization.json.JsonPrimitive(action))))
    }

    // ---- requests --------------------------------------------------------------------------------------

    /** "Share to Encore": request whatever a link points at (POST /api/requests/link). Returns a short status line. */
    fun requestLink(link: String): String {
        val o = post("/api/requests/link", json.encodeToJsonElement(LinkBody.serializer(), LinkBody(link))) as? JsonObject
            ?: return "Requested"
        val n = o.int("requested")
        val already = o.int("already_requested")
        return when {
            n > 0 -> if (n == 1) "Requested in Encore" else "Requested $n songs in Encore"
            already > 0 -> "Already requested"
            else -> "Nothing to request in that link"
        }
    }

    // ---- plumbing --------------------------------------------------------------------------------------

    fun get(path: String): JsonElement? = call("GET", url(path), null)

    fun getObject(path: String): JsonObject = get(path) as? JsonObject ?: JsonObject(emptyMap())

    fun post(path: String, body: JsonElement): JsonElement? = call("POST", url(path), body)

    /** Raw GET of a server path (covers): the caller closes the response. */
    fun rawGet(path: String): okhttp3.Response {
        if (base.isEmpty()) throw IOException("Not signed in")
        return client.newCall(Request.Builder().url(url(path)).build()).execute()
    }

    private fun call(method: String, fullUrl: String, body: JsonElement?): JsonElement? {
        if (!fullUrl.startsWith("http")) throw IOException("Not signed in")
        val rb: RequestBody? = when {
            body != null -> json.encodeToString(JsonElement.serializer(), body).toRequestBody(jsonType)
            method == "POST" || method == "PUT" || method == "DELETE" -> "".toRequestBody(jsonType)
            else -> null
        }
        val req = Request.Builder().url(fullUrl).method(method, rb).header("Accept", "application/json").build()
        client.newCall(req).execute().use { res ->
            val text = res.body.string()
            if (!res.isSuccessful) {
                val o = parseObject(text)
                val detail = o?.str("detail")?.ifEmpty { null } ?: "HTTP ${res.code}"
                throw ApiException(res.code, detail, o)
            }
            if (text.isBlank()) return null
            val el = try { json.parseToJsonElement(text) } catch (e: Exception) { throw IOException("Unexpected reply from the server") }
            CoverVersions.note(el)
            return el
        }
    }

    private fun parseObject(text: String): JsonObject? =
        try { json.parseToJsonElement(text) as? JsonObject } catch (_: Exception) { null }

    companion object {
        /**
         * Formats the server sends as they are (server/src/routes/library.rs `browser_type`) and ExoPlayer decodes
         * natively: FLAC, MP3, AAC/M4A, Ogg/Opus, WAV. Everything else (DSD, APE, WavPack, WMA, AIFF, TTA, MPC …)
         * is asked for as FLAC with `transcode=1`. ALAC inside .m4a is converted by the server on its own.
         */
        private val ORIGINAL_OK = setOf("mp3", "flac", "m4a", "mp4", "aac", "ogg", "opus", "oga", "wav", "webm")

        fun needsTranscode(format: String): Boolean {
            val f = format.lowercase()
            return f.isNotEmpty() && f !in ORIGINAL_OK
        }

        fun enc(s: String): String = URLEncoder.encode(s, "UTF-8").replace("+", "%20")

        /**
         * The artist's name as the `{name}` segment of `/api/library/artists/{name}/…`, or `_` where the path would not
         * reach the server as written: a "/" or "\" (proxies such as Apache with AllowEncodedSlashes Off answer 404
         * for `%2F` in a path: "AC/DC"), and a name of dots only ("." and ".." are path steps OkHttp resolves before
         * sending, so the request would go to another route). The server reads the real name from `name=`.
         */
        fun artistSegment(name: String): String =
            if (name.isBlank() || name.contains('/') || name.contains('\\') || name.trim('.').isEmpty()) "_" else enc(name)

        /** `/api/library/artists/{name}/<tail>?name=<name>` (client-shapes §3.2, §3.3). */
        fun artistPath(name: String, tail: String): String =
            "/api/library/artists/" + artistSegment(name) + "/$tail?name=" + enc(name)
    }
}

/**
 * GET /api/remote/spot: where the account last listened – the song (`track_id`, with its title / artist / album /
 * duration), `position` in seconds, the queue ids and the index in it, the device that played it, whether it was
 * playing, `updated_at` (epoch seconds), and whether that device is online now (and playing, by its latest check-in).
 */
data class PlaybackSpot(
    val trackId: Long,
    val positionSec: Double,
    val queue: List<Long>,
    val index: Int,
    val deviceId: String,
    val deviceName: String,
    val playing: Boolean,
    val devicePlaying: Boolean,
    val online: Boolean,
    val updatedAt: Double,
    /** `updated_at` as the server sent it: what a closed bar remembers (the web keeps `String(updated_at)`). */
    val updatedAtRaw: String,
    val title: String,
    val artist: String,
    val album: String,
    val albumId: String,
    val durationSec: Double,
) {
    companion object {
        fun from(o: JsonObject): PlaybackSpot? {
            val id = o.long("track_id")
            if (id <= 0L) return null
            val queue = (o["queue"] as? kotlinx.serialization.json.JsonArray)?.mapNotNull { e ->
                (e as? kotlinx.serialization.json.JsonPrimitive)?.content?.toDoubleOrNull()?.toLong()
            } ?: emptyList()
            val playing = o.bool("playing")
            return PlaybackSpot(
                trackId = id,
                positionSec = o.dbl("position").coerceAtLeast(0.0),
                queue = queue,
                index = if (o.containsKey("index")) o.int("index") else o.int("idx"),
                deviceId = o.str("device_id"),
                deviceName = o.str("device_name"),
                playing = playing,
                devicePlaying = if (o.containsKey("device_playing")) o.bool("device_playing") else playing,
                online = o.bool("online"),
                updatedAt = o.dbl("updated_at"),
                updatedAtRaw = o.str("updated_at"),
                title = o.str("title"),
                artist = o.str("artist"),
                album = o.str("album"),
                albumId = o.str("album_id"),
                durationSec = o.dbl("duration"),
            )
        }
    }
}
