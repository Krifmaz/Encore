package app.encore.android.data.repo

import app.encore.android.core.arr
import app.encore.android.core.int
import app.encore.android.core.long
import app.encore.android.core.obj
import app.encore.android.core.objects
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.AuthInfo
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
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

    fun me(): Me = api.meRaw().page(Me.serializer())

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

    fun sessions(): List<SessionRow> = call("GET", "/api/auth/sessions", null).objects().rows(SessionRow.serializer())

    fun revokeSession(id: String) {
        call("DELETE", "/api/auth/sessions/${EncoreApi.enc(id)}", null)
    }

    fun logoutAll() {
        call("POST", "/api/auth/logout-all", JsonObject(emptyMap()))
    }

    fun totpSetup(): TotpSetup = (call("POST", "/api/auth/2fa/setup", JsonObject(emptyMap())) as? JsonObject ?: JsonObject(emptyMap())).page(TotpSetup.serializer())

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
        getObject("/api/discover/search?q=${EncoreApi.enc(q)}&type=${EncoreApi.enc(type)}").page(SearchResults.serializer())

    fun artist(mbid: String): ArtistPage = getObject("/api/discover/artist/${EncoreApi.enc(mbid)}").page(ArtistPage.serializer())

    fun artistExtras(mbid: String): ArtistExtras = getObject("/api/discover/artist/${EncoreApi.enc(mbid)}/extras").page(ArtistExtras.serializer())

    fun artistSimilar(mbid: String): SimilarArtists = getObject("/api/discover/artist/${EncoreApi.enc(mbid)}/similar").page(SimilarArtists.serializer())

    /** Releases on Deezer / Apple Music / Bandcamp / Discogs that MusicBrainz doesn't list (empty when the admin turned the merge off). */
    fun artistCatalog(mbid: String): List<ExtRelease> =
        getObject("/api/discover/artist/${EncoreApi.enc(mbid)}/catalog").arr("releases").objectsOf().rows(ExtRelease.serializer())

    /** POST /api/discover/catalog/tracks: the release's `ext` plus the artist (for track lists without one). */
    fun catalogTracks(ext: JsonElement, artist: String = ""): List<ExtTrack> {
        val body = (ext as? JsonObject)?.let { o -> if (artist.isEmpty()) o else JsonObject(o + ("artist" to JsonPrimitive(artist))) } ?: ext
        return call("POST", "/api/discover/catalog/tracks", body).objects().rows(ExtTrack.serializer())
    }

    /**
     * POST /api/contribute/mb-seed-link: a signed link (good for 15 minutes, no sign-in needed) to the page that opens
     * the MusicBrainz release editor filled in from Deezer / Apple Music. Null when the album is on neither.
     */
    fun mbSeedLink(ext: JsonElement, artistMbid: String, artistName: String): String? {
        val o = ext as? JsonObject ?: return null
        fun id(k: String): String = (o[k] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content.orEmpty()
        val deezer = id("deezer")
        val apple = id("apple")
        val key = when {
            deezer.isNotEmpty() -> "deezer-$deezer"
            apple.isNotEmpty() -> "apple-$apple"
            else -> return null
        }
        val body = buildJsonObject {
            put("key", key)
            put("apple", if (deezer.isNotEmpty()) apple else "")
            put("artist", artistMbid)
            put("artist_name", artistName)
        }
        val path = (call("POST", "/api/contribute/mb-seed-link", body) as? JsonObject)?.str("url").orEmpty()
        return if (path.isEmpty()) null else api.url(path)
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

    fun album(rgid: String): AlbumPage = getObject("/api/discover/album/${EncoreApi.enc(rgid)}").page(AlbumPage.serializer())

    // ---- For you -------------------------------------------------------------------------------------------

    fun forYou(): ForYou = getObject("/api/discover/for-you").page(ForYou.serializer())

    fun tags(): Tags = getObject("/api/discover/tags").page(Tags.serializer())

    fun tag(name: String, page: Int): TagPage = getObject("/api/discover/tag/${EncoreApi.enc(name)}?page=$page").page(TagPage.serializer())

    fun similarTracks(artist: String, title: String, mbid: String): SimilarTracks {
        val qs = StringBuilder("/api/discover/similar-tracks?artist=").append(EncoreApi.enc(artist)).append("&title=").append(EncoreApi.enc(title))
        if (mbid.isNotEmpty()) qs.append("&mbid=").append(EncoreApi.enc(mbid))
        return getObject(qs.toString()).page(SimilarTracks.serializer())
    }

    // ---- Requests ------------------------------------------------------------------------------------------

    /** GET /api/requests?status=&mine=&limit=300 (`status` "all" means no filter). */
    fun requests(status: String, mine: Boolean, limit: Int = 300): List<RequestRow> {
        val qs = StringBuilder("/api/requests?limit=").append(limit)
        if (status.isNotEmpty() && status != "all") qs.append("&status=").append(EncoreApi.enc(status))
        if (mine) qs.append("&mine=true")
        return call("GET", qs.toString(), null).objects().rows(RequestRow.serializer())
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

    /**
     * POST /api/requests/link {url, album_mode} (the Downloader's Download): `albumMode` "song", "album", or "" for
     * the admin's default (sent as null, as the web does).
     */
    fun requestLinkWithMode(url: String, albumMode: String): LinkResult {
        val body = buildJsonObject {
            put("url", url)
            if (albumMode.isEmpty()) put("album_mode", JsonNull) else put("album_mode", albumMode)
        }
        return (call("POST", "/api/requests/link", body) as? JsonObject ?: JsonObject(emptyMap())).page(LinkResult.serializer())
    }

    /** GET /api/discover/probe?url=: title, site, uploader, length or item count, thumbnail (the Downloader's Preview). */
    fun probeLink(url: String): LinkProbe = getObject("/api/discover/probe?url=${EncoreApi.enc(url)}").page(LinkProbe.serializer())

    /** POST /api/requests/retry-failed {mine}: queue every failed request again (members: their own). */
    fun retryFailed(mine: Boolean): RetryFailedResult {
        val o = call("POST", "/api/requests/retry-failed", buildJsonObject { put("mine", mine) }) as? JsonObject
        return RetryFailedResult(o?.int("retried") ?: 0, o?.int("skipped") ?: 0, o?.str("why") ?: "")
    }

    /** How many requests wait for an admin's approval (the web's sidebar count: GET /api/requests?status=pending). */
    fun pendingCount(): Int = call("GET", "/api/requests?status=pending&limit=500", null).objects().size

    // ---- Activity (admins) -----------------------------------------------------------------------------------

    /** GET /api/system/downloads: Soulseek transfers, torrent / Usenet downloads, and why Soulseek's list is missing. */
    fun systemDownloads(): SystemDownloads {
        val o = getObject("/api/system/downloads")
        return SystemDownloads(
            slskd = o.arr("slskd").objectsOf().rows(SlskTransfer.serializer()),
            torrents = o.arr("torrents").objectsOf().rows(TorrentDownload.serializer()),
            slskdError = o.obj("errors")?.str("slskd") ?: "",
        )
    }

    /** GET /api/system/activity: the server log, newest first. */
    fun activity(): List<ActivityLine> = call("GET", "/api/system/activity", null).objects().rows(ActivityLine.serializer())

    /** POST /api/system/downloads/slskd/clear: remove finished Soulseek transfers from the list. */
    fun clearSoulseekTransfers() {
        call("POST", "/api/system/downloads/slskd/clear", JsonObject(emptyMap()))
    }

    // ---- correcting library songs and albums (web-rs/src/pages/library.rs) ------------------------------------

    /** POST /api/library/verify-manual {track_ids[, correct:false]}: "This is the right song" / Undo "confirmed". */
    fun verifyManual(trackIds: List<Long>, correct: Boolean = true) {
        val body = buildJsonObject {
            put("track_ids", buildJsonArray { for (id in trackIds) add(JsonPrimitive(id)) })
            if (!correct) put("correct", false)
        }
        call("POST", "/api/library/verify-manual", body)
    }

    /** DELETE /api/library/tracks/{id}: the file goes from disk (admins; members with `fix` for their own request). */
    fun deleteTrack(id: Long) {
        call("DELETE", "/api/library/tracks/$id", null)
    }

    /** GET /api/library/mismatches (admins): the songs whose audio doesn't match their tags (the web's Wrong audio tab). */
    fun mismatches(): List<Track> = call("GET", "/api/library/mismatches", null).objects().mapNotNull { Track.from(it) }

    /** GET /api/library/tracks/{id}/identify: what the audio fingerprint says the song is. */
    fun identifyTrack(id: Long): List<SongMatch> = call("GET", "/api/library/tracks/$id/identify", null).objects().rows(SongMatch.serializer())

    /** POST /api/library/tracks/{id}/fix {values, move}: the song's new details; returns its album id. */
    fun fixTrack(id: Long, values: Map<String, String>, move: Boolean): String {
        val body = buildJsonObject {
            put("values", buildJsonObject { for ((k, v) in values) put(k, v) })
            put("move", move)
        }
        return (call("POST", "/api/library/tracks/$id/fix", body) as? JsonObject)?.str("album_id") ?: ""
    }

    /** POST /api/library/albums/{id}/artwork: find and embed artwork; returns how many files got it. */
    fun findAlbumArtwork(id: String): Int =
        (call("POST", "/api/library/albums/${EncoreApi.enc(id)}/artwork", JsonObject(emptyMap())) as? JsonObject)?.int("embedded") ?: 0

    /** DELETE /api/library/albums/{id}: every file of the album goes from disk. */
    fun deleteAlbum(id: String) {
        call("DELETE", "/api/library/albums/${EncoreApi.enc(id)}", null)
    }

    /** GET /api/library/albums/{id}/art-candidates?q=: pictures from every catalogue for "Change artwork". */
    fun artCandidates(albumId: String, q: String): List<ArtCandidate> =
        call("GET", "/api/library/albums/${EncoreApi.enc(albumId)}/art-candidates?q=${EncoreApi.enc(q)}", null).objects().rows(ArtCandidate.serializer())

    /** POST /api/library/albums/{id}/art {url} or {data: a data: URL}: embed the picture; returns how many files got it. */
    fun setAlbumArt(albumId: String, url: String? = null, data: String? = null): Int {
        val body = buildJsonObject {
            if (url != null) put("url", url)
            if (data != null) put("data", data)
        }
        return (call("POST", "/api/library/albums/${EncoreApi.enc(albumId)}/art", body) as? JsonObject)?.int("embedded") ?: 0
    }

    /** GET /api/library/albums/{id}/match?provider=&artist=&album=: releases that may be this album ("Tag / match metadata"). */
    fun albumMatch(albumId: String, provider: String, artist: String, album: String): AlbumMatch =
        getObject("/api/library/albums/${EncoreApi.enc(albumId)}/match?provider=${EncoreApi.enc(provider)}&artist=${EncoreApi.enc(artist)}&album=${EncoreApi.enc(album)}")
            .page(AlbumMatch.serializer())

    /** GET /api/library/albums/{id}/preview?release_id=: what the files would be called after tagging from that release. */
    fun albumMatchPreview(albumId: String, releaseId: String): MatchPreview =
        getObject("/api/library/albums/${EncoreApi.enc(albumId)}/preview?release_id=${EncoreApi.enc(releaseId)}").page(MatchPreview.serializer())

    /** POST /api/library/albums/{id}/retag {release_id, rename}: write the release's tags; returns the album's (new) id. */
    fun retagAlbum(albumId: String, releaseId: String, rename: Boolean): String =
        (call("POST", "/api/library/albums/${EncoreApi.enc(albumId)}/retag", buildJsonObject { put("release_id", releaseId); put("rename", rename) }) as? JsonObject)?.str("album_id") ?: ""

    /** GET /api/library/albums/{id}, as it comes (Edit details needs its genre, discs and track numbers). */
    fun albumRaw(albumId: String): JsonObject = getObject("/api/library/albums/${EncoreApi.enc(albumId)}")

    /** POST /api/library/tags {track_ids, values[, rename]}: write tags into the files; returns the album's (new) id. */
    fun writeTags(trackIds: List<Long>, values: Map<String, String>, rename: Boolean = false): String {
        val body = buildJsonObject {
            put("track_ids", buildJsonArray { for (id in trackIds) add(JsonPrimitive(id)) })
            put("values", buildJsonObject { for ((k, v) in values) put(k, v) })
            if (rename) put("rename", true)
        }
        return (call("POST", "/api/library/tags", body) as? JsonObject)?.str("album_id") ?: ""
    }

    /** POST /api/library/albums/{id}/lyrics: look for every song's lyrics → (found, checked; 0 checked: all had some). */
    fun albumLyrics(albumId: String): Pair<Int, Int> {
        val o = call("POST", "/api/library/albums/${EncoreApi.enc(albumId)}/lyrics", JsonObject(emptyMap())) as? JsonObject
        return (o?.int("found") ?: 0) to (o?.int("checked") ?: 0)
    }

    /** DELETE /api/library/artists?name=: every album by the artist goes from disk, their downloads are cancelled. */
    fun removeArtist(name: String): Int =
        (call("DELETE", "/api/library/artists?name=${EncoreApi.enc(name)}", null) as? JsonObject)?.int("removed") ?: 0

    /** GET /api/discover/album-lookup?artist=&album=: the MusicBrainz release group of a library album ("" when unknown). */
    fun albumLookup(artist: String, album: String): String =
        getObject("/api/discover/album-lookup?artist=${EncoreApi.enc(artist)}&album=${EncoreApi.enc(album)}").str("rgid")

    // ---- Picker --------------------------------------------------------------------------------------------

    fun pickerSources(): PickerInfo = getObject("/api/picker/sources").page(PickerInfo.serializer())

    /**
     * POST /api/picker/search for one source. `request_id` goes along when the picker was opened for a request, and
     * `fresh` asks for a new search instead of a kept one ("Search again"); servers that keep no searches ignore both.
     */
    fun pickerSearch(source: String, opts: PickerOpts, query: String, fresh: Boolean = false): PickSearch {
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
            if (opts.requestId > 0L) put("request_id", opts.requestId)
            if (fresh) put("fresh", true)
        }
        return pickSearchOf(call("POST", "/api/picker/search", body))
    }

    /**
     * GET /api/system/soulseek `search_running`: another Soulseek search holds the turn (one runs at a time), so a
     * new one waits for it. Null when the server can't say (an older server, or no permission).
     */
    fun soulseekSearchRunning(): Boolean? =
        try { (call("GET", "/api/system/soulseek", null) as? JsonObject)?.get("search_running")?.let { it.toString() == "true" } } catch (e: CancellationException) { throw e } catch (_: Exception) { null }

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

    fun playlistPage(id: Long): PlaylistPage = getObject("/api/playlists/$id").page(PlaylistPage.serializer())

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
        (call("POST", "/api/playlists/preview", importBody(url, text)) as? JsonObject ?: JsonObject(emptyMap())).page(ImportPreview.serializer())

    fun importPlaylist(url: String?, text: String?, name: String?, follow: Boolean, into: Long?, requestMissing: Boolean, albumMode: String?): ImportResult {
        val body = buildJsonObject {
            for ((k, v) in importBody(url, text)) put(k, v)
            if (!name.isNullOrBlank()) put("name", name.trim())
            put("follow", follow)
            if (into != null) put("into", into)
            put("request_missing", requestMissing)
            if (!albumMode.isNullOrEmpty()) put("album_mode", albumMode)
        }
        return (call("POST", "/api/playlists/import", body) as? JsonObject ?: JsonObject(emptyMap())).page(ImportResult.serializer())
    }

    private fun importBody(url: String?, text: String?): JsonObject = buildJsonObject {
        if (!text.isNullOrBlank()) put("text", text) else put("url", (url ?: "").trim())
    }

    /** POST /api/playlists/{id}/request-missing {album_mode} → what was requested, what is already on its way, what couldn't be. */
    fun requestMissing(id: Long, albumMode: String?): MissingReport {
        val body = buildJsonObject { if (!albumMode.isNullOrEmpty()) put("album_mode", albumMode) }
        return (call("POST", "/api/playlists/$id/request-missing", body) as? JsonObject ?: JsonObject(emptyMap())).page(MissingReport.serializer())
    }

    fun dedupePlaylist(id: Long): Int = (call("POST", "/api/playlists/$id/dedupe", JsonObject(emptyMap())) as? JsonObject)?.int("removed") ?: 0

    fun rematchPlaylist(id: Long): Int = (call("POST", "/api/playlists/$id/rematch", JsonObject(emptyMap())) as? JsonObject)?.int("matched") ?: 0

    fun refreshPlaylist(id: Long): Int = (call("POST", "/api/playlists/$id/refresh", JsonObject(emptyMap())) as? JsonObject)?.int("added") ?: 0

    // ---- Leaderboard ---------------------------------------------------------------------------------------

    fun leaderboard(): Leaderboard = getObject("/api/stats/leaderboard").page(Leaderboard.serializer())

    // ---- Report a problem (web/js/pages/report.js) ----------------------------------------------------------

    /** POST /api/report {kind, track_id | album_id, reason, note}: blocks the source, asks for a replacement. */
    fun report(body: JsonObject): ReportResult {
        val o = call("POST", "/api/report", body) as? JsonObject ?: JsonObject(emptyMap())
        return ReportResult(o.str("status"), o.str("message"))
    }

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

/** What the server did with a report: status replacing | replaced | no_replacement, and its message. */
data class ReportResult(val status: String, val message: String)
