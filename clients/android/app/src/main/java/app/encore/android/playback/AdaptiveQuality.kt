package app.encore.android.playback

import android.os.Handler
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.analytics.AnalyticsListener
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.data.prefs.Quality
import app.encore.android.features.group.GroupQuality
import app.encore.android.ui.components.Toasts
import okhttp3.OkHttpClient
import okhttp3.Request
import java.util.concurrent.ExecutorService

/**
 * Keeping the music going on a weak connection (Settings → Playback → Quality "Auto"; web player `stalled` /
 * `stepDownNow` / `decide`): the bandwidth estimate from ExoPlayer's transfers and from timed probes while nothing
 * downloads, a stall or a slow start steps the stream one rung down at once, and the estimate holding 1.5× a higher
 * rung's need for 15 s steps it back up. A switch reopens the song at its spot ([CurrentStream.reopen]).
 *
 * In a group listen the rung is fixed for the length of a song ([GroupQuality]): the estimate changes nothing mid-song
 * (the next song starts on what it says), and only a real stall drops the rung, once per song, reopened where the
 * group will be ([reloadAt]). Every reload there was a gap and then a correction seek.
 *
 * Main thread only (the probes run on `io`).
 */
class AdaptiveQuality(
    private val player: ExoPlayer,
    private val current: CurrentStream,
    private val resolver: StreamResolver,
    private val estimator: BandwidthEstimator,
    private val api: EncoreApi,
    private val http: OkHttpClient,
    private val io: ExecutorService,
    private val main: Handler,
    private val prefs: () -> PlaybackPrefs,
    private val released: () -> Boolean,
    private val publish: () -> Unit,
) {
    private val stalls = ArrayList<Long>()
    private var stallAt = 0L
    private var seekAt = 0L
    private var stepAt = 0L
    private var switchedAt = 0L
    private var slowToastAt = 0L
    private var previousRung = 0
    /** in a group listen this song already dropped a rung for a stall (the one drop a song gets) */
    private var droppedThisSong = false

    /** set by the service: in a group listen now */
    var inGroup: () -> Boolean = { false }
    /**
     * set by the service: in a playing group, where to reopen the song playing (ms, song time) so it lands where the
     * group will be, for a stream of that kind (true: server-made); null: at its own spot
     */
    var reloadAt: (Boolean) -> Long? = { null }

    /** ExoPlayer's transfers feed the estimate. */
    val analytics = object : AnalyticsListener {
        override fun onBandwidthEstimate(eventTime: AnalyticsListener.EventTime, totalLoadTimeMs: Int, totalBytesLoaded: Long, bitrateEstimate: Long) {
            // a live Cadenza render is paced by the server, not the network: its download rate says nothing
            val v = current.variant()
            if (v != null && v.dsp && v.dspState != "cached") return
            val kbps = Quality.rateOf(totalBytesLoaded, totalLoadTimeMs / 1000.0) ?: return
            if (totalBytesLoaded < 16 * 1024) return
            estimator.addSample(kbps)
        }
    }

    private val auto: Boolean get() = prefs().quality == "auto"

    fun start() {
        main.post(tick)
    }

    fun stop() {
        main.removeCallbacks(tick)
        main.removeCallbacks(slowStart)
    }

    /** A seek (yours, or a reopen): the buffering it causes is no stall. */
    fun noteSeek() {
        seekAt = System.currentTimeMillis()
    }

    /** Another song: its stalls start from nothing. */
    fun songChanged() {
        stalls.clear()
        stepAt = 0L
        droppedThisSong = false
    }

    /**
     * A song starts on rung `v.rung`: say so when Auto went down (or back up) since the last one. A downloaded copy
     * says nothing (the connection did not change) and counts as the original file, as on the web.
     */
    fun songStarted(v: StreamVariant) {
        if (auto && v.local.isEmpty()) noteRungChange(previousRung, v.rung)
        previousRung = v.rung
    }

    /** Can't even get going: six seconds in and nothing has played yet → one rung down. */
    fun watchStart() {
        main.removeCallbacks(slowStart)
        if (auto) main.postDelayed(slowStart, 6000L)
    }

    /** The player has enough to play. */
    fun ready() {
        main.removeCallbacks(slowStart)
    }

    private val slowStart = Runnable {
        if (released()) return@Runnable
        if (player.playWhenReady && player.playbackState == Player.STATE_BUFFERING && player.currentPosition < 500 && auto) stepDownNow()
    }

    /** The player ran out of buffer mid-song. On Auto quality step the stream down. */
    fun stalled() {
        val now = System.currentTimeMillis()
        if (!player.playWhenReady || now - seekAt < 3000 || now - switchedAt < 3000 || player.currentPosition < 1000) return
        stallAt = now
        stalls.add(now)
        if (!auto) return
        // stalling again within 90 s of stepping down: the new rung isn't small enough either
        if (stepAt != 0L && now - stepAt < 90_000) { stepDownNow(); return }
        stalls.removeAll { now - it >= 90_000 }
        if (stalls.size >= 2) { stepDownNow(); return }
        // one long stall counts too: still waiting four seconds later
        main.postDelayed({
            if (!released() && player.playWhenReady && player.playbackState == Player.STATE_BUFFERING && stallAt == now) stepDownNow()
        }, 4000L)
    }

    /**
     * Auto: a stall forces one rung down right away, and the estimate drops to 60 % of what the rung needed. In a group
     * once per song ([GroupQuality.mayChange]).
     */
    private fun stepDownNow() {
        val t = current.track() ?: return
        if (!auto) return
        if (!GroupQuality.mayChange(inGroup(), stall = true, droppedThisSong = droppedThisSong)) return
        main.removeCallbacks(slowStart)
        val v = resolver.variantOf(t.id) ?: resolver.startVariant(t)
        // a downloaded copy plays from the phone: nothing to step down
        if (v.local.isNotEmpty()) return
        val orig = resolver.origKbps(t)
        val lower = Quality.stepDown(v.rung, orig)
        if (lower == v.rung) return   // already at 32 kbps
        estimator.dropTo(0.6 * Quality.needed(v.rung, orig))
        stalls.clear()
        stepAt = System.currentTimeMillis()
        if (inGroup()) droppedThisSong = true
        switchRung(t, lower)
    }

    /** Reload the song at the current position on another rung (in a playing group: where the group will be). */
    private fun switchRung(t: StreamTrack, rung: Int) {
        val v = resolver.variantOf(t.id) ?: resolver.startVariant(t)
        noteRungChange(v.rung, rung)
        previousRung = rung
        switchedAt = System.currentTimeMillis()
        seekAt = switchedAt
        val at = reloadAt(resolver.reopenVariant(t, rung, null, 0.0).serverStream) ?: current.absPositionMs()
        current.reopen(t, rung = rung, dsp = null, offsetSec = at / 1000.0)
    }

    private fun noteRungChange(was: Int, now: Int) {
        if (was == now) return
        if (now > 0 && (was == 0 || now < was)) {
            val t = System.currentTimeMillis()
            if (t - slowToastAt > 60_000) { slowToastAt = t; Toasts.show("Connection is slow – lowering quality to $now kbps to keep playing", "info") }
        } else if (now == 0) Toasts.show("Connection recovered – back to the original file", "ok")
    }

    /** Every 2 s while something plays: sample, probe when idle, and on Auto follow the estimate. */
    private val tick = object : Runnable {
        override fun run() {
            if (released()) return
            try { adapt() } catch (_: Exception) {}
            main.postDelayed(this, 2000L)
        }
    }

    private fun adapt() {
        val t = current.track() ?: return
        if (!player.isPlaying) { estimator.upSince = 0L; return }
        // a downloaded copy uses no bandwidth: no probes, nothing to decide (the web's adapt_tick)
        if (resolver.variantOf(t.id)?.local?.isNotEmpty() == true) { estimator.upSince = 0L; publish(); return }
        if (!player.isLoading) probe(t)
        if (auto) decide(t)
        publish()
    }

    /** Probe sizes: 128 KB, and when that arrives in under 0.25 s (too fast to time), 1 MB, then 4 MB. */
    private val probeSizes = listOf(131072L, 1048576L, 4194304L)

    /** Nothing downloading and the estimate is older than 20 s: time a piece of the current or next song. */
    private fun probe(t0: StreamTrack) {
        val now = System.currentTimeMillis()
        if (estimator.probing || now - estimator.at < 20_000 || now - estimator.probeAt < 20_000) return
        val nextId = if (player.hasNextMediaItem()) MediaItems.trackId(player.getMediaItemAt(player.nextMediaItemIndex).mediaId) else null
        val candidates = listOfNotNull(t0, nextId?.let { resolver.track(it) })
        val t = candidates.firstOrNull { !EncoreApi.needsTranscode(it.format) } ?: return
        estimator.probing = true
        estimator.probeAt = now
        io.execute {
            try {
                for (size in probeSizes) {
                    val p = probeOnce(t.id, size) ?: return@execute
                    val kbps = Quality.rateOf(p.first, p.second)
                    if (kbps != null) { estimator.addSample(kbps); return@execute }
                    if (p.first < size || size == probeSizes.last()) {
                        // still under 0.25 s (the whole song, or 4 MB): "at least this fast"
                        estimator.atLeast(p.first * 8.0 / 1000.0 / 0.25)
                        return@execute
                    }
                }
            } catch (_: Exception) {
                // offline: try again later
            } finally {
                estimator.probing = false
            }
        }
    }

    /** Fetch the first `size` bytes of a song, timed: (bytes, seconds), or null when the server refuses. */
    private fun probeOnce(id: Long, size: Long): Pair<Long, Double>? {
        val t0 = System.nanoTime()
        val req = Request.Builder().url(api.url("/api/library/tracks/$id/stream")).header("Range", "bytes=0-${size - 1}").header("Cache-Control", "no-store").build()
        http.newCall(req).execute().use { res ->
            if (res.code != 206 && res.code != 200) return null
            var bytes = 0L
            val buf = ByteArray(64 * 1024)
            res.body.byteStream().use { s ->
                while (bytes < size) {
                    val n = s.read(buf)
                    if (n < 0) break
                    bytes += n
                }
            }
            return minOf(bytes, size) to (System.nanoTime() - t0) / 1e9
        }
    }

    /** Auto: follow the estimate – down at once (when the buffer is short), up when it has held 1.5× the higher rung's need for 15 s. */
    private fun decide(t: StreamTrack) {
        // in a group the rung stays for the song: the next one starts on what the estimate says
        if (!GroupQuality.mayChange(inGroup(), stall = false, droppedThisSong = droppedThisSong)) return
        val v = resolver.variantOf(t.id) ?: return
        if (v.local.isNotEmpty()) return
        val orig = resolver.origKbps(t)
        val l = Quality.ladder(orig)
        val ci = l.indexOf(v.rung)
        val fresh = estimator.fresh
        estimator.fresh = false
        val est = estimator.kbps ?: return
        if (ci < 0) return
        val now = System.currentTimeMillis()
        val buffered = player.totalBufferedDuration / 1000.0
        val ti = l.indexOf(Quality.chooseRung(est, orig))
        if (ti > ci) {
            estimator.upSince = 0L
            if (fresh && now - switchedAt > 10_000 && buffered < 20) switchRung(t, l[ti])
            return
        }
        val up = l.firstOrNull { r -> Quality.needed(r, orig) * 1.5 <= est }
        if (up == null || l.indexOf(up) >= ci) { estimator.upSince = 0L; return }
        if (estimator.upSince == 0L) { estimator.upSince = now; return }
        // safe to reload: 10 s in hand, or a server-made stream the player has finished fetching
        val safe = buffered > 10 || (v.serverStream && !player.isLoading)
        if (now - estimator.upSince >= 15_000 && safe) { estimator.upSince = 0L; switchRung(t, up) }
    }
}
