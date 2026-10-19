package app.encore.android.playback

import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DefaultAudioSink
import androidx.media3.exoplayer.source.MediaSource
import app.encore.android.cadenza.CadenzaEngine
import app.encore.android.core.dbl
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.playback.output.EffectsAudioProcessor
import app.encore.android.playback.output.EffectsChain
import kotlinx.serialization.json.JsonObject
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService

/**
 * Song transitions on the phone (Settings → Playback → Song transitions; web/js/player.js maybeTransition):
 * Crossfade (equal-power, both fade), Overlap (the next song starts at full volume early, only the old one fades out
 * under it) and Smart overlap (the silent ends of both songs are skipped, from GET /api/library/tracks/{id}/edges).
 *
 * The session's player ([main]) always holds the queue and the song in front of you: when the window opens it moves
 * on to the next song at once (so the notification, lock screen, car, remote state, scrobbles and the resume spot
 * are all on the new song), while a second ExoPlayer ([aux]) – prepared a few seconds ahead at that very spot –
 * plays the old song's tail and fades it out. Both volume ramps follow the main player's clock, stepped from a
 * handler every 20 ms (a service is not throttled like a hidden web page); the main player's level, a session event
 * every time it changes, is stepped at most every 100 ms.
 *
 * Not done (falls back to the normal gapless change): bit-perfect USB output, a server-made stream as the old song
 * (it can't be reopened at a spot without a new request), repeat one, a held (restored, not yet played) queue, and
 * songs shorter than twice the transition. The aux player has its own Cadenza engine with the same effects but no
 * phone upsampling (it plays at the song's rate): only the fading-out tail is affected.
 */
class Transitions(
    private val context: Context,
    private val main: ExoPlayer,
    /** the aux player's sources: straight over the network, not through the stream resolver (it must not touch
     *  the resolver's choices or its idea of which song was opened last) */
    private val auxSources: MediaSource.Factory,
    /** the old song as the aux player opens it: its stream URL in the variant it is playing; null = can't */
    private val auxItem: (MediaItem) -> MediaItem?,
    private val api: EncoreApi,
    private val io: ExecutorService,
    private val prefs: () -> PlaybackPrefs,
    /** false while a transition can't apply now (bit-perfect, a server-made stream, a held queue) */
    private val allowed: () -> Boolean,
    /** the playing song's length in seconds (the library's for server streams) */
    private val durationSec: () -> Double,
    /** the main player is about to move on to the next song while the old one fades out (it counts as played to its end) */
    private val onHandOver: (MediaItem) -> Unit = {},
) : Player.Listener {
    private val handler = Handler(Looper.getMainLooper())
    private var aux: ExoPlayer? = null
    private var auxFx: EffectsAudioProcessor? = null

    /** the old song prepared on the aux player at [Plan.windowStartMs], waiting for the window */
    private var plan: Plan? = null
    /** the transition running now */
    private var fade: Fade? = null
    /** our own seek into the next song: the listener must not take it for a skip */
    private var handingOver = false
    private var released = false

    /** track id → (start_s, end_s); a missing key = not asked yet, an entry with end 0 = asked, unknown */
    private val edges = ConcurrentHashMap<Long, Pair<Double, Double>>()

    private class Plan(val item: MediaItem, val itemIndex: Int, val nextIndex: Int, val windowStartMs: Long, val endMs: Long, val lenMs: Long, val fromMs: Long, val mode: String)
    private class Fade(val mode: String, val lenMs: Long, var fromMs: Long, val auxEndMs: Long, val targetId: String, var base: Float, var lastMain: Float) {
        /** the main player's own events for our seek into the next song have come (callbacks may come after seekTo returns) */
        var arrived = false
        var seeked = false
        /** the next song's real start is known (a server-made stream can't seek: it starts at 0, not at fromMs) */
        var settled = false
        /** when the main player's volume was last stepped (it is a session event every time: kept to 10 a second) */
        var mainSetAt = 0L
    }

    /** a transition is running (the main player's volume is ramping) */
    val fading: Boolean get() = fade != null

    /** the level the main player is at, or ramps to during a transition */
    fun volume(): Float = fade?.base ?: main.volume

    init {
        main.addListener(this)
    }

    fun release() {
        released = true
        handler.removeCallbacks(step)
        main.removeListener(this)
        finish(restore = true)
        aux?.release()
        aux = null
        auxFx?.release()
        auxFx = null
    }

    // ---- the clock -----------------------------------------------------------------------------------------

    private val step = object : Runnable {
        override fun run() {
            if (released) return
            try { tick() } catch (_: Exception) { finish(restore = true) }
            if (main.isPlaying || fade != null || plan != null) handler.postDelayed(this, if (fade != null || plan != null) 20L else 250L)
        }
    }

    private fun kick() {
        handler.removeCallbacks(step)
        handler.post(step)
    }

    override fun onIsPlayingChanged(isPlaying: Boolean) {
        // a pause during a transition pauses the old song's tail too
        aux?.let { a -> if (fade != null) a.playWhenReady = main.playWhenReady }
        if (isPlaying) kick()
    }

    override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) {
        aux?.let { a -> if (fade != null) a.playWhenReady = playWhenReady }
    }

    override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
        if (handingOver) return
        val f = fade
        if (f != null && !f.arrived && reason == Player.MEDIA_ITEM_TRANSITION_REASON_SEEK && mediaItem?.mediaId == f.targetId) { f.arrived = true; return }
        // a skip, a new queue or the normal end of the song: whatever was planned or fading stops
        cancelPlan()
        finish(restore = true)
        if (mediaItem != null && prefs().transition == "smart") MediaItems.trackId(mediaItem.mediaId)?.let { edgesOf(it) }
        kick()
    }

    override fun onPositionDiscontinuity(oldPosition: Player.PositionInfo, newPosition: Player.PositionInfo, reason: Int) {
        if (handingOver || reason != Player.DISCONTINUITY_REASON_SEEK) return
        val f = fade
        if (f != null && !f.seeked && newPosition.mediaItem?.mediaId == f.targetId) { f.seeked = true; return }
        // you moved within the song: the window is somewhere else now
        cancelPlan()
        finish(restore = true)
        kick()
    }

    override fun onTimelineChanged(timeline: androidx.media3.common.Timeline, reason: Int) {
        // the queue changed (a song added, moved, removed): plan again with the new next song. Not on source
        // updates (the next song being prepared, a stream's length becoming known): tick() checks the plan anyway
        if (reason == Player.TIMELINE_CHANGE_REASON_PLAYLIST_CHANGED && fade == null && plan != null) cancelPlan()
    }

    // ---- planning and the hand-over ---------------------------------------------------------------------------

    private fun tick() {
        val f = fade
        if (f != null) { stepFade(f); return }
        val p = prefs()
        val mode = p.transition
        if (mode == "off" || !allowed() || main.repeatMode == Player.REPEAT_MODE_ONE || !main.playWhenReady) { cancelPlan(); return }
        val item = main.currentMediaItem ?: run { cancelPlan(); return }
        val nextIndex = main.nextMediaItemIndex
        if (nextIndex == C.INDEX_UNSET) { cancelPlan(); return }
        val next = main.getMediaItemAt(nextIndex)
        val oldId = MediaItems.trackId(item.mediaId) ?: run { cancelPlan(); return }
        val newId = MediaItems.trackId(next.mediaId)
        if (p.crossfadeAlbumGapless) {
            val a = item.mediaMetadata.extras?.getString(MediaItems.EXTRA_ALBUM_ID).orEmpty()
            val b = next.mediaMetadata.extras?.getString(MediaItems.EXTRA_ALBUM_ID).orEmpty()
            if (a.isNotEmpty() && a == b) { cancelPlan(); return }
        }
        val secs = (if (p.crossfade > 0) p.crossfade else PlaybackPrefs.TRANSITION_DEFAULT_S).coerceIn(1, 12)
        val d = durationSec()
        if (d <= 0 || d < 2.0 * secs) { cancelPlan(); return }
        val durMs = Math.round(d * 1000)
        // Smart overlap: the old song's last audible moment and where the next one's sound starts
        var endMs = durMs
        var fromMs = 0L
        if (mode == "smart") {
            edgesOf(oldId)?.let { (_, e) -> if (e > 2.0 * secs && e < d) endMs = Math.round(e * 1000) }
            if (newId != null) edgesOf(newId)?.let { (s, _) -> if (s > 0.05 && s < 30.0) fromMs = Math.round(s * 1000) }
        }
        val lenMs = secs * 1000L
        val windowStart = endMs - lenMs
        val pos = main.currentPosition
        val current = plan
        if (current != null && (current.item.mediaId != item.mediaId || current.nextIndex != nextIndex || current.windowStartMs != windowStart || current.fromMs != fromMs)) cancelPlan()
        if (pos < windowStart - 6000L) return
        if (pos > endMs - 1000L) { cancelPlan(); return }   // too late: the song ends normally
        val pl = plan ?: (prepare(item, main.currentMediaItemIndex, nextIndex, windowStart, endMs, lenMs, fromMs, mode) ?: return).also { plan = it }
        val a = aux ?: return
        // start the tail a moment early: it takes a little while for its sound to come out
        if (pos >= pl.windowStartMs - 120L && a.playbackState == Player.STATE_READY && !a.playWhenReady) {
            a.volume = 0f   // silent until the hand-over, so the old song never plays twice over itself
            a.playWhenReady = true
        }
        // the tail sounds: the main player moves on to the next song now
        if (a.isPlaying && a.currentPosition >= pl.windowStartMs) handOver(pl, a)
        else if (pos >= pl.windowStartMs + 1500L) cancelPlan()   // the tail never got going: the song ends normally
    }

    private fun prepare(item: MediaItem, itemIndex: Int, nextIndex: Int, windowStart: Long, endMs: Long, lenMs: Long, fromMs: Long, mode: String): Plan? {
        val src = auxItem(item) ?: return null
        val a = aux ?: build().also { aux = it }
        auxFx?.params = prefs().engineParams()
        a.volume = 0f
        a.setMediaSource(auxSources.createMediaSource(src), windowStart.coerceAtLeast(0L))
        a.playWhenReady = false
        a.prepare()
        return Plan(item, itemIndex, nextIndex, windowStart, endMs, lenMs, fromMs, mode)
    }

    private fun handOver(pl: Plan, a: ExoPlayer) {
        plan = null
        val base = main.volume
        val f = Fade(pl.mode, pl.lenMs, pl.fromMs, pl.endMs, main.getMediaItemAt(pl.nextIndex).mediaId, base, base)
        fade = f
        // the old song is done as far as plays go (the seek below is not the normal end of a song)
        try { onHandOver(pl.item) } catch (_: Exception) {}
        // the tail takes over the old song's sound, the main player starts the next one at its first level
        a.volume = base
        val (_, gIn) = PlaybackPrefs.transitionGains(f.mode, 0.0, 0.0)
        main.volume = base * gIn
        f.lastMain = main.volume
        handingOver = true
        try {
            main.seekTo(pl.nextIndex, pl.fromMs)
        } finally {
            handingOver = false
        }
        if (!main.playWhenReady) main.playWhenReady = true
    }

    private fun stepFade(f: Fade) {
        val a = aux ?: run { finish(restore = true); return }
        // the volume was changed meanwhile (the slider, another device): that is the new level
        if (Math.abs(main.volume - f.lastMain) > 0.001f) f.base = main.volume.coerceAtLeast(0f)
        // the lead-in seek has landed once the next song plays: a stream that couldn't seek started at 0, so the
        // fade runs from where it really is (the web's fade.from = audio.currentTime)
        if (!f.settled && main.currentMediaItem?.mediaId == f.targetId && main.playbackState == Player.STATE_READY) {
            f.settled = true
            val at = main.currentPosition
            if (Math.abs(at - f.fromMs) > 1000L) f.fromMs = at.coerceAtLeast(0L)
        }
        val elapsedMs = if (f.settled) (main.currentPosition - f.fromMs).coerceAtLeast(0L) else 0L
        val x = (elapsedMs.toDouble() / f.lenMs).coerceIn(0.0, 1.0)
        val (gOut, gIn) = PlaybackPrefs.transitionGains(f.mode, x, elapsedMs / 1000.0)
        val now = android.os.SystemClock.uptimeMillis()
        val target = (f.base * gIn).coerceIn(0f, 1f)
        if (now - f.mainSetAt >= 100L || x >= 1.0) {
            f.mainSetAt = now
            if (Math.abs(main.volume - target) > 0.001f) main.volume = target
            f.lastMain = main.volume
        }
        a.volume = (f.base * gOut).coerceIn(0f, 1f)
        if (x >= 1.0 || a.playbackState == Player.STATE_ENDED || a.currentPosition >= f.auxEndMs) finish(restore = true)
    }

    /** The transition is over (or cut short): the tail stops, the new song plays at its full level. */
    private fun finish(restore: Boolean) {
        val f = fade ?: return
        fade = null
        aux?.let { a ->
            a.playWhenReady = false
            a.stop()
            a.clearMediaItems()
        }
        if (restore) main.volume = f.base
    }

    private fun cancelPlan() {
        if (plan == null) return
        plan = null
        aux?.let { a ->
            a.playWhenReady = false
            a.stop()
            a.clearMediaItems()
        }
    }

    /**
     * The second player: the same streams, the same effects through a second Cadenza engine (effects only: no
     * beforeConfigure, so no upsampling), no audio focus of its own (the main player has it).
     */
    private fun build(): ExoPlayer {
        val fx = EffectsAudioProcessor(CadenzaEngine())
        fx.params = prefs().engineParams()
        auxFx = fx
        val renderers = object : DefaultRenderersFactory(context) {
            override fun buildAudioSink(context: Context, enableFloatOutput: Boolean, enableAudioTrackPlaybackParams: Boolean): AudioSink? =
                DefaultAudioSink.Builder(context)
                    .setEnableFloatOutput(false)
                    .setEnableAudioTrackPlaybackParams(enableAudioTrackPlaybackParams)
                    .setAudioProcessorChain(EffectsChain(fx))
                    .build()
        }
        val attrs = AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_MUSIC).build()
        return ExoPlayer.Builder(context, renderers)
            .setMediaSourceFactory(auxSources)
            .setAudioAttributes(attrs, false)
            .setLooper(Looper.getMainLooper())
            .build()
    }

    // ---- Smart overlap: where a song's sound starts and stops --------------------------------------------------

    /** (start_s, end_s) once the server has answered; null until then (or when it can't tell). */
    private fun edgesOf(id: Long): Pair<Double, Double>? {
        val e = edges[id]
        if (e != null) return if (e.second > 0) e else null
        edges[id] = 0.0 to 0.0
        io.execute {
            try {
                val o = api.get("/api/library/tracks/$id/edges") as? JsonObject ?: return@execute
                val s = o.dbl("start_s")
                val end = o.dbl("end_s")
                if (end > 0) edges[id] = s.coerceAtLeast(0.0) to end
            } catch (_: Exception) {
                // offline or an older server: Smart overlap works like Overlap for this song
            }
        }
        return null
    }
}
