package app.encore.android.car

import android.content.ComponentName
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.graphics.Bitmap
import android.os.Bundle
import androidx.car.app.AppManager
import androidx.car.app.CarAppService
import androidx.car.app.CarContext
import androidx.car.app.CarToast
import androidx.car.app.Screen
import androidx.car.app.Session
import androidx.car.app.model.Action
import androidx.car.app.model.ActionStrip
import androidx.car.app.model.CarIcon
import androidx.car.app.model.ItemList
import androidx.car.app.model.ListTemplate
import androidx.car.app.model.MessageTemplate
import androidx.car.app.model.Row
import androidx.car.app.model.Template
import androidx.car.app.navigation.model.MapWithContentTemplate
import androidx.car.app.validation.HostValidator
import androidx.core.content.ContextCompat
import androidx.core.graphics.drawable.IconCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.media3.common.MediaItem
import androidx.media3.session.LibraryResult
import androidx.media3.session.MediaBrowser
import androidx.media3.session.MediaController
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionToken
import app.encore.android.R
import app.encore.android.features.group.GroupListen
import app.encore.android.graph
import app.encore.android.playback.ArtistNode
import app.encore.android.playback.CarRules
import app.encore.android.playback.EncorePlaybackService
import app.encore.android.playback.MediaItems
import com.google.common.util.concurrent.ListenableFuture
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import java.util.concurrent.Executors

/**
 * Encore Lyrics: a second car app (besides the media app Android Auto builds from EncorePlaybackService) that
 * shows the playing song's synced lyrics on the car screen. A point-of-interest template app (it is not a
 * navigation app) with the map templates permission. For that category MapWithContentTemplate is the template
 * with the largest surface the app draws on itself (NavigationTemplate's full-screen map is for navigation apps
 * only), so: the surface draws the lyrics over all the free area ([LyricsSurface]); the content card Android
 * Auto requires holds one row, the song with a Like button (a tap opens "More by <artist>", [BrowseScreen]); and
 * the controls are one strip of icons: Previous, Play/Pause, Next, and "More by <artist>" (the person icon: adds
 * a shuffle of the artist's songs after the current one).
 *
 * It reads the song and position from the playback service in this process ([CarNowPlaying]) and controls it
 * through a MediaBrowser on the service's session (which also starts the service when the car app opens first);
 * "More by <artist>" reads the media app's browse nodes (`more-by:…`, `artist-shuffle:<name>`) through the same
 * browser.
 */
class LyricsCarAppService : CarAppService() {
    override fun createHostValidator(): HostValidator {
        val debuggable = (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE) != 0
        return if (debuggable) {
            HostValidator.ALLOW_ALL_HOSTS_VALIDATOR
        } else {
            HostValidator.Builder(applicationContext)
                .addAllowedHosts(androidx.car.app.R.array.hosts_allowlist_sample)
                .build()
        }
    }

    override fun onCreateSession(): Session = LyricsCarSession()
}

class LyricsCarSession : Session() {
    private var controllerFuture: ListenableFuture<MediaBrowser>? = null
    private var controller: MediaBrowser? = null
    private var surface: LyricsSurface? = null
    /** browse requests made before the browser connected */
    private val waiting = ArrayList<(MediaBrowser?) -> Unit>()

    override fun onCreateScreen(intent: Intent): Screen {
        val ctx = carContext
        val token = SessionToken(ctx, ComponentName(ctx, EncorePlaybackService::class.java))
        val future = MediaBrowser.Builder(ctx, token).buildAsync()
        controllerFuture = future
        future.addListener({
            controller = try { future.get() } catch (_: Exception) { null }
            val pending = ArrayList(waiting)
            waiting.clear()
            for (w in pending) w(controller)
        }, ContextCompat.getMainExecutor(ctx))

        val s = LyricsSurface(ctx) { ms -> seek(ms) }
        surface = s
        if (ctx.carAppApiLevel >= MIN_API) {
            try { ctx.getCarService(AppManager::class.java).setSurfaceCallback(s) } catch (_: Exception) {}
        }
        lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onDestroy(owner: LifecycleOwner) {
                try { ctx.getCarService(AppManager::class.java).setSurfaceCallback(null) } catch (_: Exception) {}
                s.release()
                surface = null
                controllerFuture?.let { MediaController.releaseFuture(it) }
                controllerFuture = null
                controller = null
                waiting.clear()
            }
        })
        return LyricsScreen(ctx, s, this)
    }

    fun playPause() {
        val c = controller ?: return
        if (c.isPlaying) c.pause() else c.play()
    }

    fun next() { controller?.seekToNext() }

    fun previous() { controller?.seekToPrevious() }

    /** Like or unlike the song (the same likes the phone, the car's media buttons and the web use). */
    fun toggleLike(trackId: Long) { carContext.graph.likes.toggle(trackId) }

    /**
     * "More by <artist>": up to [CarRules.MORE_BY_SONGS] of the artist's songs, shuffled, after the current one (in
     * a group listen the service adds them to the group's Up next). [onDone] gets the short message to show.
     * Main thread.
     */
    fun moreBy(artist: ArtistNode, onDone: (String) -> Unit) {
        children(CarRules.ARTIST_SHUFFLE + artist.name) { list ->
            val c = controller
            if (list == null || c == null) { onDone(CarRules.moreByFailed(artist.name)); return@children }
            val playing = c.currentMediaItem?.mediaId?.let { MediaItems.trackId(it) }
            val songs = list.filter { it.mediaId.startsWith("track:") && MediaItems.trackId(it.mediaId) != playing }.take(CarRules.MORE_BY_SONGS)
            if (songs.isNotEmpty()) {
                val at = if (c.mediaItemCount == 0) 0 else (c.currentMediaItemIndex + 1).coerceIn(0, c.mediaItemCount)
                c.addMediaItems(at, songs)
            }
            onDone(CarRules.moreByMessage(artist.name, songs.size, GroupListen.inGroup))
        }
    }

    /**
     * The children of a browse node of the media app ([onDone] gets null when the phone's Encore can't answer).
     * Main thread; [onDone] runs on the main thread.
     */
    fun children(nodeId: String, onDone: (List<MediaItem>?) -> Unit) {
        val c = controller
        if (c == null) {
            if (controllerFuture == null) { onDone(null); return }
            waiting.add { b -> if (b == null) onDone(null) else children(nodeId, onDone) }
            return
        }
        val f = try { c.getChildren(nodeId, 0, BROWSE_PAGE, null) } catch (_: Exception) { onDone(null); return }
        f.addListener({
            val r = try { f.get() } catch (_: Exception) { null }
            onDone(if (r != null && r.resultCode == LibraryResult.RESULT_SUCCESS) r.value?.toList() ?: emptyList() else null)
        }, ContextCompat.getMainExecutor(carContext))
    }

    /**
     * Plays a browse item: a song with the list it came from, or everything a folder or Shuffle stands for (the
     * service resolves the id as it does for Android Auto).
     */
    fun play(mediaId: String) {
        val c = controller ?: return
        c.setMediaItem(MediaItem.Builder().setMediaId(mediaId).build())
        c.prepare()
        c.play()
    }

    /** From the start of the song, also for server-made streams (the service's own seek command). */
    fun seek(ms: Long) {
        val c = controller ?: return
        val args = Bundle().apply { putLong(EncorePlaybackService.ARG_POSITION_MS, ms) }
        c.sendCustomCommand(SessionCommand(EncorePlaybackService.CMD_SEEK, Bundle.EMPTY), args)
    }

    companion object {
        /** MapWithContentTemplate needs car API level 7 (car-app library 1.7); the manifest asks for it too. */
        const val MIN_API = 7

        /** most items one browse request asks for (the host shows fewer: its list limit) */
        const val BROWSE_PAGE = 200
    }
}

/**
 * The one screen: the lyrics on the map surface (all the free area), the song in a one-row card with Like, the
 * controls in the action strip.
 */
class LyricsScreen(
    carContext: CarContext,
    private val surface: LyricsSurface,
    private val session: LyricsCarSession,
) : Screen(carContext) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private val covers = Executors.newSingleThreadExecutor { r -> Thread(r, "car-lyrics-cover").apply { isDaemon = true } }

    /** what the template shows now: a new template only when it changes (hosts limit template updates) */
    private var shownKey: List<Any?>? = null
    private var np: NowPlaying? = null
    private var coverFor: String? = null
    private var cover: Bitmap? = null
    private var liked = false

    init {
        lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) = surface.setShown(true)
            override fun onStop(owner: LifecycleOwner) = surface.setShown(false)
            override fun onDestroy(owner: LifecycleOwner) {
                scope.cancel()
                covers.shutdownNow()
            }
        })
        scope.launch {
            CarNowPlaying.state.collect { now ->
                np = now
                if (now?.artworkUri?.toString() != coverFor) loadCover(now)
                refresh()
            }
        }
        scope.launch { carContext.graph.likes.liked.collect { refresh() } }
    }

    /** A new template when what it shows changed. */
    private fun refresh() {
        val now = np
        liked = now?.trackId?.let { carContext.graph.likes.isLiked(it) } == true
        val key = listOf(now?.mediaId, now?.title, now?.artist, now?.album, now?.artistNode, now?.playing, liked, coverFor == now?.artworkUri?.toString() && cover != null)
        if (key != shownKey) {
            shownKey = key
            invalidate()
        }
    }

    private fun loadCover(now: NowPlaying?) {
        val uri = now?.artworkUri
        coverFor = uri?.toString()
        cover = null
        if (uri == null) return
        val want = coverFor
        try {
            covers.execute {
                val b = CoverArt.load(carContext, uri, 128)
                scope.launch {
                    if (coverFor != want || b == null) return@launch
                    cover = b
                    shownKey = null
                    refresh()
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {}
    }

    override fun onGetTemplate(): Template {
        if (carContext.carAppApiLevel < LyricsCarSession.MIN_API) {
            return MessageTemplate.Builder("Encore Lyrics needs a newer Android Auto. Update Android Auto on your phone.")
                .setTitle("Encore Lyrics")
                .setHeaderAction(Action.APP_ICON)
                .build()
        }
        val now = np
        val row = Row.Builder()
        if (now == null) {
            row.setTitle("Nothing playing")
            row.addText("Start a song in Encore")
        } else {
            row.setTitle(now.title.ifEmpty { "Unknown title" })
            val sub = listOf(now.artist, now.album).filter { it.isNotEmpty() }.joinToString(" · ")
            if (sub.isNotEmpty()) row.addText(sub)
            cover?.let { row.setImage(CarIcon.Builder(IconCompat.createWithBitmap(it)).build(), Row.IMAGE_TYPE_SMALL) }
            now.trackId?.let { id ->
                row.addAction(
                    Action.Builder()
                        .setIcon(icon(if (liked) R.drawable.ic_favorite else R.drawable.ic_favorite_border))
                        .setOnClickListener { session.toggleLike(id) }
                        .build(),
                )
            }
            // the artist's page (Shuffle, Popular songs, Albums, Liked from this artist), as on the media app's Home
            now.artistNode?.let { a ->
                row.setOnClickListener { screenManager.push(BrowseScreen(carContext, session, CarRules.MORE_BY + a.key, CarRules.moreByTitle(a.name))) }
            }
        }
        // no header: the card Android Auto keeps beside the lyrics stays one row high
        val content = ListTemplate.Builder()
            .setSingleList(ItemList.Builder().addItem(row.build()).build())
            .build()
        val playing = now?.playing == true
        val strip = ActionStrip.Builder()
            .addAction(iconAction(R.drawable.ic_skip_previous) { session.previous() })
            .addAction(iconAction(if (playing) R.drawable.ic_pause else R.drawable.ic_play) { session.playPause() })
            .addAction(iconAction(R.drawable.ic_skip_next) { session.next() })
        // the icon alone, like the other three, so the strip stays small on wide screens too; the message after a
        // tap names the artist and what was added
        now?.artistNode?.let { a ->
            strip.addAction(
                iconAction(R.drawable.ic_person) {
                    session.moreBy(a) { msg -> CarToast.makeText(carContext, msg, CarToast.LENGTH_SHORT).show() }
                },
            )
        }
        return MapWithContentTemplate.Builder()
            .setContentTemplate(content)
            .setActionStrip(strip.build())
            .build()
    }

    private fun icon(res: Int): CarIcon = CarIcon.Builder(IconCompat.createWithResource(carContext, res)).build()

    private fun iconAction(res: Int, onClick: () -> Unit): Action =
        Action.Builder()
            .setIcon(icon(res))
            .setOnClickListener { onClick() }
            .build()
}
