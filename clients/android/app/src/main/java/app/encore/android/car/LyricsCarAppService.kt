package app.encore.android.car

import android.content.ComponentName
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.graphics.Bitmap
import android.os.Bundle
import androidx.car.app.AppManager
import androidx.car.app.CarAppService
import androidx.car.app.CarContext
import androidx.car.app.Screen
import androidx.car.app.Session
import androidx.car.app.model.Action
import androidx.car.app.model.ActionStrip
import androidx.car.app.model.CarIcon
import androidx.car.app.model.MessageTemplate
import androidx.car.app.model.Pane
import androidx.car.app.model.PaneTemplate
import androidx.car.app.model.Row
import androidx.car.app.model.Template
import androidx.car.app.navigation.model.MapController
import androidx.car.app.navigation.model.MapWithContentTemplate
import androidx.car.app.validation.HostValidator
import androidx.core.content.ContextCompat
import androidx.core.graphics.drawable.IconCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.media3.session.MediaController
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionToken
import app.encore.android.R
import app.encore.android.playback.EncorePlaybackService
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
 * navigation app) with the map templates permission: MapWithContentTemplate's map surface draws the lyrics
 * ([LyricsSurface]), its content pane shows the song with previous / play-pause / next.
 *
 * It reads the song and position from the playback service in this process ([CarNowPlaying]) and controls it
 * through a MediaController on the service's session, which also starts the service when the car app opens
 * first.
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
    private var controllerFuture: ListenableFuture<MediaController>? = null
    private var controller: MediaController? = null
    private var surface: LyricsSurface? = null

    override fun onCreateScreen(intent: Intent): Screen {
        val ctx = carContext
        val token = SessionToken(ctx, ComponentName(ctx, EncorePlaybackService::class.java))
        val future = MediaController.Builder(ctx, token).buildAsync()
        controllerFuture = future
        future.addListener({
            controller = try { future.get() } catch (_: Exception) { null }
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

    /** From the start of the song, also for server-made streams (the service's own seek command). */
    fun seek(ms: Long) {
        val c = controller ?: return
        val args = Bundle().apply { putLong(EncorePlaybackService.ARG_POSITION_MS, ms) }
        c.sendCustomCommand(SessionCommand(EncorePlaybackService.CMD_SEEK, Bundle.EMPTY), args)
    }

    companion object {
        /** MapWithContentTemplate needs car API level 7 (car-app library 1.7); the manifest asks for it too. */
        const val MIN_API = 7
    }
}

/** The one screen: lyrics on the map surface, the song and its controls in the content pane. */
class LyricsScreen(
    carContext: CarContext,
    private val surface: LyricsSurface,
    private val session: LyricsCarSession,
) : Screen(carContext) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private val covers = Executors.newSingleThreadExecutor { r -> Thread(r, "car-lyrics-cover").apply { isDaemon = true } }

    /** what the pane shows now: a new template only when it changes (hosts limit template updates) */
    private var shownKey: List<Any?>? = null
    private var np: NowPlaying? = null
    private var coverFor: String? = null
    private var cover: Bitmap? = null

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
                val key = listOf(now?.mediaId, now?.title, now?.artist, now?.album, now?.playing, coverFor == now?.artworkUri?.toString() && cover != null)
                if (now?.artworkUri?.toString() != coverFor) loadCover(now)
                if (key != shownKey) {
                    shownKey = key
                    invalidate()
                }
            }
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
                val b = CoverArt.load(carContext, uri, 256)
                scope.launch {
                    if (coverFor != want || b == null) return@launch
                    cover = b
                    shownKey = null
                    invalidate()
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
            if (now.artist.isNotEmpty()) row.addText(now.artist)
            if (now.album.isNotEmpty()) row.addText(now.album)
            cover?.let { row.setImage(CarIcon.Builder(IconCompat.createWithBitmap(it)).build(), Row.IMAGE_TYPE_LARGE) }
        }
        val pane = Pane.Builder().addRow(row.build()).build()
        @Suppress("DEPRECATION")
        val content = PaneTemplate.Builder(pane)
            .setTitle("Encore Lyrics")
            .setHeaderAction(Action.APP_ICON)
            .build()
        val playing = now?.playing == true
        val strip = ActionStrip.Builder()
            .addAction(iconAction(R.drawable.ic_skip_previous) { session.previous() })
            .addAction(iconAction(if (playing) R.drawable.ic_pause else R.drawable.ic_play) { session.playPause() })
            .addAction(iconAction(R.drawable.ic_skip_next) { session.next() })
            .build()
        // the PAN action makes the host forward drags on the surface (SurfaceCallback.onScroll / onFling)
        val map = MapController.Builder()
            .setMapActionStrip(ActionStrip.Builder().addAction(Action.PAN).build())
            .build()
        return MapWithContentTemplate.Builder()
            .setContentTemplate(content)
            .setMapController(map)
            .setActionStrip(strip)
            .build()
    }

    private fun iconAction(res: Int, onClick: () -> Unit): Action =
        Action.Builder()
            .setIcon(CarIcon.Builder(IconCompat.createWithResource(carContext, res)).build())
            .setOnClickListener { onClick() }
            .build()
}
