package app.encore.android.playback

import android.content.Context
import android.net.ConnectivityManager
import android.net.Uri
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.ResolvingDataSource
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.data.api.TrackGain
import app.encore.android.data.prefs.DspInfo
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.data.prefs.Quality
import app.encore.android.features.group.GroupQuality
import java.util.concurrent.ConcurrentHashMap

/** How one song is being streamed: the web player's `state.rung / dsp / transcoded / offset` for one track. */
data class StreamVariant(
    /** kbps of the lossy stream (0 = not lossy) */
    val rung: Int = 0,
    /** upsampled by the server's Cadenza */
    val dsp: Boolean = false,
    /** the rate the server sends when `dsp` (Hz) */
    val dspRate: Int = 0,
    /** FLAC made by the server (formats Android can't decode) */
    val transcode: Boolean = false,
    /** where a server-made stream starts (seconds): its clock starts here, the player's at 0 */
    val offsetSec: Double = 0.0,
    /** what the pre-render cache said about this song: cached | rendering | queued | busy | "" */
    val dspState: String = "",
    /** the song plays from its downloaded copy on this phone (features/offline): that copy's content:// or file:// URI */
    val local: String = "",
) {
    val lossy: Boolean get() = rung > 0

    /** A stream the server makes as it goes (no Range): seeking means asking for it again at `t=`. */
    val serverStream: Boolean get() = lossy || transcode || (dsp && dspState != "cached")

    /** "original" | "flac" | "aac-96" | "cadenza-96000" | "local" – the transport kind the web player reports. */
    val kind: String get() = when {
        local.isNotEmpty() -> "local"
        rung > 0 -> "aac-$rung"
        dsp -> "cadenza-$dspRate"
        transcode -> "flac"
        else -> "original"
    }
}

/** What the resolver needs to know about a song. */
data class StreamTrack(val id: Long, val format: String, val bitrate: Int, val durationSec: Double, val gain: TrackGain? = null) {
    companion object {
        fun of(t: Track): StreamTrack = StreamTrack(t.id, t.format, t.bitrate, t.durationSec, t.gain)
    }
}

/**
 * The bandwidth estimate for Auto quality (player.js `est`): an exponentially weighted average of measured
 * throughput in kbit/s – from ExoPlayer's transfers, from a small Range probe when nothing is downloading, and,
 * until the first sample, the connection's own guess (ConnectivityManager).
 */
class BandwidthEstimator(context: Context) {
    @Volatile var kbps: Double? = null
        private set
    @Volatile var at: Long = 0L
        private set
    /** a new sample arrived since the last decision */
    @Volatile var fresh: Boolean = false
    /** since when the estimate has allowed a higher rung (0 = it doesn't) */
    @Volatile var upSince: Long = 0L
    @Volatile var probing: Boolean = false
    @Volatile var probeAt: Long = 0L

    init {
        try {
            val cm = context.applicationContext.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
            val caps = cm?.activeNetwork?.let { cm.getNetworkCapabilities(it) }
            val down = caps?.linkDownstreamBandwidthKbps ?: 0
            if (down > 0) {
                kbps = down.toDouble()
                at = System.currentTimeMillis()
            }
        } catch (_: Exception) {}
    }

    fun addSample(sample: Double) {
        if (sample < 0 || sample.isNaN() || sample.isInfinite()) return
        kbps = Quality.ewma(kbps, sample)
        at = System.currentTimeMillis()
        fresh = true
    }

    /** "at least this fast": a probe that finished too quickly to time may lift the estimate, never lower it. */
    fun atLeast(sample: Double) {
        val k = kbps
        if (k == null || k < sample) addSample(sample)
    }

    /** A stall: the estimate drops to 60 % of what the rung needed. */
    fun dropTo(value: Double) {
        kbps = value
        at = System.currentTimeMillis()
        fresh = false
        upSince = 0L
    }
}

/**
 * Builds each song's stream URL the moment ExoPlayer opens it (PLAN §6.2), so a pre-loaded next song gets the
 * quality current at that moment: the original file, `transcode=1` for formats Android can't decode, the lossy
 * rungs `q=aac&kbps=N` (Auto / Data saver), or `dsp=1&rate=&filter=&dither=` for the server's Cadenza. The
 * service asks it which variant a song got (the quality badge, the signal path) and tells it when a song must be
 * re-opened at a position on another rung.
 */
class StreamResolver(private val api: EncoreApi, private val estimator: BandwidthEstimator) : ResolvingDataSource.Resolver {
    @Volatile var prefs: PlaybackPrefs = PlaybackPrefs()
    @Volatile var dspInfo: DspInfo? = null
    /** a processed stream failed this session: play originals until the app restarts (player.js dspFailed) */
    @Volatile var dspFailed: Boolean = false
    /** this device's output sample rate, for `dsp_rate = "device"` */
    @Volatile var deviceRate: Int = 0

    /** track id → what the pre-render cache said (prepare) */
    val dspStates = ConcurrentHashMap<Long, String>()
    /** track id → the song's data as last queued */
    private val tracks = ConcurrentHashMap<Long, StreamTrack>()
    /** track id → the variant it is playing / pre-loading with */
    private val chosen = ConcurrentHashMap<Long, StreamVariant>()
    /** track id → the variant its next open must use (a switch or a seek through the server) */
    private val pending = ConcurrentHashMap<Long, StreamVariant>()

    fun remember(t: StreamTrack) { tracks[t.id] = t }

    /**
     * The downloaded copy of a song (features/offline/OfflineStore): its URI, null when it has none or should stream
     * ("Play downloaded songs from this device" off while the server can be reached). A song with a copy plays from
     * it: no stream, no quality steps, no server processing.
     */
    @Volatile var localCopy: (Long) -> String? = { null }

    /** songs whose downloaded copy would not play this session: they stream from the server */
    private val brokenCopies: MutableSet<Long> = ConcurrentHashMap.newKeySet()

    /** The downloaded copy of a song would not play: this session it streams from the server. */
    fun copyFailed(id: Long) { brokenCopies.add(id) }

    private fun localOf(id: Long): String? = if (id in brokenCopies) null else try { localCopy(id) } catch (_: Exception) { null }

    fun track(id: Long): StreamTrack? = tracks[id]

    fun variantOf(id: Long): StreamVariant? = chosen[id]

    /** The song whose data the player opened last (NONE: something that is not a library stream, e.g. an offline file). */
    @Volatile private var lastOpened: Long = NONE

    /**
     * The song the sink is about to be configured for (the one opened last, see [serverRenderAt]), for its volume
     * normalization gain; null when that is not a library song.
     */
    fun lastOpenedTrack(): StreamTrack? {
        val id = lastOpened
        return if (id == NONE) null else tracks[id]
    }

    /**
     * The song the sink is about to be configured for at `rate` Hz is the server's Cadenza render, so the phone's
     * Cadenza leaves it alone (server-side processing wins per song). The sink configures a song right after its data
     * was opened: at the start of a song, and during a gapless hand-over once the next song is pre-loading (by then
     * the current song is fully loaded; a seek that reopens it drops and later reopens the next one too). So the
     * song opened last is the one being configured, and a render still playing does not stop the phone upsampling
     * the next song at the same rate. When that song is no longer known, any render at `rate` counts.
     */
    fun serverRenderAt(rate: Int): Boolean {
        val id = lastOpened
        if (id == NONE) return false
        val v = chosen[id] ?: return chosen.values.any { it.dsp && it.dspRate == rate }
        return v.dsp && v.dspRate == rate
    }

    /** The next open of this song uses `v` (and keeps it until the song ends). */
    fun force(id: Long, v: StreamVariant) {
        pending[id] = v
        chosen[id] = v
    }

    /** The song finished or left the queue: the next time it plays it starts afresh. */
    fun forget(id: Long) {
        chosen.remove(id)
        pending.remove(id)
    }

    fun forgetAll() {
        chosen.clear()
        pending.clear()
    }

    // ---- the policy (player.js rungFor / startUrl / wantDsp) -----------------------------------------------

    /** The rate asked of the server's Cadenza: this device's output rate, 0 (= the highest the server allows) or a fixed rate. */
    fun dspRequestRate(): Int = when (prefs.dspRate) {
        "device" -> deviceRate.coerceAtLeast(0)
        "auto" -> 0
        else -> prefs.dspRate.toIntOrNull() ?: 0
    }

    /**
     * The rate the server will send (its rule: at most max_rate, never below the file's; 0 → the highest multiple of
     * the file's rate). The phone does not know the file's rate before it plays, so it counts with 44.1 kHz: the
     * badge and the Auto ladder can be low for a hi-res file under server processing.
     */
    fun dspOutRate(): Int {
        val src = SOURCE_RATE
        val max = dspInfo?.maxRate?.takeIf { it > 0 } ?: 192000
        val want = dspRequestRate()
        var r = if (want > 0) minOf(want, max) else src
        if (want <= 0) while (r * 2 <= max) r *= 2
        return maxOf(r, src)
    }

    /** Upsample this song on the server? Only when switched on and allowed; Data saver and Auto's lighter streams win. */
    fun wantDsp(lossy: Boolean): Boolean = prefs.dsp && dspInfo?.allowed == true && !lossy && !dspFailed

    val dspCacheOn: Boolean get() = dspInfo?.cacheOn == true

    /** kbit/s of a Cadenza stream at `rate` (24-bit stereo FLAC, roughly 65 % of the raw PCM). */
    private fun dspKbps(rate: Int): Int = Math.round(rate * 48 * 0.65 / 1000).toInt()

    /** kbit/s of the file itself (0 = unknown, counted as 1411). */
    fun fileKbps(t: StreamTrack): Int = if (t.bitrate > 0) t.bitrate else 0

    /** kbit/s the song needs as the "original" rung: the Cadenza stream's when the server upsamples it, else the file's. */
    fun origKbps(t: StreamTrack): Int = if (wantDsp(false)) dspKbps(dspOutRate()) else fileKbps(t)

    /** set by the service: in a group listen now (any thread) */
    @Volatile var grouped: () -> Boolean = { false }

    /**
     * Which rung a song starts on: Original never converts, Data saver is fixed at 96 kbps, Auto follows the estimated
     * bandwidth – in a group listen preferring the original file whenever the estimate covers it ([GroupQuality]).
     */
    fun rungFor(t: StreamTrack): Int = when (prefs.quality) {
        "original" -> 0
        "saver" -> Quality.SAVER_KBPS
        else -> if (grouped()) GroupQuality.startRung(estimator.kbps, origKbps(t)) else Quality.chooseRung(estimator.kbps, origKbps(t))
    }

    /** The variant a song starts with. */
    fun startVariant(t: StreamTrack): StreamVariant {
        localOf(t.id)?.let { return StreamVariant(local = it) }
        val rung = rungFor(t)
        val dsp = wantDsp(rung > 0)
        return StreamVariant(
            rung = rung,
            dsp = dsp,
            dspRate = if (dsp) dspOutRate() else 0,
            transcode = rung == 0 && !dsp && EncoreApi.needsTranscode(t.format),
            dspState = if (dsp) (dspStates[t.id] ?: "") else "",
        )
    }

    /** The same song again at `offsetSec` on `rung` (null = the rung it has), with or without the server's Cadenza. */
    fun reopenVariant(t: StreamTrack, rung: Int?, dsp: Boolean?, offsetSec: Double): StreamVariant {
        // a downloaded copy is a whole file: the same spot again is a seek in it
        localOf(t.id)?.let { return StreamVariant(local = it, offsetSec = offsetSec) }
        val cur = chosen[t.id] ?: startVariant(t)
        val r = rung ?: cur.rung
        val d = (dsp ?: cur.dsp) && wantDsp(r > 0)
        return StreamVariant(
            rung = r,
            dsp = d,
            dspRate = if (d) dspOutRate() else 0,
            transcode = r == 0 && !d && EncoreApi.needsTranscode(t.format),
            offsetSec = offsetSec,
            dspState = if (d) (dspStates[t.id] ?: "") else "",
        )
    }

    /** The stream URL for a song in a given variant (player.js streamUrl). */
    fun url(id: Long, v: StreamVariant): String {
        if (v.local.isNotEmpty()) return v.local
        val q = ArrayList<String>()
        if (v.rung > 0) {
            q.add("q=aac"); q.add("kbps=${v.rung}")
        } else if (v.dsp) {
            q.add("dsp=1"); q.add("rate=${dspRequestRate()}")
            if (prefs.dspFilter.isNotEmpty()) q.add("filter=${EncoreApi.enc(prefs.dspFilter)}")
            q.add("dither=${EncoreApi.enc(prefs.dspDither)}")
        } else if (v.transcode) q.add("transcode=1")
        // a server-made stream restarts at the seek point; a whole file (the original, a cached render) seeks itself
        if (v.offsetSec > 0 && v.serverStream) q.add("t=" + String.format(java.util.Locale.US, "%.1f", v.offsetSec))
        return api.url("/api/library/tracks/$id/stream") + (if (q.isEmpty()) "" else "?" + q.joinToString("&"))
    }

    // ---- ResolvingDataSource.Resolver ---------------------------------------------------------------------

    override fun resolveDataSpec(dataSpec: DataSpec): DataSpec {
        val id = trackIdOf(dataSpec.uri)
        lastOpened = id ?: NONE
        if (id == null) return dataSpec
        val t = tracks[id] ?: return dataSpec
        val v = pending.remove(id) ?: chosen[id] ?: startVariant(t).also { chosen[id] = it }
        return dataSpec.buildUpon().setUri(Uri.parse(url(id, v))).build()
    }

    companion object {
        private const val NONE = -1L

        /** The source rate the server's output rate is predicted from (CD audio). */
        private const val SOURCE_RATE = 44100

        /** `/api/library/tracks/{id}/stream` → id */
        fun trackIdOf(uri: Uri): Long? {
            val segs = uri.pathSegments
            val i = segs.indexOf("tracks")
            if (i < 0 || i + 2 >= segs.size || segs[i + 2] != "stream") return null
            return segs[i + 1].toLongOrNull()
        }
    }
}
