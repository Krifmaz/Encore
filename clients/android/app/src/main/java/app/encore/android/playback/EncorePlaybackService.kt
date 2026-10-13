package app.encore.android.playback

import android.app.PendingIntent
import android.content.Intent
import android.os.Handler
import android.os.Looper
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.datasource.DataSourceBitmapLoader
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.CacheBitmapLoader
import androidx.media3.session.DefaultMediaNotificationProvider
import androidx.media3.session.LibraryResult
import androidx.media3.session.MediaLibraryService
import androidx.media3.session.MediaSession
import app.encore.android.AppGraph
import app.encore.android.EncoreApp
import app.encore.android.R
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.Track
import app.encore.android.ui.MainActivity
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.SettableFuture
import kotlinx.coroutines.runBlocking
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * The one playback service (PLAN §2.2, §6.1): ExoPlayer + a Media3 library session. The phone UI connects with a
 * MediaController; the notification, lock screen, Bluetooth and Android Auto talk to the same session.
 *
 * Ported from the Tauri app's CarService: the browse tree, voice search, playback resumption and the scrobble
 * rules (now playing when a song starts; a play at half the song or 4 minutes, songs under 30 s at the end).
 */
class EncorePlaybackService : MediaLibraryService() {
    private lateinit var graph: AppGraph
    private lateinit var player: ExoPlayer
    private lateinit var session: MediaLibrarySession
    private lateinit var tree: BrowseTree
    private lateinit var resume: ResumeStore
    private val io: ExecutorService = Executors.newFixedThreadPool(4)
    private val main = Handler(Looper.getMainLooper())
    private val searches = ConcurrentHashMap<String, List<MediaItem>>()
    private var released = false

    // scrobbling state (main thread only)
    private var announced: MediaItem? = null
    private var logged = false
    private var lastId: Long? = null
    private var lastPositionSec = 0.0
    private var lastDurationSec = 0.0

    // autoplay: songs played in this listening session never come back in the shuffle
    private val played = LinkedHashSet<Long>()
    private var fetchingMore = false

    override fun onCreate() {
        super.onCreate()
        graph = (application as EncoreApp).graph
        tree = BrowseTree(graph.api)
        resume = ResumeStore(this)

        // Audio goes through the shared client (auth + User-Agent) but without the JSON cache.
        val mediaHttp = graph.http.newBuilder().cache(null).readTimeout(30, TimeUnit.SECONDS).build()
        val dataSourceFactory = DefaultDataSource.Factory(this, OkHttpDataSource.Factory(mediaHttp))
        val audioAttributes = AudioAttributes.Builder()
            .setUsage(C.USAGE_MEDIA)
            .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC)
            .build()
        player = ExoPlayer.Builder(this)
            .setMediaSourceFactory(DefaultMediaSourceFactory(dataSourceFactory))
            .setAudioAttributes(audioAttributes, true)
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .build()
        player.addListener(listener)

        val notifications = DefaultMediaNotificationProvider.Builder(this).build()
        notifications.setSmallIcon(R.drawable.ic_note)
        setMediaNotificationProvider(notifications)

        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val sessionActivity = PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        session = MediaLibrarySession.Builder(this, player, Callback())
            .setSessionActivity(sessionActivity)
            // covers are content:// URIs from CoverProvider, read with the default data source
            .setBitmapLoader(CacheBitmapLoader(DataSourceBitmapLoader.Builder(this).build()))
            .build()
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaLibrarySession = session

    override fun onDestroy() {
        released = true
        main.removeCallbacks(tick)
        saveResume()
        session.release()
        player.removeListener(listener)
        player.release()
        io.shutdownNow()
        super.onDestroy()
    }

    // ---- scrobbling: the same rules and endpoints as web/js/player.js ---------------------------------------

    private val listener = object : Player.Listener {
        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
            // a song that ran to its end (gapless hand-over) counts with its full length
            if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_AUTO || reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT) {
                val id = lastId
                if (!logged && id != null && lastDurationSec > 0) logPlay(id, lastDurationSec)
            }
            logged = false
            lastId = null
            lastPositionSec = 0.0
            lastDurationSec = 0.0
            if (mediaItem != null) {
                announce(mediaItem)
                MediaItems.trackId(mediaItem.mediaId)?.let { played.add(it) }
            }
            saveResume()
            maybeAutoplay()
        }

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            main.removeCallbacks(tick)
            if (isPlaying) main.post(tick) else saveResumePosition()
        }

        override fun onPlaybackStateChanged(playbackState: Int) {
            if (playbackState == Player.STATE_ENDED) {
                val id = lastId
                if (!logged && id != null && lastDurationSec > 0) logPlay(id, lastDurationSec)
                logged = true
                maybeAutoplay()
            }
        }
    }

    /** Runs once a second while playing (and only then: no requests while paused). */
    private val tick = object : Runnable {
        override fun run() {
            if (released) return
            checkPlay()
            if (player.isPlaying) main.postDelayed(this, 1000L)
        }
    }

    private fun checkPlay() {
        val item = player.currentMediaItem ?: return
        val id = MediaItems.trackId(item.mediaId) ?: return
        val p = player.currentPosition / 1000.0
        val d = if (player.duration > 0) player.duration / 1000.0
            else item.mediaMetadata.extras?.getDouble(MediaItems.EXTRA_DURATION) ?: 0.0
        lastId = id
        lastPositionSec = p
        lastDurationSec = d
        if (logged) return
        // half the song or 4 minutes, whichever comes first; songs under 30 s at their end
        val hit = if (d > 0) (if (d < 30) p >= d - 1 else p >= minOf(d / 2, 240.0)) else p >= 30
        if (hit) logPlay(id, p)
    }

    private fun logPlay(id: Long, seconds: Double) {
        logged = true
        io.execute { try { graph.api.logPlay(id, seconds) } catch (_: Exception) {} }
    }

    private fun announce(item: MediaItem) {
        if (announced === item) return
        announced = item
        val id = MediaItems.trackId(item.mediaId) ?: return
        io.execute { try { graph.api.nowPlaying(id) } catch (_: Exception) {} }
    }

    // ---- autoplay: when the queue runs out, a shuffle of the whole library ----------------------------------

    private fun maybeAutoplay() {
        if (fetchingMore || !graph.store.settings.value.autoplay) return
        val count = player.mediaItemCount
        if (count == 0 || player.repeatMode != Player.REPEAT_MODE_OFF) return
        if (player.currentMediaItemIndex < count - 1 && player.playbackState != Player.STATE_ENDED) return
        fetchingMore = true
        val exclude = ArrayList<Long>(played)
        for (i in 0 until count) MediaItems.trackId(player.getMediaItemAt(i).mediaId)?.let { exclude.add(it) }
        io.execute {
            val more = try { graph.api.shuffle(50, exclude.distinct()) } catch (_: Exception) { emptyList() }
            main.post {
                fetchingMore = false
                if (released || more.isEmpty()) return@post
                val ended = player.playbackState == Player.STATE_ENDED
                player.addMediaItems(more.map { MediaItems.track(graph.api, it) })
                if (ended) {
                    player.seekTo(player.currentMediaItemIndex + 1, 0L)
                    player.prepare()
                    player.play()
                }
            }
        }
    }

    // ---- resume ----------------------------------------------------------------------------------------------

    private fun saveResume() {
        try {
            val tracks = ArrayList<Track>()
            for (i in 0 until player.mediaItemCount) MediaItems.toTrack(player.getMediaItemAt(i))?.let { tracks.add(it) }
            if (tracks.isEmpty()) return
            resume.save(tracks, player.currentMediaItemIndex, player.currentPosition)
        } catch (_: Exception) {}
    }

    private fun saveResumePosition() {
        try {
            if (player.mediaItemCount > 0) resume.savePosition(player.currentMediaItemIndex, player.currentPosition)
        } catch (_: Exception) {}
    }

    // ---- the session -----------------------------------------------------------------------------------------

    private inner class Callback : MediaLibrarySession.Callback {
        /** Android Auto, the system UI and our own app get full control; other apps get what Media3 allows them. */
        override fun onConnect(session: MediaSession, controller: MediaSession.ControllerInfo): MediaSession.ConnectionResult {
            if (controller.isTrusted || controller.packageName in FULL_ACCESS || controller.packageName == packageName) {
                return MediaSession.ConnectionResult.accept(
                    MediaSession.ConnectionResult.DEFAULT_SESSION_AND_LIBRARY_COMMANDS,
                    MediaSession.ConnectionResult.DEFAULT_PLAYER_COMMANDS,
                )
            }
            return MediaSession.ConnectionResult.AcceptedResultBuilder(session, controller).build()
        }

        override fun onGetLibraryRoot(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, params: LibraryParams?): ListenableFuture<LibraryResult<MediaItem>> {
            val f = SettableFuture.create<LibraryResult<MediaItem>>()
            io.execute {
                try { runBlocking { graph.store.awaitLoaded() } } catch (_: Exception) {}
                if (graph.store.current == null) {
                    f.set(LibraryResult.ofError(LibraryResult.RESULT_ERROR_SESSION_AUTHENTICATION_EXPIRED))
                } else {
                    f.set(LibraryResult.ofItem(tree.root(), params))
                }
            }
            return f
        }

        override fun onGetChildren(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, parentId: String, page: Int, pageSize: Int, params: LibraryParams?): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> {
            val f = SettableFuture.create<LibraryResult<ImmutableList<MediaItem>>>()
            io.execute {
                try {
                    runBlocking { graph.store.awaitLoaded() }
                    val all = tree.list(parentId)
                    f.set(LibraryResult.ofItemList(ImmutableList.copyOf(pageOf(all, page, pageSize)), params))
                } catch (e: ApiException) {
                    f.set(LibraryResult.ofError(if (e.code == 401) LibraryResult.RESULT_ERROR_SESSION_AUTHENTICATION_EXPIRED else LibraryResult.RESULT_ERROR_IO))
                } catch (_: Exception) {
                    f.set(LibraryResult.ofError(LibraryResult.RESULT_ERROR_IO))
                }
            }
            return f
        }

        override fun onGetItem(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, mediaId: String): ListenableFuture<LibraryResult<MediaItem>> {
            val f = SettableFuture.create<LibraryResult<MediaItem>>()
            io.execute {
                try {
                    runBlocking { graph.store.awaitLoaded() }
                    val (list, at) = tree.resolve(mediaId)
                    f.set(if (list.isEmpty()) LibraryResult.ofError(LibraryResult.RESULT_ERROR_BAD_VALUE) else LibraryResult.ofItem(list[at], null))
                } catch (_: Exception) {
                    f.set(LibraryResult.ofError(LibraryResult.RESULT_ERROR_IO))
                }
            }
            return f
        }

        override fun onSearch(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, query: String, params: LibraryParams?): ListenableFuture<LibraryResult<Void>> {
            io.execute {
                val found = try { tree.search(query) } catch (_: Exception) { emptyList() }
                searches[query] = found
                main.post { if (!released) session.notifySearchResultChanged(browser, query, found.size, params) }
            }
            return Futures.immediateFuture(LibraryResult.ofVoid())
        }

        override fun onGetSearchResult(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, query: String, page: Int, pageSize: Int, params: LibraryParams?): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> {
            val found = searches[query] ?: emptyList()
            return Futures.immediateFuture(LibraryResult.ofItemList(ImmutableList.copyOf(pageOf(found, page, pageSize)), params))
        }

        /** Items a controller adds ("play this", "add to queue"): give them their stream URIs. */
        override fun onAddMediaItems(mediaSession: MediaSession, controller: MediaSession.ControllerInfo, mediaItems: MutableList<MediaItem>): ListenableFuture<MutableList<MediaItem>> {
            val f = SettableFuture.create<MutableList<MediaItem>>()
            io.execute {
                val out = ArrayList<MediaItem>()
                for (item in mediaItems) {
                    val ready = MediaItems.withUri(graph.api, item)
                    if (ready != null) { out.add(ready); continue }
                    try { out.addAll(resolveIncoming(item).first) } catch (_: Exception) {}
                }
                f.set(out)
            }
            return f
        }

        /** "Play" on a browsed song plays the whole list it came from, starting there; folders play all their songs. */
        override fun onSetMediaItems(mediaSession: MediaSession, controller: MediaSession.ControllerInfo, mediaItems: MutableList<MediaItem>, startIndex: Int, startPositionMs: Long): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val f = SettableFuture.create<MediaSession.MediaItemsWithStartPosition>()
            io.execute {
                try {
                    runBlocking { graph.store.awaitLoaded() }
                    val single = mediaItems.size == 1 && mediaItems[0].localConfiguration == null
                    if (single && (MediaItems.parentOf(mediaItems[0].mediaId) != null || MediaItems.trackId(mediaItems[0].mediaId) == null)) {
                        val (list, at) = resolveIncoming(mediaItems[0])
                        f.set(MediaSession.MediaItemsWithStartPosition(list, at, startPositionMs.coerceAtLeast(0L)))
                    } else {
                        val out = ArrayList<MediaItem>()
                        for (item in mediaItems) {
                            val ready = MediaItems.withUri(graph.api, item)
                            if (ready != null) out.add(ready) else try { out.addAll(resolveIncoming(item).first) } catch (_: Exception) {}
                        }
                        val index = if (out.isEmpty()) 0 else startIndex.coerceIn(0, out.size - 1)
                        f.set(MediaSession.MediaItemsWithStartPosition(out, index, startPositionMs))
                    }
                } catch (_: Exception) {
                    f.set(MediaSession.MediaItemsWithStartPosition(mediaItems, startIndex, startPositionMs))
                }
            }
            return f
        }

        /** "Resume" from the lock screen, Bluetooth or the car: the last queue (stored on the phone), or recently played. */
        override fun onPlaybackResumption(mediaSession: MediaSession, controller: MediaSession.ControllerInfo, isForPlayback: Boolean): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val f = SettableFuture.create<MediaSession.MediaItemsWithStartPosition>()
            io.execute {
                try {
                    runBlocking { graph.store.awaitLoaded() }
                    val snap = resume.load()
                    if (snap != null) {
                        val items = snap.tracks.map { MediaItems.track(graph.api, it) }
                        f.set(MediaSession.MediaItemsWithStartPosition(items, snap.index, snap.positionMs))
                    } else if (graph.store.current != null) {
                        val items = graph.api.recentPlays(50).map { MediaItems.track(graph.api, it) }
                        f.set(MediaSession.MediaItemsWithStartPosition(items, 0, 0L))
                    } else {
                        f.setException(IllegalStateException("Not signed in"))
                    }
                } catch (e: Exception) {
                    f.setException(e)
                }
            }
            return f
        }
    }

    /**
     * What a controller's item stands for: a browse id or song id (via the tree), or – with an empty id – a voice
     * request ("play <album>", "play <artist>", or just "play music"), answered from the library.
     */
    private fun resolveIncoming(item: MediaItem): Pair<List<MediaItem>, Int> {
        if (item.mediaId.isNotEmpty()) return tree.resolve(item.mediaId)
        return playFromSearch(item.requestMetadata.searchQuery ?: "") to 0
    }

    private fun playFromSearch(query: String): List<MediaItem> {
        val q = query.trim()
        if (q.isEmpty()) return tree.list("shuffle").filter { it.mediaId.startsWith("track:") }
        val albums = graph.api.albums(q = q, limit = 10)
        val album = albums.firstOrNull { it.title.equals(q, ignoreCase = true) || "${it.artist} ${it.title}".equals(q, ignoreCase = true) }
        if (album != null) return tree.list("album:${album.id}")
        val tracks = graph.api.tracks(q = q, limit = 100)
        if (tracks.isNotEmpty()) return tracks.map { MediaItems.track(graph.api, it) }
        val first = albums.firstOrNull() ?: return emptyList()
        return tree.list("album:${first.id}")
    }

    private fun pageOf(all: List<MediaItem>, page: Int, pageSize: Int): List<MediaItem> {
        if (pageSize <= 0) return all
        val from = (page.toLong() * pageSize).coerceAtMost(all.size.toLong()).toInt()
        val to = (from + pageSize).coerceAtMost(all.size)
        return all.subList(from, to)
    }

    private companion object {
        /** Controllers that may play and skip even if Media3 can't verify them as trusted. */
        val FULL_ACCESS = setOf(
            "com.google.android.projection.gearhead",   // Android Auto
            "com.google.android.autosimulator",          // Desktop Head Unit
            "com.android.systemui",
            "com.google.android.carassistant",
            "com.google.android.googlequicksearchbox",   // Assistant
            "com.google.android.wearable.app",
        )
    }
}
