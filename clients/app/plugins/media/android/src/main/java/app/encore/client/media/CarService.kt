package app.encore.client.media

import android.app.PendingIntent
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.CacheBitmapLoader
import androidx.media3.datasource.DataSourceBitmapLoader
import androidx.media3.session.LibraryResult
import androidx.media3.session.MediaConstants
import androidx.media3.session.MediaLibraryService
import androidx.media3.session.MediaSession
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import com.google.common.util.concurrent.SettableFuture
import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Executors

/**
 * Encore for Android Auto (and anything else that browses media apps: Wear, Assistant, Bluetooth stereos).
 *
 * A Media3 library service: the car browses the library through a tree built from the server's API (recently
 * added, Made for you, playlists, liked songs, albums, artists, recently played), voice search looks the library
 * up, and the songs play in ExoPlayer straight from the server with the web view's session cookie – so it works
 * when the car starts Encore with no screen open. It reports now-playing and plays like the web player does
 * (so Last.fm / ListenBrainz keep scrobbling) and shows up under "Your devices" as a remote-controllable player,
 * so the phone's Encore – or any browser – can push a queue to the car.
 */
class CarService : MediaLibraryService() {
    private lateinit var player: ExoPlayer
    private lateinit var session: MediaLibrarySession
    private val api by lazy { EncoreApi(this) }
    private val main = Handler(Looper.getMainLooper())
    private val io = Executors.newCachedThreadPool()
    private val tree = Tree()
    private var remote: RemoteDevice? = null
    private var logged = false
    private var announced: String? = null
    private val searches = HashMap<String, List<MediaItem>>()

    override fun onCreate() {
        super.onCreate()
        instance = this
        // every request carries the current cookie (it changes when you sign in again)
        val http = DefaultHttpDataSource.Factory().setUserAgent("Encore car player").setConnectTimeoutMs(10000).setReadTimeoutMs(20000).setAllowCrossProtocolRedirects(true)
        val source = ResolvingDataSource.Factory(http) { spec -> spec.buildUpon().setHttpRequestHeaders(api.headers()).build() }
        player = ExoPlayer.Builder(this)
            .setMediaSourceFactory(DefaultMediaSourceFactory(source))
            .setAudioAttributes(AudioAttributes.DEFAULT, true)
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .build()
        player.addListener(object : Player.Listener {
            override fun onMediaItemTransition(item: MediaItem?, reason: Int) {
                logged = false
                announce(item)
                remote?.beatSoon()
                saveResumePoint()
            }
            override fun onIsPlayingChanged(isPlaying: Boolean) {
                if (isPlaying) MediaPlugin.current?.control("pause")   // one player at a time: the web view steps back
                remote?.beatSoon()
            }
            override fun onPlaybackStateChanged(state: Int) { remote?.beatSoon() }
        })
        val launch = packageManager.getLaunchIntentForPackage(packageName)
        val builder = MediaLibrarySession.Builder(this, player, Callback())
            .setBitmapLoader(CacheBitmapLoader(DataSourceBitmapLoader(MoreExecutors.listeningDecorator(Executors.newSingleThreadExecutor()), source)))
        if (launch != null) builder.setSessionActivity(PendingIntent.getActivity(this, 0, launch, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
        session = builder.build()
        main.post(tick)
        remote = RemoteDevice().also { it.start() }
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaLibrarySession = session

    override fun onTaskRemoved(rootIntent: Intent?) {
        if (!player.playWhenReady || player.mediaItemCount == 0) stopSelf()
    }

    override fun onDestroy() {
        remote?.stop()
        main.removeCallbacks(tick)
        session.release()
        player.release()
        io.shutdownNow()
        instance = null
        super.onDestroy()
    }

    /** The web view started playing: this player gives way. */
    fun pauseFromWebView() { main.post { if (player.isPlaying) player.pause() } }

    // ---- scrobbling: the same rules as the web player ------------------------------------------------
    private fun announce(item: MediaItem?) {
        val id = item?.let { trackId(it.mediaId) } ?: return
        if (announced == id) return
        announced = id
        io.execute { try { api.post("/api/library/now-playing", JSONObject().put("track_id", id.toLong())) } catch (_: Exception) {} }
    }
    private val tick = object : Runnable {
        override fun run() {
            try {
                val item = player.currentMediaItem
                val id = item?.let { trackId(it.mediaId) }
                if (id != null && player.isPlaying && !logged) {
                    val p = player.currentPosition / 1000.0
                    val d = if (player.duration > 0) player.duration / 1000.0 else 0.0
                    // half the song or 4 minutes, whichever comes first; very short songs at the end
                    val hit = if (d > 0) (if (d < 30) p >= d - 1 else p >= Math.min(d / 2, 240.0)) else p >= 30
                    if (hit) {
                        logged = true
                        io.execute { try { api.post("/api/library/plays", JSONObject().put("track_id", id.toLong()).put("seconds", p)) } catch (_: Exception) {} }
                    }
                }
                remote?.snapshot = snapshot()
            } catch (_: Exception) {}
            main.postDelayed(this, 3000)
        }
    }

    // ---- the queue as the remote protocol sees it ------------------------------------------------------
    private fun snapshot(): JSONObject {
        val item = player.currentMediaItem ?: return JSONObject().put("playing", false)
        val m = item.mediaMetadata
        val id = trackId(item.mediaId)
        val s = JSONObject()
        if (id != null) s.put("id", id.toLong())
        s.put("title", m.title ?: "").put("artist", m.artist ?: "").put("album", m.albumTitle ?: "")
        m.extras?.getString("album_id")?.let { s.put("album_id", it); s.put("cover", "/api/library/albums/$it/cover") }
        m.extras?.getString("format")?.let { s.put("format", it) }
        s.put("playing", player.isPlaying).put("position", player.currentPosition / 1000.0)
        s.put("duration", if (player.duration > 0) player.duration / 1000.0 else 0.0).put("volume", player.volume.toDouble())
        s.put("index", player.currentMediaItemIndex)
        s.put("signal", JSONObject().put("label", (m.extras?.getString("format") ?: "").uppercase()).put("transport", if (item.localConfiguration?.uri?.toString()?.contains("transcode") == true) "flac" else "original"))
        val q = JSONArray()
        for (i in 0 until Math.min(player.mediaItemCount, 300)) {
            val it = player.getMediaItemAt(i)
            val tid = trackId(it.mediaId) ?: continue
            val mm = it.mediaMetadata
            q.put(JSONObject().put("id", tid.toLong()).put("title", mm.title ?: "").put("artist", mm.artist ?: "").put("album", mm.albumTitle ?: "")
                .put("album_id", mm.extras?.getString("album_id")).put("format", mm.extras?.getString("format")).put("duration", mm.extras?.getDouble("duration") ?: 0.0))
        }
        s.put("queue", q)
        return s
    }

    /** A command from another Encore device (the same ones the web player understands). */
    private fun apply(c: JSONObject) {
        when (c.optString("action")) {
            "toggle" -> if (player.isPlaying) player.pause() else { player.prepare(); player.play() }
            "play" -> { player.prepare(); player.play() }
            "pause", "stop" -> player.pause()
            "next" -> player.seekToNextMediaItem()
            "prev" -> if (player.currentPosition > 3000 || !player.hasPreviousMediaItem()) player.seekTo(0) else player.seekToPreviousMediaItem()
            "seek" -> player.seekTo((c.optDouble("value", 0.0) * 1000).toLong())
            "volume" -> player.volume = c.optDouble("value", 1.0).toFloat().coerceIn(0f, 1f)
            "enqueue" -> player.addMediaItems(items(c.optJSONArray("tracks")))
            "play_next" -> player.addMediaItems(player.currentMediaItemIndex + 1, items(c.optJSONArray("tracks")))
            "play_queue" -> {
                val list = items(c.optJSONArray("tracks"))
                if (list.isEmpty()) return
                val index = c.optInt("index", 0).coerceIn(0, list.size - 1)
                player.setMediaItems(list, index, (c.optDouble("position", 0.0) * 1000).toLong().coerceAtLeast(0))
                player.prepare()
                player.play()
            }
        }
    }
    private fun items(arr: JSONArray?): List<MediaItem> {
        val out = ArrayList<MediaItem>()
        if (arr == null) return out
        for (i in 0 until arr.length()) { val t = arr.optJSONObject(i) ?: continue; if (t.has("id")) out.add(tree.trackItem(t, null)) }
        return out
    }

    private fun saveResumePoint() {
        try {
            val ids = (0 until player.mediaItemCount).mapNotNull { trackId(player.getMediaItemAt(it).mediaId) }.take(200)
            getSharedPreferences("encore", MODE_PRIVATE).edit().putString("resume_ids", ids.joinToString(",")).putInt("resume_index", player.currentMediaItemIndex).apply()
        } catch (_: Exception) {}
    }

    // ---- the library as a browse tree -------------------------------------------------------------------
    /** Builds media items from the server's JSON; remembers every track it listed so a chosen one can be played with its siblings. */
    inner class Tree {
        val tracks = HashMap<String, JSONObject>()        // track id → row
        val children = HashMap<String, List<MediaItem>>()  // parent id → what we listed under it

        fun root(): MediaItem = folder("root", "Encore", null, MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false)

        fun home(): List<MediaItem> = listOf(
            folder("recent", "Recently added", "New in your library", MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS, grid = true),
            folder("played", "Recently played", "Pick up where you left off", MediaMetadata.MEDIA_TYPE_FOLDER_MIXED, grid = false),
            folder("mfy", "Made for you", "Your ListenBrainz playlists", MediaMetadata.MEDIA_TYPE_FOLDER_PLAYLISTS, grid = true),
            folder("playlists", "Playlists", null, MediaMetadata.MEDIA_TYPE_FOLDER_PLAYLISTS, grid = true),
            folder("liked", "Liked Songs", null, MediaMetadata.MEDIA_TYPE_PLAYLIST, grid = false),
            folder("albums", "Albums", null, MediaMetadata.MEDIA_TYPE_FOLDER_ALBUMS, grid = true),
            folder("artists", "Artists", null, MediaMetadata.MEDIA_TYPE_FOLDER_ARTISTS, grid = false),
            folder("shuffle", "Shuffle everything", "A random walk through the whole library", MediaMetadata.MEDIA_TYPE_PLAYLIST, grid = false),
        )

        /** The children of a node, fetched from the server. */
        fun list(parent: String): List<MediaItem> {
            val out: List<MediaItem> = when {
                parent == "root" -> home()
                parent == "recent" -> albums(api.getArray("/api/library/recent?limit=60"), parent)
                parent == "albums" -> albums(api.getArray("/api/library/albums?sort=artist&limit=500"), parent)
                parent == "artists" -> api.getArray("/api/library/artists?sort=name").map { a -> val name = a.optString("name"); folder("artist:${name}", name, plural(a.optInt("albums"), "album"), MediaMetadata.MEDIA_TYPE_ARTIST, grid = true, art = a.optString("cover_album").takeIf { it.isNotEmpty() }?.let { cover(it) }) }
                parent.startsWith("artist:") -> albums(api.getArray("/api/library/albums?artist=${api.enc(parent.removePrefix("artist:"))}&limit=200"), parent)
                parent.startsWith("album:") -> api.getObject("/api/library/albums/${parent.removePrefix("album:")}").optJSONArray("tracks").map { trackItem(it, parent) }
                parent == "playlists" -> api.getArray("/api/playlists").filter { it.optString("kind") != "liked" }.map { p -> folder("playlist:${p.optLong("id")}", p.optString("name"), plural(p.optInt("total"), "song"), MediaMetadata.MEDIA_TYPE_PLAYLIST, grid = true, art = p.optString("cover_url").takeIf { it.isNotEmpty() }) }
                parent == "liked" -> {
                    val liked = api.getArray("/api/playlists").firstOrNull { it.optString("kind") == "liked" }
                    if (liked == null) emptyList() else playlistTracks(liked.optLong("id"), parent)
                }
                parent.startsWith("playlist:") -> playlistTracks(parent.removePrefix("playlist:").toLong(), parent)
                parent == "mfy" -> api.getObject("/api/discover/made-for-you").optJSONArray("playlists").map { p -> folder("mfy:${p.optString("kind")}", p.optString("name"), "${p.optInt("owned")} of ${p.optInt("total")} in your library", MediaMetadata.MEDIA_TYPE_PLAYLIST, grid = true, art = p.optJSONArray("covers")?.optString(0)?.takeIf { it.isNotEmpty() }) }
                parent.startsWith("mfy:") -> {
                    val kind = parent.removePrefix("mfy:")
                    val p = api.getObject("/api/discover/made-for-you").optJSONArray("playlists").firstOrNull { it.optString("kind") == kind }
                    p?.optJSONArray("tracks").mapNotNull { t -> t.optJSONObject("library")?.let { trackItem(it, parent) } } ?: emptyList()
                }
                parent == "played" -> api.getArray("/api/library/plays/recent?limit=60").map { trackItem(it, parent) }
                parent == "shuffle" -> (api.post("/api/library/shuffle", JSONObject().put("limit", 100).put("exclude", JSONArray())) as? JSONArray).map { trackItem(it, parent) }
                else -> emptyList()
            }
            children[parent] = out
            return out
        }

        private fun playlistTracks(id: Long, parent: String): List<MediaItem> {
            val d = api.getObject("/api/playlists/$id")
            val arr = d.optJSONArray("tracks") ?: d.optJSONArray("items") ?: JSONArray()
            return arr.mapNotNull { row ->
                // a playlist row carries the matched library track (or is one itself)
                val t = row.optJSONObject("track") ?: row.optJSONObject("library") ?: row
                if (t.has("id") && (t.has("path") || t.has("format") || row.has("track_id") || t.has("album_id"))) trackItem(t, parent) else null
            }
        }

        private fun albums(arr: JSONArray, parent: String): List<MediaItem> = arr.map { a ->
            val id = a.optString("id")
            folder("album:$id", a.optString("album"), listOfNotNull(a.optString("albumartist").takeIf { it.isNotEmpty() }, a.optString("year").takeIf { it.isNotEmpty() }).joinToString(" · "), MediaMetadata.MEDIA_TYPE_ALBUM, grid = true, art = cover(id))
        }

        fun cover(albumId: String): String = api.url("/api/library/albums/$albumId/cover?s=600")

        /** A playable song. `parent` is where it was listed – picking it plays the whole list from there. */
        fun trackItem(t: JSONObject, parent: String?): MediaItem {
            val id = t.optLong("id").toString()
            tracks[id] = t
            val format = t.optString("format").lowercase()
            val albumId = t.optString("album_id")
            val convert = format in CONVERT
            val uri = api.url("/api/library/tracks/$id/stream" + if (convert) "?transcode=1" else "")
            val extras = Bundle().apply { putString("album_id", albumId); putString("format", format); putDouble("duration", t.optDouble("duration", 0.0)) }
            val md = MediaMetadata.Builder()
                .setTitle(t.optString("title").ifEmpty { "Unknown title" })
                .setArtist(t.optString("artist"))
                .setAlbumTitle(t.optString("album"))
                .setAlbumArtist(t.optString("albumartist").ifEmpty { t.optString("artist") })
                .setTrackNumber(t.optInt("track").takeIf { it > 0 })
                .setDurationMs((t.optDouble("duration", 0.0) * 1000).toLong().takeIf { it > 0 })
                .setIsBrowsable(false).setIsPlayable(true).setMediaType(MediaMetadata.MEDIA_TYPE_MUSIC)
                .setExtras(extras)
            if (albumId.isNotEmpty()) md.setArtworkUri(Uri.parse(cover(albumId)))
            return MediaItem.Builder().setMediaId("track:$id" + (parent?.let { "|$it" } ?: "")).setUri(uri).setMediaMetadata(md.build()).build()
        }

        private fun folder(id: String, title: String, subtitle: String?, type: Int, grid: Boolean, art: String? = null): MediaItem {
            val extras = Bundle().apply {
                putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_BROWSABLE, if (grid) MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_GRID_ITEM else MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM)
                putInt(MediaConstants.EXTRAS_KEY_CONTENT_STYLE_PLAYABLE, MediaConstants.EXTRAS_VALUE_CONTENT_STYLE_LIST_ITEM)
            }
            val md = MediaMetadata.Builder().setTitle(title).setSubtitle(subtitle).setIsBrowsable(true).setIsPlayable(id == "shuffle" || id == "liked" || id.startsWith("playlist:") || id.startsWith("album:") || id.startsWith("mfy:")).setMediaType(type).setExtras(extras)
            if (art != null) md.setArtworkUri(Uri.parse(if (art.startsWith("http")) art else api.url(art)))
            return MediaItem.Builder().setMediaId(id).setMediaMetadata(md.build()).build()
        }

        /** A library search for the car's voice command: songs, albums and artists whose names match. */
        fun search(q: String): List<MediaItem> {
            val out = ArrayList<MediaItem>()
            out += api.getArray("/api/library/tracks?q=${api.enc(q)}&limit=20").map { trackItem(it, "search:$q") }
            out += albums(api.getArray("/api/library/albums?q=${api.enc(q)}&limit=10"), "search:$q")
            out += api.getArray("/api/library/artists?q=${api.enc(q)}").take(5).map { a -> val name = a.optString("name"); folder("artist:$name", name, "Artist", MediaMetadata.MEDIA_TYPE_ARTIST, grid = true, art = a.optString("cover_album").takeIf { it.isNotEmpty() }?.let { cover(it) }) }
            children["search:$q"] = out.filter { it.mediaMetadata.isPlayable == true && it.mediaId.startsWith("track:") }
            return out
        }

        private fun plural(n: Int, what: String) = "$n $what${if (n == 1) "" else "s"}"
    }

    /** The playable items that `id` stands for: a track (with its listed siblings), or everything under a folder. */
    private fun resolve(item: MediaItem): Pair<List<MediaItem>, Int> {
        val id = item.mediaId
        if (id.startsWith("track:")) {
            val (tid, parent) = id.removePrefix("track:").split("|", limit = 2).let { it[0] to it.getOrNull(1) }
            val siblings = parent?.let { tree.children[it] ?: try { tree.list(it) } catch (_: Exception) { null } }
            if (siblings != null) {
                val at = siblings.indexOfFirst { trackId(it.mediaId) == tid }
                if (at >= 0) return siblings to at
            }
            val row = tree.tracks[tid] ?: try { api.getObject("/api/library/tracks/$tid") } catch (_: Exception) { null }
            return if (row != null && row.has("id")) listOf(tree.trackItem(row, null)) to 0 else listOf(item) to 0
        }
        val list = tree.list(id).filter { it.mediaId.startsWith("track:") }
        return list to 0
    }

    private inner class Callback : MediaLibrarySession.Callback {
        override fun onGetLibraryRoot(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, params: LibraryParams?): ListenableFuture<LibraryResult<MediaItem>> {
            if (!api.configured) return Futures.immediateFuture(LibraryResult.ofError(LibraryResult.RESULT_ERROR_SESSION_AUTHENTICATION_EXPIRED))
            return Futures.immediateFuture(LibraryResult.ofItem(tree.root(), params))
        }

        override fun onGetChildren(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, parentId: String, page: Int, pageSize: Int, params: LibraryParams?): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> {
            val f = SettableFuture.create<LibraryResult<ImmutableList<MediaItem>>>()
            io.execute {
                try {
                    val all = tree.list(parentId)
                    val from = (page * pageSize).coerceAtMost(all.size)
                    val to = (from + pageSize).coerceAtMost(all.size)
                    f.set(LibraryResult.ofItemList(ImmutableList.copyOf(if (pageSize > 0) all.subList(from, to) else all), params))
                } catch (e: EncoreApi.ApiException) {
                    f.set(LibraryResult.ofError(if (e.code == 401) LibraryResult.RESULT_ERROR_SESSION_AUTHENTICATION_EXPIRED else LibraryResult.RESULT_ERROR_IO))
                } catch (_: Exception) { f.set(LibraryResult.ofError(LibraryResult.RESULT_ERROR_IO)) }
            }
            return f
        }

        override fun onGetItem(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, mediaId: String): ListenableFuture<LibraryResult<MediaItem>> {
            val f = SettableFuture.create<LibraryResult<MediaItem>>()
            io.execute {
                try {
                    val (list, at) = resolve(MediaItem.Builder().setMediaId(mediaId).build())
                    f.set(if (list.isEmpty()) LibraryResult.ofError(LibraryResult.RESULT_ERROR_BAD_VALUE) else LibraryResult.ofItem(list[at], null))
                } catch (_: Exception) { f.set(LibraryResult.ofError(LibraryResult.RESULT_ERROR_IO)) }
            }
            return f
        }

        override fun onSearch(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, query: String, params: LibraryParams?): ListenableFuture<LibraryResult<Void>> {
            io.execute {
                val found = try { tree.search(query) } catch (_: Exception) { emptyList() }
                synchronized(searches) { searches[query] = found }
                main.post { session.notifySearchResultChanged(browser, query, found.size, params) }
            }
            return Futures.immediateFuture(LibraryResult.ofVoid())
        }

        override fun onGetSearchResult(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, query: String, page: Int, pageSize: Int, params: LibraryParams?): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> {
            val found = synchronized(searches) { searches[query] } ?: emptyList()
            val from = (page * pageSize).coerceAtMost(found.size)
            val to = (from + pageSize).coerceAtMost(found.size)
            return Futures.immediateFuture(LibraryResult.ofItemList(ImmutableList.copyOf(if (pageSize > 0) found.subList(from, to) else found), params))
        }

        /** Items a controller adds (e.g. "play this song"): give them their stream URIs. */
        override fun onAddMediaItems(mediaSession: MediaSession, controller: MediaSession.ControllerInfo, mediaItems: MutableList<MediaItem>): ListenableFuture<MutableList<MediaItem>> {
            val f = SettableFuture.create<MutableList<MediaItem>>()
            io.execute {
                val out = ArrayList<MediaItem>()
                for (it in mediaItems) {
                    if (it.localConfiguration != null) { out.add(it); continue }
                    try { out.addAll(resolve(it).first) } catch (_: Exception) {}
                }
                f.set(out)
            }
            return f
        }

        /** "Play" on a browsed song: the whole list it came from, starting there. */
        override fun onSetMediaItems(mediaSession: MediaSession, controller: MediaSession.ControllerInfo, mediaItems: MutableList<MediaItem>, startIndex: Int, startPositionMs: Long): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val f = SettableFuture.create<MediaSession.MediaItemsWithStartPosition>()
            io.execute {
                try {
                    if (mediaItems.size == 1 && mediaItems[0].localConfiguration == null) {
                        val (list, at) = resolve(mediaItems[0])
                        f.set(MediaSession.MediaItemsWithStartPosition(list, at, startPositionMs.coerceAtLeast(0)))
                    } else {
                        val out = ArrayList<MediaItem>()
                        for (it in mediaItems) { if (it.localConfiguration != null) out.add(it) else out.addAll(resolve(it).first) }
                        f.set(MediaSession.MediaItemsWithStartPosition(out, startIndex.coerceIn(0, (out.size - 1).coerceAtLeast(0)), startPositionMs.coerceAtLeast(0)))
                    }
                } catch (_: Exception) { f.set(MediaSession.MediaItemsWithStartPosition(mediaItems, startIndex, startPositionMs)) }
            }
            return f
        }

        /** The car's "resume" button: the last queue, or recently played. */
        override fun onPlaybackResumption(mediaSession: MediaSession, controller: MediaSession.ControllerInfo): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val f = SettableFuture.create<MediaSession.MediaItemsWithStartPosition>()
            io.execute {
                try {
                    val prefs = getSharedPreferences("encore", MODE_PRIVATE)
                    val ids = (prefs.getString("resume_ids", "") ?: "").split(",").filter { it.isNotEmpty() }
                    val items = if (ids.isNotEmpty()) ids.mapNotNull { id -> try { api.getObject("/api/library/tracks/$id").takeIf { it.has("id") }?.let { tree.trackItem(it, null) } } catch (_: Exception) { null } }
                        else tree.list("played")
                    f.set(MediaSession.MediaItemsWithStartPosition(items, prefs.getInt("resume_index", 0).coerceIn(0, (items.size - 1).coerceAtLeast(0)), 0))
                } catch (_: Exception) { f.set(MediaSession.MediaItemsWithStartPosition(emptyList(), 0, 0)) }
            }
            return f
        }
    }

    // ---- an Encore device: heartbeat + the server's event stream, like web/js/remote.js ------------------
    private inner class RemoteDevice {
        @Volatile var snapshot: JSONObject = JSONObject().put("playing", false)
        private val id = "car-" + (Settings.Secure.getString(contentResolver, Settings.Secure.ANDROID_ID) ?: "0").take(16)
        private val name = "Car · ${Build.MODEL}"
        private val seen = ArrayList<Long>()
        @Volatile private var running = false
        private var beatNow = false

        fun start() {
            running = true
            Thread({ loop() }, "encore-remote-beat").apply { isDaemon = true }.start()
            Thread({ events() }, "encore-remote-events").apply { isDaemon = true }.start()
        }
        fun stop() { running = false }
        fun beatSoon() { beatNow = true }

        private fun run(c: JSONObject) {
            if (c.has("seq")) {
                val seq = c.optLong("seq")
                synchronized(seen) { if (seen.contains(seq)) return; seen.add(seq); if (seen.size > 200) seen.subList(0, 100).clear() }
            }
            main.post { try { apply(c) } catch (_: Exception) {} }
        }

        private fun loop() {
            while (running) {
                try {
                    if (api.configured) {
                        val r = api.post("/api/remote/heartbeat", JSONObject().put("id", id).put("name", name).put("state", snapshot)) as? JSONObject
                        r?.optJSONArray("commands")?.let { for (i in 0 until it.length()) it.optJSONObject(i)?.let(::run) }
                    }
                } catch (_: Exception) {}
                // check in every 2 s, right away when something changed
                var waited = 0
                while (running && waited < 2000 && !beatNow) { Thread.sleep(100); waited += 100 }
                beatNow = false
            }
        }

        /** The server pushes our commands the moment they are sent. */
        private fun events() {
            while (running) {
                try {
                    if (!api.configured) { Thread.sleep(5000); continue }
                    val c = URL(api.url("/api/remote/events?device=${api.enc(id)}")).openConnection() as HttpURLConnection
                    for ((k, v) in api.headers()) if (v.isNotEmpty()) c.setRequestProperty(k, v)
                    c.setRequestProperty("Accept", "text/event-stream")
                    c.readTimeout = 0
                    if (c.responseCode != 200) { c.disconnect(); Thread.sleep(30000); continue }
                    BufferedReader(InputStreamReader(c.inputStream)).use { r ->
                        var event = ""
                        while (running) {
                            val line = r.readLine() ?: break
                            when {
                                line.startsWith("event:") -> event = line.substring(6).trim()
                                line.startsWith("data:") -> if (event == "command") (org.json.JSONTokener(line.substring(5).trim()).nextValue() as? JSONObject)?.optJSONObject("command")?.let(::run)
                                line.isEmpty() -> event = ""
                            }
                        }
                    }
                    c.disconnect()
                } catch (_: Exception) {}
                if (running) Thread.sleep(5000)
            }
        }
    }

    companion object {
        @Volatile var instance: CarService? = null
        /** formats Android can't decode → the server converts them to FLAC */
        private val CONVERT = setOf("ape", "wv", "aiff", "aif", "wma", "alac", "dsf", "dff", "tta", "mpc")
        fun trackId(mediaId: String): String? = if (mediaId.startsWith("track:")) mediaId.removePrefix("track:").substringBefore("|") else null
        private inline fun <T> JSONArray?.map(f: (JSONObject) -> T): List<T> { val out = ArrayList<T>(); if (this != null) for (i in 0 until length()) optJSONObject(i)?.let { out.add(f(it)) }; return out }
        private inline fun <T> JSONArray?.mapNotNull(f: (JSONObject) -> T?): List<T> { val out = ArrayList<T>(); if (this != null) for (i in 0 until length()) optJSONObject(i)?.let { f(it)?.let(out::add) }; return out }
        private inline fun JSONArray?.filter(f: (JSONObject) -> Boolean): List<JSONObject> { val out = ArrayList<JSONObject>(); if (this != null) for (i in 0 until length()) optJSONObject(i)?.let { if (f(it)) out.add(it) }; return out }
        private inline fun JSONArray?.firstOrNull(f: (JSONObject) -> Boolean): JSONObject? { if (this != null) for (i in 0 until length()) optJSONObject(i)?.let { if (f(it)) return it }; return null }
        private fun JSONArray?.take(n: Int): List<JSONObject> { val out = ArrayList<JSONObject>(); if (this != null) for (i in 0 until Math.min(n, length())) optJSONObject(i)?.let(out::add); return out }
    }
}
