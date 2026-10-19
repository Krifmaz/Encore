package app.encore.android.playback

import android.content.Context
import android.os.Handler
import androidx.media3.common.MediaItem
import androidx.media3.exoplayer.ExoPlayer
import app.encore.android.CrashLog
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.data.session.SessionStore
import kotlinx.coroutines.runBlocking
import java.util.concurrent.ExecutorService

/**
 * The queue and spot on this phone ([ResumeStore]): saved while playing (every song change, pause, and every 5 s),
 * put back paused when the service starts (Settings → Playback → "Resume where I left off when the app opens"), and
 * crash recovery: when the process died while playing, the queue comes back at its spot even with the setting off,
 * and plays on when it was playing under 2 minutes ago (once: a song that crashes the app must not loop).
 * Main thread only.
 */
class Resume(
    context: Context,
    private val player: ExoPlayer,
    private val current: CurrentStream,
    private val api: EncoreApi,
    private val session: SessionStore,
    private val io: ExecutorService,
    private val main: Handler,
    private val prefs: () -> PlaybackPrefs,
    private val released: () -> Boolean,
    /** the songs as the service's media items, remembered for the stream resolver */
    private val remember: (List<MediaItem>) -> Unit,
) {
    val store = ResumeStore(context)

    /**
     * The last queue was put back paused at start and hasn't played yet: no now-playing, no autoplay, no song
     * transitions, and saving it doesn't make it newer than another device's spot.
     */
    var held = false
        private set

    /** when the tick last saved the position (epoch ms) */
    private var savedAt = 0L

    fun save() {
        try {
            val tracks = ArrayList<Track>()
            for (i in 0 until player.mediaItemCount) MediaItems.toTrack(player.getMediaItemAt(i))?.let { tracks.add(it) }
            if (tracks.isEmpty()) return
            store.save(tracks, player.currentMediaItemIndex, current.absPositionMs(), touch = !held)
        } catch (_: Exception) {}
    }

    fun savePosition() {
        try {
            if (player.mediaItemCount > 0) store.savePosition(player.currentMediaItemIndex, current.absPositionMs(), touch = !held)
        } catch (_: Exception) {}
    }

    /**
     * Once a second while playing: the spot every 5 s, so it stays newer than another device's even when playback
     * here ends without a pause or a song change (the process killed).
     */
    fun tick() {
        val now = System.currentTimeMillis()
        if (player.isPlaying && now - savedAt >= 5000L) {
            savedAt = now
            savePosition()
        }
    }

    /** The held queue plays now: it counts as playing here from this moment. True when it was held. */
    fun unhold(): Boolean {
        if (!held) return false
        held = false
        return true
    }

    /** Crash recovery: "playing" (or about to, e.g. buffering) survives the process; see [restoreAtStart]. */
    fun markPlaying(playing: Boolean) {
        store.markPlaying(playing)
    }

    /** A clean stop: the spot is saved and there is nothing to recover at the next start. */
    fun stopped() {
        save()
        store.markPlaying(false)
    }

    /**
     * "Resume where I left off when the app opens": the last queue on this phone comes back paused at its song and
     * position (nothing plays until you press play). Off: the player starts empty, unless the process died while
     * playing (crash recovery, above).
     */
    fun restoreAtStart() {
        val interruptedMs = store.takeInterrupted()
        val playOn = interruptedMs != null && interruptedMs < RECOVER_WINDOW_MS &&
            System.currentTimeMillis() - store.recoveredAt > RECOVER_WINDOW_MS
        if (interruptedMs != null) CrashLog.log("playback stopped unexpectedly ${interruptedMs / 1000} s ago: ${if (playOn) "playing on" else "restoring paused"}")
        if (!prefs().resume && interruptedMs == null) return
        io.execute {
            try { runBlocking { session.awaitLoaded() } } catch (_: Exception) {}
            if (session.current == null) return@execute
            val snap = store.load() ?: return@execute
            val items = snap.tracks.map { MediaItems.track(api, it) }
            main.post {
                if (released() || (!prefs().resume && interruptedMs == null) || player.mediaItemCount > 0) return@post
                remember(items)
                if (playOn) {
                    store.recoveredAt = System.currentTimeMillis()
                    held = false
                    player.setMediaItems(items, snap.index, snap.positionMs)
                    player.prepare()
                    player.play()
                    return@post
                }
                held = true
                player.setMediaItems(items, snap.index, snap.positionMs)
            }
        }
    }

    private companion object {
        /** Crash recovery: playing this recently when the process died → it plays on by itself. */
        const val RECOVER_WINDOW_MS = 2 * 60 * 1000L
    }
}
