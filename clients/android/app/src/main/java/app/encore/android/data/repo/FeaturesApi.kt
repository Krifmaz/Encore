package app.encore.android.data.repo

import app.encore.android.data.api.ApiException
import app.encore.android.data.api.AuthInfo
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.data.api.int
import app.encore.android.data.api.long
import app.encore.android.data.api.objects
import app.encore.android.data.api.objectsOf
import app.encore.android.data.api.str
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MultipartBody
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

/**
 * The server calls behind Discover, For you, Requests, the picker, playlist editing, Account and the Leaderboard
 * (web/js/pages/{discover,foryou,requests,picker,playlists,account,leaderboard}.js). Blocking, like [EncoreApi];
 * callers run them on Dispatchers.IO. Requests go through the shared OkHttpClient, so they carry the session.
 */
class FeaturesApi(private val api: EncoreApi, private val client: OkHttpClient) {
    private val json get() = api.json
    private val jsonType = "application/json; charset=utf-8".toMediaType()

    // ---- account -------------------------------------------------------------------------------------------

    fun me(): Me = Me.from(api.meRaw())

    /** GET /api/auth/info of the signed-in server: which account features exist (rename, avatar, totp, logout_all). */
    fun info(): AuthInfo = api.info(api.base)

    /** PUT /api/auth/profile {username} or {email}; returns `{username, email}` as the server now has them. */
    fun updateProfile(username: String? = null, email: String? = null): JsonObject {
        val body = buildJsonObject {
            if (username != null) put("username", username)
            if (email != null) put("email", email)
        }
        return call("PUT", "/api/auth/profile", body) as? JsonObject ?: JsonObject(emptyMap())
    }

    fun changePassword(current: String, new: String) {
        call("POST", "/api/auth/password", buildJsonObject { put("current", current); put("new", new) })
    }

    fun sessions(): List<SessionRow> = call("GET", "/api/auth/sessions", null).objects().map { SessionRow.from(it) }

    fun revokeSession(id: String) {
        call("DELETE", "/api/auth/sessions/${EncoreApi.enc(id)}", null)
    }

    fun logoutAll() {
        call("POST", "/api/auth/logout-all", JsonObject(emptyMap()))
    }

    fun totpSetup(): TotpSetup = TotpSetup.from(call("POST", "/api/auth/2fa/setup", JsonObject(emptyMap())) as? JsonObject ?: JsonObject(emptyMap()))

    fun totpEnable(code: String) {
        call("POST", "/api/auth/2fa/enable", buildJsonObject { put("code", code) })
    }

    fun totpDisable(password: String) {
        call("POST", "/api/auth/2fa/disable", buildJsonObject { put("password", password) })
    }

    /** POST /api/auth/avatar (multipart `file`): the picture, which the server crops square and stores as JPEG. */
    fun uploadAvatar(bytes: ByteArray, mime: String) {
        val type = (mime.ifEmpty { "image/jpeg" }).toMediaType()
        val body = MultipartBody.Builder().setType(MultipartBody.FORM)
            .addFormDataPart("file", "avatar", bytes.toRequestBody(type))
            .build()
        val req = Request.Builder().url(api.url("/api/auth/avatar")).post(body).header("Accept", "application/json").build()
        client.newCall(req).execute().use { res ->
            if (!res.isSuccessful) {
                val o = parseObject(res.body.string())
                throw ApiException(res.code, o?.str("detail")?.ifEmpty { null } ?: "Upload failed", o)
            }
        }
    }

    fun deleteAvatar() {
        call("DELETE", "/api/auth/avatar", null)
    }

    /** The avatar's URL for Coil (`?v=` busts the cache after a change). */
    fun avatarUrl(userId: Long, version: Double): String = api.url("/api/users/$userId/avatar?v=${version.toLong()}")

    // ---- Discover (MusicBrainz) -----------------------------------------------------------------------------

    /** GET /api/discover/search?q=&type=all|artist|album|track. */
    fun search(q: String, type: String): SearchResults =
        SearchResults.from(getObject("/api/discover/search?q=${EncoreApi.enc(q)}&type=${EncoreApi.enc(type)}"))

    fun artist(mbid: String): ArtistPage = ArtistPage.from(getObject("/api/discover/artist/${EncoreApi.enc(mbid)}"))

    fun artistExtras(mbid: String): ArtistExtras = ArtistExtras.from(getObject("/api/discover/artist/${EncoreApi.enc(mbid)}/extras"))

    fun artistSimilar(mbid: String): SimilarArtists = SimilarArtists.from(getObject("/api/discover/artist/${EncoreApi.enc(mbid)}/similar"))

    /** Releases on Deezer / Apple Music / Bandcamp / Discogs that MusicBrainz doesn't list (empty when the admin turned the merge off). */
    fun artistCatalog(mbid: String): List<ExtRelease> =
        getObject("/api/discover/artist/${EncoreApi.enc(mbid)}/catalog").let { o -> (o["releases"] as? kotlinx.serialization.json.JsonArray).objectsOf().mapNotNull { ExtRelease.from(it) } }

    /** POST /api/discover/catalog/tracks: the release's `ext` plus the artist (for track lists without one). */
    fun catalogTracks(ext: JsonElement, artist: String = ""): List<ExtTrack> {
        val body = (ext as? JsonObject)?.let { o -> if (artist.isEmpty()) o else JsonObject(o + ("artist" to JsonPrimitive(artist))) } ?: ext
        return call("POST", "/api/discover/catalog/tracks", body).objects().mapNotNull { ExtTrack.from(it) }
    }

    private val sseClient: OkHttpClient by lazy { client.newBuilder().cache(null).readTimeout(0, TimeUnit.MILLISECONDS).build() }

    /**
     * GET /api/requests/events: [onEvent] gets every change of the user's requests (`{request_id, kind, keys, status,
     * label, progress}`) until the coroutine is cancelled; reconnects with a backoff. Run it on Dispatchers.IO.
     * The server sends a keep-alive every 15 s, so a cancelled caller stops within that.
     */
    suspend fun requestEvents(onEvent: suspend (JsonObject) -> Unit) {
        var backoff = 2000L
        while (currentCoroutineContext().isActive) {
            try {
                if (api.base.isEmpty()) throw IOException("Not signed in")
                val req = Request.Builder().url(api.url("/api/requests/events"))
                    .header("Accept", "text/event-stream").header("Cache-Control", "no-cache").build()
                sseClient.newCall(req).execute().use { res ->
                    if (!res.isSuccessful) throw IOException("HTTP ${res.code}")
                    backoff = 2000L
                    val src = res.body.source()
                    var event = ""
                    val data = StringBuilder()
                    while (currentCoroutineContext().isActive) {
                        val line = src.readUtf8Line() ?: break
                        when {
                            line.isEmpty() -> {
                                if (event == "request" && data.isNotEmpty()) parseObject(data.toString())?.let { onEvent(it) }
                                event = ""
                                data.setLength(0)
                            }
                            line.startsWith(":") -> {}
                            line.startsWith("event:") -> event = line.substring(6).trim()
                            line.startsWith("data:") -> {
                                if (data.isNotEmpty()) data.append('\n')
                                data.append(line.substring(5).removePrefix(" "))
                            }
                        }
                    }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
            }
            delay(backoff)
            backoff = (backoff * 2).coerceAtMost(30_000L)
        }
    }

    /** Every library song by this artist, in album order (the artist page's Play / Shuffle). */
    fun artistLibraryTracks(mbid: String, name: String): List<Track> =
        call("GET", "/api/discover/artist/${EncoreApi.enc(mbid)}/library-tracks?name=${EncoreApi.enc(name)}", null).objects().mapNotNull { Track.from(it) }

    /** A library artist's MusicBrainz id (404 when MusicBrainz doesn't know the name). */
    fun artistLookup(name: String): String = getObject("/api/discover/artist-lookup?name=${EncoreApi.enc(name)}").str("mbid")

    fun album(rgid: String): AlbumPage = AlbumPage.from(getObject("/api/discover/album/${EncoreApi.enc(rgid)}"))

    // ---- For you -------------------------------------------------------------------------------------------

    fun forYou(): ForYou = ForYou.from(getObject("/api/discover/for-you"))

    fun tags(): Tags = Tags.from(getObject("/api/discover/tags"))

    fun tag(name: String, page: Int): TagPage = TagPage.from(getObject("/api/discover/tag/${EncoreApi.enc(name)}?page=$page"))

    fun similarTracks(artist: String, title: String, mbid: String): SimilarTracks {
        val qs = StringBuilder("/api/discover/similar-tracks?artist=").append(EncoreApi.enc(artist)).append("&title=").append(EncoreApi.enc(title))
        if (mbid.isNotEmpty()) qs.append("&mbid=").append(EncoreApi.enc(mbid))
        return SimilarTracks.from(getObject(qs.toString()))
    }

    // ---- Requests ------------------------------------------------------------------------------------------

    /** GET /api/requests?status=&mine=&limit=300 (`status` "all" means no filter). */
    fun requests(status: String, mine: Boolean, limit: Int = 300): List<RequestRow> {
        val qs = StringBuilder("/api/requests?limit=").append(limit)
        if (status.isNotEmpty() && status != "all") qs.append("&status=").append(EncoreApi.enc(status))
        if (mine) qs.append("&mine=true")
        return call("GET", qs.toString(), null).objects().mapNotNull { RequestRow.from(it) }
    }

    /** POST /api/requests: the new row; its `status` is `pending` (needs approval) or `queued`. */
    fun createRequest(body: JsonObject): String = (call("POST", "/api/requests", body) as? JsonObject)?.str("status") ?: ""

    /** POST /api/requests/{id}/approve|deny|retry|cancel. */
    fun requestAction(id: Long, action: String) {
        call("POST", "/api/requests/$id/$action", JsonObject(emptyMap()))
    }

    /** DELETE /api/requests/{id}[?files=true]; returns the number of files deleted. */
    fun deleteRequest(id: Long, files: Boolean): Int =
        (call("DELETE", "/api/requests/$id" + if (files) "?files=true" else "", null) as? JsonObject)?.int("deleted_files") ?: 0

    /** POST /api/requests/clear → (removed, deleted_files). */
    fun clearRequests(statuses: List<String>, files: Boolean, mine: Boolean): Pair<Int, Int> {
        val body = buildJsonObject {
            put("statuses", buildJsonArray { for (s in statuses) add(JsonPrimitive(s)) })
            put("files", files)
            put("mine", mine)
        }
        val o = call("POST", "/api/requests/clear", body) as? JsonObject
        return (o?.int("removed") ?: 0) to (o?.int("deleted_files") ?: 0)
    }

    /** POST /api/requests/link {url}: request whatever a link points at (same as "Share to Encore"). */
    fun requestLink(url: String): String = api.requestLink(url)

    // ---- Picker --------------------------------------------------------------------------------------------

    fun pickerSources(): PickerInfo = PickerInfo.from(getObject("/api/picker/sources"))

    fun pickerSearch(source: String, opts: PickerOpts, query: String): List<PickResult> {
        val body = buildJsonObject {
            put("source", source)
            put("kind", opts.kind)
            put("artist", opts.artist)
            put("title", opts.title.ifEmpty { opts.album })
            put("query", query)
            if (opts.kind == "track" && opts.album.isNotEmpty()) put("album", opts.album)
            if (opts.mbid.isNotEmpty()) put("mbid", opts.mbid)
            if (opts.artistMbid.isNotEmpty()) put("artist_mbid", opts.artistMbid)
            if (opts.trackCount > 0) put("track_count", opts.trackCount)
            if (opts.lengthSec > 0) put("length", opts.lengthSec)
        }
        return call("POST", "/api/picker/search", body).objects().mapNotNull { PickResult.from(it) }
    }

    /** POST /api/picker/pick: a request locked to this result; returns its status (`pending` or `queued`). */
    fun pick(source: String, payload: JsonElement, title: String, opts: PickerOpts): String {
        val body = buildJsonObject {
            put("source", source)
            put("payload", payload)
            put("title", title)
            if (opts.requestId > 0L) {
                put("request_id", opts.requestId)
            } else {
                put("request", buildJsonObject {
                    put("kind", opts.kind)
                    if (opts.mbid.isNotEmpty()) put("mbid", opts.mbid)
                    put("artist", opts.artist)
                    put("title", opts.title.ifEmpty { opts.album })
                    val album = opts.album.ifEmpty { if (opts.kind == "album") opts.title else "" }
                    if (album.isNotEmpty()) put("album", album)
                    if (opts.year.isNotEmpty()) put("year", opts.year)
                    if (opts.coverUrl.isNotEmpty()) put("cover_url", opts.coverUrl)
                    if (opts.lengthSec > 0) put("length", opts.lengthSec)
                    if (opts.isrc.isNotEmpty()) put("isrc", opts.isrc)
                    put("album_mode", "song")
                    val ext = opts.ext
                    if (ext != null && ext !is JsonNull) put("ext", ext)
                })
            }
        }
        return (call("POST", "/api/picker/pick", body) as? JsonObject)?.str("status") ?: ""
    }

    // ---- Playlists -----------------------------------------------------------------------------------------

    fun playlistPage(id: Long): PlaylistPage = PlaylistPage.from(getObject("/api/playlists/$id"))

    /** POST /api/playlists {name} → the new playlist's id. */
    fun createPlaylist(name: String): Long = (call("POST", "/api/playlists", buildJsonObject { put("name", name) }) as? JsonObject)?.long("id") ?: 0L

    fun renamePlaylist(id: Long, name: String) {
        call("PATCH", "/api/playlists/$id", buildJsonObject { put("name", name) })
    }

    fun setPlaylistFollow(id: Long, follow: Boolean) {
        call("PATCH", "/api/playlists/$id", buildJsonObject { put("follow", follow) })
    }

    fun deletePlaylist(id: Long) {
        call("DELETE", "/api/playlists/$id", null)
    }

    fun removePlaylistItem(playlistId: Long, itemId: Long) {
        call("DELETE", "/api/playlists/$playlistId/items/$itemId", null)
    }

    /** POST /api/playlists/{id}/move {item_id, to}: the item lands at position `to` (0-based). */
    fun movePlaylistItem(playlistId: Long, itemId: Long, to: Int) {
        call("POST", "/api/playlists/$playlistId/move", buildJsonObject { put("item_id", itemId); put("to", to) })
    }

    fun importPreview(url: String?, text: String?): ImportPreview =
        ImportPreview.from(call("POST", "/api/playlists/preview", importBody(url, text)) as? JsonObject ?: JsonObject(emptyMap()))

    fun importPlaylist(url: String?, text: String?, name: String?, follow: Boolean, into: Long?, requestMissing: Boolean, albumMode: String?): ImportResult {
        val body = buildJsonObject {
            for ((k, v) in importBody(url, text)) put(k, v)
            if (!name.isNullOrBlank()) put("name", name.trim())
            put("follow", follow)
            if (into != null) put("into", into)
            put("request_missing", requestMissing)
            if (!albumMode.isNullOrEmpty()) put("album_mode", albumMode)
        }
        return ImportResult.from(call("POST", "/api/playlists/import", body) as? JsonObject ?: JsonObject(emptyMap()))
    }

    private fun importBody(url: String?, text: String?): JsonObject = buildJsonObject {
        if (!text.isNullOrBlank()) put("text", text) else put("url", (url ?: "").trim())
    }

    /** POST /api/playlists/{id}/request-missing {album_mode} → how many songs were requested. */
    fun requestMissing(id: Long, albumMode: String?): Int {
        val body = buildJsonObject { if (!albumMode.isNullOrEmpty()) put("album_mode", albumMode) }
        return (call("POST", "/api/playlists/$id/request-missing", body) as? JsonObject)?.int("requested") ?: 0
    }

    fun dedupePlaylist(id: Long): Int = (call("POST", "/api/playlists/$id/dedupe", JsonObject(emptyMap())) as? JsonObject)?.int("removed") ?: 0

    fun rematchPlaylist(id: Long): Int = (call("POST", "/api/playlists/$id/rematch", JsonObject(emptyMap())) as? JsonObject)?.int("matched") ?: 0

    fun refreshPlaylist(id: Long): Int = (call("POST", "/api/playlists/$id/refresh", JsonObject(emptyMap())) as? JsonObject)?.int("added") ?: 0

    // ---- Leaderboard ---------------------------------------------------------------------------------------

    fun leaderboard(): Leaderboard = Leaderboard.from(getObject("/api/stats/leaderboard"))

    // ---- plumbing ------------------------------------------------------------------------------------------

    private fun getObject(path: String): JsonObject = call("GET", path, null) as? JsonObject ?: JsonObject(emptyMap())

    private fun call(method: String, path: String, body: JsonElement?): JsonElement? {
        if (api.base.isEmpty()) throw IOException("Not signed in")
        val rb: RequestBody? = when {
            body != null -> json.encodeToString(JsonElement.serializer(), body).toRequestBody(jsonType)
            method == "POST" || method == "PUT" || method == "PATCH" || method == "DELETE" -> "".toRequestBody(jsonType)
            else -> null
        }
        val req = Request.Builder().url(api.url(path)).method(method, rb).header("Accept", "application/json").build()
        client.newCall(req).execute().use { res ->
            val text = res.body.string()
            if (!res.isSuccessful) {
                val o = parseObject(text)
                val detail = o?.str("detail")?.ifEmpty { null } ?: "HTTP ${res.code}"
                throw ApiException(res.code, detail, o)
            }
            if (text.isBlank()) return null
            val el = try { json.parseToJsonElement(text) } catch (e: Exception) { throw IOException("Unexpected reply from the server") }
            app.encore.android.data.api.CoverVersions.note(el)
            return el
        }
    }

    private fun parseObject(text: String): JsonObject? =
        try { json.parseToJsonElement(text) as? JsonObject } catch (_: Exception) { null }
}

private val featuresCache = java.util.WeakHashMap<app.encore.android.AppGraph, FeaturesApi>()

/** The one [FeaturesApi] of the app's graph (built lazily; AppGraph itself stays the player/design parts' file). */
val app.encore.android.AppGraph.features: FeaturesApi
    get() = synchronized(featuresCache) { featuresCache.getOrPut(this) { FeaturesApi(api, http) } }
