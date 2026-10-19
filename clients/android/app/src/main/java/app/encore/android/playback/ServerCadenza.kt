package app.encore.android.playback

import android.os.Handler
import androidx.media3.exoplayer.ExoPlayer
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.ui.components.Toasts
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService

/**
 * The server's Cadenza (Settings → Playback → Server processing; the stream's `dsp=1`): songs rendered ahead on the
 * server (`POST /api/playback/dsp/prepare`), so the next one starts at once and a seek can land in the pre-rendered
 * file, and the fall-back to the original when the server can't process. Main thread, except [prepare] (blocking,
 * on `io`).
 */
class ServerCadenza(
    private val player: ExoPlayer,
    private val current: CurrentStream,
    private val resolver: StreamResolver,
    private val api: EncoreApi,
    private val io: ExecutorService,
    private val main: Handler,
    private val prefs: () -> PlaybackPrefs,
    private val released: () -> Boolean,
    private val publish: () -> Unit,
) {
    /** track id → when its render state was last asked */
    private val asked = ConcurrentHashMap<Long, Long>()
    private var toasted = false

    /** New playback settings: another filter, dither or rate means other renders. */
    fun prefsChanged(before: PlaybackPrefs, p: PlaybackPrefs) {
        if (before.dsp != p.dsp || before.dspFilter != p.dspFilter || before.dspDither != p.dspDither || before.dspRate != p.dspRate) {
            resolver.dspStates.clear()
            asked.clear()
        }
    }

    /** Ask the server to render a song ahead (or say how far it is): cached | rendering | queued | busy. Blocking. */
    fun prepare(id: Long): String {
        asked[id] = System.currentTimeMillis()
        return try {
            val p = prefs()
            val s = api.dspPrepare(id, resolver.dspRequestRate(), p.dspFilter, p.dspDither)
            resolver.dspStates[id] = s
            s
        } catch (_: Exception) { "" }
    }

    /** The current song started as a processed stream: learn whether it is pre-rendered, and have the next one rendered now. */
    fun prepareAhead(id: Long) {
        val nextId = if (player.hasNextMediaItem()) MediaItems.trackId(player.getMediaItemAt(player.nextMediaItemIndex).mediaId) else null
        io.execute {
            val s = prepare(id)
            main.post {
                if (released() || current.id != id) return@post
                val v = resolver.variantOf(id)
                if (v != null && v.dsp && v.dspState != s) { resolver.force(id, v.copy(dspState = s)); publish() }
            }
            if (nextId != null) {
                val n = resolver.track(nextId)
                if (n != null && resolver.wantDsp(resolver.rungFor(n) > 0)) prepare(nextId)
            }
        }
    }

    /**
     * A processed stream failed (the server's Cadenza is off, busy or broken): no more processing this session, and
     * a note the first time.
     */
    fun failed() {
        resolver.dspFailed = true
        if (!toasted) { toasted = true; Toasts.show("Server processing isn't available right now – playing the original", "info") }
    }

    /**
     * A seek to `sec` in a render that wasn't finished when the song started: maybe it is now, and the player seeks
     * in the pre-rendered file itself; otherwise the stream starts again at the spot. False when the render state was
     * asked under 3 s ago (the caller reopens at once).
     */
    fun seekInRender(t: StreamTrack, v: StreamVariant, sec: Double): Boolean {
        if (!(v.dsp && resolver.dspCacheOn && v.dspState != "cached" && System.currentTimeMillis() - (asked[t.id] ?: 0L) > 3000)) return false
        io.execute {
            val s = prepare(t.id)
            main.post {
                if (released() || current.id != t.id) return@post
                if (s == "cached" && v.offsetSec == 0.0) player.seekTo(Math.round(sec * 1000))
                else current.reopen(t, rung = v.rung, dsp = v.dsp, offsetSec = sec)
            }
        }
        return true
    }
}
