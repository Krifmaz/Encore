package app.encore.android.playback

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.datasource.DataSourceBitmapLoader
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.DefaultLoadControl
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.analytics.AnalyticsListener
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DefaultAudioSink
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.session.CacheBitmapLoader
import androidx.media3.session.DefaultMediaNotificationProvider
import androidx.media3.session.LibraryResult
import androidx.media3.session.MediaConstants
import androidx.media3.session.MediaLibraryService
import androidx.media3.session.MediaSession
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionResult
import app.encore.android.AppGraph
import app.encore.android.EncoreApp
import app.encore.android.R
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.Track
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.data.prefs.Quality
import app.encore.android.ui.MainActivity
import app.encore.android.ui.components.Toasts
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.SettableFuture
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import okhttp3.Request
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
 *
 * M2 adds what web/js/player.js does around the stream: the quality ladder (Auto / Original / Data saver) with the
 * bandwidth estimate, stalls and the step-down / step-up rules, the server's Cadenza (`dsp=1`, render ahead,
 * fall back to the original), seeking in server-made streams through `t=`, the sound effects as an audio
 * processor, and what the UI shows about it all (quality badge, signal path) as the session's extras.
 */
class EncorePlaybackService : MediaLibraryService() {
    private lateinit var graph: AppGraph
    private lateinit var player: ExoPlayer
    private lateinit var session: MediaLibrarySession
    private lateinit var tree: BrowseTree
    private lateinit var resume: ResumeStore
    private lateinit var resolver: StreamResolver
    private lateinit var estimator: BandwidthEstimator
    private lateinit var effects: EffectsAudioProcessor
    private val io: ExecutorService = Executors.newFixedThreadPool(4)
    private val main = Handler(Looper.getMainLooper())
    private val searches = ConcurrentHashMap<String, List<MediaItem>>()
    private var released = false
    private var prefsJob: Job? = null
    private var dspJob: Job? = null
    private var prefs: PlaybackPrefs = PlaybackPrefs()

    // scrobbling state (main thread only)
    private var announced: Long? = null
    private var logged = false
    private var lastId: Long? = null
    private var lastPositionSec = 0.0
    private var lastDurationSec = 0.0

    // adaptive quality (player.js state.*): main thread only
    private var currentId: Long? = null
    private var switchingTo: Long? = null      // a reopen of the same song is on its way: no new play / now-playing
    private val stalls = ArrayList<Long>()
    private var stallAt = 0L
    private var seekAt = 0L
    private var stepAt = 0L
    private var switchedAt = 0L
    private var slowToastAt = 0L
    private var dspToasted = false
    private var transcodeRetried: Long? = null
    private val dspAsked = ConcurrentHashMap<Long, Long>()

    // autoplay: songs played in this listening session never come back in the shuffle
    private val played = LinkedHashSet<Long>()
    private var fetchingMore = false

    override fun onCreate() {
        super.onCreate()
        graph = (application as EncoreApp).graph
        tree = BrowseTree(graph.api)
        resume = ResumeStore(this)
        estimator = BandwidthEstimator(this)
        resolver = StreamResolver(graph.api, estimator)
        resolver.deviceRate = DeviceOutput.of(this).rateHz
        effects = EffectsAudioProcessor(DspEngines.create())
        prefs = graph.prefs.playback.value
        applyPrefs(prefs)
        resolver.dspInfo = graph.prefs.dsp.value

        // Audio goes through the shared client (auth + User-Agent) but without the JSON cache; the resolver
        // decides the quality of every stream the moment it is opened.
        val mediaHttp = graph.http.newBuilder().cache(null).readTimeout(30, TimeUnit.SECONDS).build()
        val upstream = DefaultDataSource.Factory(this, OkHttpDataSource.Factory(mediaHttp))
        val dataSourceFactory = ResolvingDataSource.Factory(upstream, resolver)
        val audioAttributes = AudioAttributes.Builder()
            .setUsage(C.USAGE_MEDIA)
            .setContentType(C.AUDIO_CONTENT_TYPE_MUSIC)
            .build()
        val fx = effects
        val renderers = object : DefaultRenderersFactory(this) {
            override fun buildAudioSink(context: Context, enableFloatOutput: Boolean, enableAudioTrackPlaybackParams: Boolean): AudioSink? =
                DefaultAudioSink.Builder(context)
                    // the processor chain only runs on the 16-bit path (float output bypasses it in Media3)
                    .setEnableFloatOutput(false)
                    .setEnableAudioTrackPlaybackParams(enableAudioTrackPlaybackParams)
                    .setAudioProcessorChain(DefaultAudioSink.DefaultAudioProcessorChain(fx))
                    .build()
        }
        // pre-load: with it the next song is fetched while this one plays (60 s ahead); without it the buffer
        // stays short so all the bandwidth goes to the song playing (applied when the service starts)
        val loadControl = DefaultLoadControl.Builder()
            .setBufferDurationsMs(if (prefs.preload) 30_000 else 15_000, if (prefs.preload) 60_000 else 15_000, 2_500, 5_000)
            .build()
        player = ExoPlayer.Builder(this, renderers)
            .setMediaSourceFactory(DefaultMediaSourceFactory(dataSourceFactory))
            .setLoadControl(loadControl)
            .setAudioAttributes(audioAttributes, true)
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .build()
        player.addListener(listener)
        player.addAnalyticsListener(analytics)

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

        // the preferences follow the account; the UI changes them through PrefsRepository in this process
        prefsJob = graph.scope.launch { graph.prefs.playback.collect { p -> if (!released) applyPrefs(p) } }
        dspJob = graph.scope.launch { graph.prefs.dsp.collect { d -> if (!released) { resolver.dspInfo = d; publish() } } }
        graph.prefs.sync()
        main.post(adaptTick)
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaLibrarySession = session

    override fun onDestroy() {
        released = true
        main.removeCallbacks(tick)
        main.removeCallbacks(adaptTick)
        prefsJob?.cancel()
        dspJob?.cancel()
        saveResume()
        session.release()
        player.removeListener(listener)
        player.removeAnalyticsListener(analytics)
        player.release()
        effects.release()
        io.shutdownNow()
        super.onDestroy()
    }

    private fun applyPrefs(p: PlaybackPrefs) {
        val before = prefs
        prefs = p
        resolver.prefs = p
        effects.params = p.engineParams()
        if (before.dsp != p.dsp || before.dspFilter != p.dspFilter || before.dspDither != p.dspDither || before.dspRate != p.dspRate) {
            resolver.dspStates.clear()
            dspAsked.clear()
        }
        publish()
    }

    // ---- what the UI knows about the stream (session extras) ---------------------------------------------

    private fun currentTrack(): StreamTrack? {
        val item = player.currentMediaItem ?: return null
        val id = MediaItems.trackId(item.mediaId) ?: return null
        return resolver.track(id) ?: MediaItems.toTrack(item)?.let { streamTrack(it) }
    }

    private fun streamTrack(t: Track): StreamTrack = StreamTrack(t.id, t.format, t.bitrate, t.durationSec)

    private fun currentVariant(): StreamVariant? = currentId?.let { resolver.variantOf(it) }

    /** Where a server-made stream started (a whole file is seeked in the player, so its offset is 0). */
    private fun offsetMs(): Long {
        val v = currentVariant() ?: return 0L
        return if (v.serverStream) Math.round(v.offsetSec * 1000) else 0L
    }

    /** The position in the song, including where a server-made stream started. */
    private fun absPositionMs(): Long = player.currentPosition.coerceAtLeast(0L) + offsetMs()

    private fun durationSec(): Double {
        val item = player.currentMediaItem ?: return 0.0
        val v = currentVariant()
        val known = item.mediaMetadata.extras?.getDouble(MediaItems.EXTRA_DURATION) ?: 0.0
        // a server-made stream reports the length it has made so far, if anything: trust the library
        if (v != null && v.serverStream) return known
        return if (player.duration > 0) player.duration / 1000.0 else known
    }

    /** player.js quality() / qualityTitle() / transport(). */
    private fun describe(t: StreamTrack, v: StreamVariant): StreamInfo {
        val f = t.format.uppercase()
        val khz = Quality.khz(v.dspRate)
        val badge = when {
            v.rung > 0 -> "$f → AAC ${v.rung}k"
            v.dsp -> "$f → Cadenza $khz kHz"
            v.transcode -> "$f → FLAC"
            else -> f + (if (t.bitrate > 0) " ${t.bitrate}k" else "")
        }
        val title = when {
            v.rung > 0 -> if (prefs.quality == "saver") "Data saver: a small 96 kbps stream" else "Adaptive quality: ${v.rung} kbps while the connection can't keep up with the original"
            v.dsp -> "Upsampled on the server by Cadenza (lossless 24-bit FLAC)"
            v.transcode -> "Converted to FLAC for this device (lossless)"
            else -> "Original file, no conversion"
        }
        val note = when (v.dspState) { "cached" -> " (pre-rendered)"; "rendering", "queued" -> " (rendering ahead)"; else -> "" }
        val text = when {
            v.rung > 0 -> "AAC ${v.rung} kbps, lossy " + (if (prefs.quality == "saver") "(Data saver is on)" else "(adaptive – the connection could not keep up)")
            v.dsp -> "Upsampled on the server by Cadenza to 24-bit / $khz kHz (lossless FLAC)$note"
            v.transcode -> "Converted to FLAC by the server (lossless) – this device can't play $f"
            else -> "The original file, byte for byte – nothing was converted or resampled on the way"
        }
        return StreamInfo(
            trackId = t.id,
            qualityBadge = badge,
            qualityTitle = title,
            transportKind = v.kind,
            transportText = text,
            offsetMs = if (v.serverStream) Math.round(v.offsetSec * 1000) else 0L,
            serverStream = v.serverStream,
            connectionKbps = estimator.kbps ?: 0.0,
            effectsEngine = if (effects.bypass) "" else effects.engineSignalPath(),
            processorRate = effects.configuredOutputRate,
        )
    }

    private var lastPublished: StreamInfo? = null

    private fun publish() {
        if (released || !this::session.isInitialized) return
        val t = currentTrack()
        val info = if (t == null) StreamInfo() else describe(t, resolver.variantOf(t.id) ?: resolver.startVariant(t))
        if (info == lastPublished) return
        lastPublished = info
        try { session.setSessionExtras(info.toBundle()) } catch (_: Exception) {}
    }

    // ---- scrobbling: the same rules and endpoints as web/js/player.js ---------------------------------------

    private val listener = object : Player.Listener {
        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
            val id = mediaItem?.let { MediaItems.trackId(it.mediaId) }
            val sameSong = id != null && id == switchingTo
            switchingTo = null
            if (sameSong) {
                // the same song again at another rung / position: nothing starts anew
                currentId = id
                publish()
                return
            }
            // a song that ran to its end (gapless hand-over) counts with its full length
            if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_AUTO || reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT) {
                val prev = lastId
                if (!logged && prev != null && lastDurationSec > 0) logPlay(prev, lastDurationSec)
            }
            val previous = currentId
            if (previous != null && previous != id) resolver.forget(previous)
            logged = false
            lastId = null
            lastPositionSec = 0.0
            lastDurationSec = 0.0
            stalls.clear()
            stepAt = 0L
            currentId = id
            if (mediaItem != null && id != null) {
                MediaItems.toTrack(mediaItem)?.let { resolver.remember(streamTrack(it)) }
                announce(id)
                played.add(id)
                val v = resolver.variantOf(id) ?: resolver.startVariant(resolver.track(id) ?: streamTrack(MediaItems.toTrack(mediaItem)!!)).also { resolver.force(id, it) }
                if (prefs.quality == "auto") noteRungChange(previousRung, v.rung)
                previousRung = v.rung
                if (v.dsp && resolver.dspCacheOn) prepareAhead(id)
                // can't even get going: six seconds in and nothing has played yet → one rung down
                slowStartAt = System.currentTimeMillis()
                main.removeCallbacks(slowStart)
                if (prefs.quality == "auto") main.postDelayed(slowStart, 6000L)
            }
            publish()
            saveResume()
            maybeAutoplay()
        }

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            main.removeCallbacks(tick)
            if (isPlaying) main.post(tick) else saveResumePosition()
        }

        override fun onPlaybackStateChanged(playbackState: Int) {
            if (playbackState == Player.STATE_BUFFERING) stalled()
            if (playbackState == Player.STATE_READY) main.removeCallbacks(slowStart)
            if (playbackState == Player.STATE_ENDED) {
                val id = lastId
                if (!logged && id != null && lastDurationSec > 0) logPlay(id, lastDurationSec)
                logged = true
                maybeAutoplay()
            }
        }

        override fun onPlayerError(error: PlaybackException) {
            val t = currentTrack() ?: return
            val v = resolver.variantOf(t.id) ?: return
            val at = absPositionMs() / 1000.0
            if (v.dsp) {
                // the server's Cadenza is off, busy or failed → the same spot again without processing, and no
                // more processing this session
                resolver.dspFailed = true
                if (!dspToasted) { dspToasted = true; Toasts.show("Server processing isn't available right now – playing the original", "info") }
                reopen(t, rung = v.rung, dsp = false, offsetSec = if (at > 0.5) at else 0.0, prepareFirst = true)
                return
            }
            if (!v.transcode && !v.lossy && transcodeRetried != t.id) {
                // the phone couldn't decode the original (e.g. an odd M4A) → ask the server for FLAC
                transcodeRetried = t.id
                resolver.force(t.id, v.copy(transcode = true, offsetSec = at))
                switchingTo = t.id
                replaceCurrent(prepare = true)
            }
        }

        override fun onPositionDiscontinuity(oldPosition: Player.PositionInfo, newPosition: Player.PositionInfo, reason: Int) {
            if (reason == Player.DISCONTINUITY_REASON_SEEK) seekAt = System.currentTimeMillis()
        }
    }

    private var previousRung = 0
    private var slowStartAt = 0L
    private val slowStart = Runnable {
        if (released) return@Runnable
        if (player.playWhenReady && player.playbackState == Player.STATE_BUFFERING && player.currentPosition < 500 && prefs.quality == "auto") stepDownNow()
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
        val p = absPositionMs() / 1000.0
        val d = durationSec()
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

    private fun announce(id: Long) {
        if (announced == id) return
        announced = id
        io.execute { try { graph.api.nowPlaying(id) } catch (_: Exception) {} }
    }

    // ---- keeping the music going on a weak connection (player.js stalled / stepDownNow / decide) -------------

    private val analytics = object : AnalyticsListener {
        override fun onBandwidthEstimate(eventTime: AnalyticsListener.EventTime, totalLoadTimeMs: Int, totalBytesLoaded: Long, bitrateEstimate: Long) {
            // a live Cadenza render is paced by the server, not the network: its download rate says nothing
            val v = currentVariant()
            if (v != null && v.dsp && v.dspState != "cached") return
            val kbps = Quality.rateOf(totalBytesLoaded, totalLoadTimeMs / 1000.0) ?: return
            if (totalBytesLoaded < 16 * 1024) return
            estimator.addSample(kbps)
        }
    }

    /** The player ran out of buffer mid-song. On Auto quality step the stream down. */
    private fun stalled() {
        val now = System.currentTimeMillis()
        if (!player.playWhenReady || now - seekAt < 3000 || now - switchedAt < 3000 || player.currentPosition < 1000) return
        stallAt = now
        stalls.add(now)
        if (prefs.quality != "auto") return
        // stalling again within 90 s of stepping down: the new rung isn't small enough either
        if (stepAt != 0L && now - stepAt < 90_000) { stepDownNow(); return }
        stalls.removeAll { now - it >= 90_000 }
        if (stalls.size >= 2) { stepDownNow(); return }
        // one long stall counts too: still waiting four seconds later
        main.postDelayed({
            if (!released && player.playWhenReady && player.playbackState == Player.STATE_BUFFERING && stallAt == now) stepDownNow()
        }, 4000L)
    }

    /** Auto: a stall forces one rung down right away, and the estimate drops to 60 % of what the rung needed. */
    private fun stepDownNow() {
        val t = currentTrack() ?: return
        if (prefs.quality != "auto") return
        main.removeCallbacks(slowStart)
        val v = resolver.variantOf(t.id) ?: resolver.startVariant(t)
        val orig = resolver.origKbps(t)
        val lower = Quality.stepDown(v.rung, orig)
        if (lower == v.rung) return   // already at 32 kbps
        estimator.dropTo(0.6 * Quality.needed(v.rung, orig))
        stalls.clear()
        stepAt = System.currentTimeMillis()
        switchRung(t, lower)
    }

    /** Reload the song at the current position on another rung. */
    private fun switchRung(t: StreamTrack, rung: Int) {
        val v = resolver.variantOf(t.id) ?: resolver.startVariant(t)
        noteRungChange(v.rung, rung)
        previousRung = rung
        switchedAt = System.currentTimeMillis()
        seekAt = switchedAt
        reopen(t, rung = rung, dsp = null, offsetSec = absPositionMs() / 1000.0)
    }

    private fun noteRungChange(was: Int, now: Int) {
        if (was == now) return
        if (now > 0 && (was == 0 || now < was)) {
            val t = System.currentTimeMillis()
            if (t - slowToastAt > 60_000) { slowToastAt = t; Toasts.show("Connection is slow – lowering quality to $now kbps to keep playing", "info") }
        } else if (now == 0) Toasts.show("Connection recovered – back to the original file", "ok")
    }

    /**
     * The same song again at `offsetSec`: on `rung` (null = the rung it has), with or without the server's Cadenza.
     * A server-made stream starts at `t=offset` and the player at 0; a whole file is seeked in the player.
     */
    private fun reopen(t: StreamTrack, rung: Int?, dsp: Boolean?, offsetSec: Double, prepareFirst: Boolean = false) {
        val v = resolver.reopenVariant(t, rung, dsp, offsetSec)
        resolver.force(t.id, v)
        switchingTo = t.id
        replaceCurrent(prepare = prepareFirst)
    }

    private fun replaceCurrent(prepare: Boolean) {
        val index = player.currentMediaItemIndex
        val item = player.currentMediaItem ?: return
        val id = MediaItems.trackId(item.mediaId) ?: return
        val v = resolver.variantOf(id) ?: return
        val wasPlaying = player.playWhenReady
        // a different URI, or ExoPlayer keeps the running stream (replaceMediaItem only updates metadata for an
        // unchanged one); the resolver rewrites it anyway, so the nonce never reaches the server
        val base = resolver.url(id, v)
        val fresh = item.buildUpon().setUri(base + (if (base.contains('?')) "&" else "?") + "r=" + System.nanoTime()).build()
        player.replaceMediaItem(index, fresh)
        if (!v.serverStream && v.offsetSec > 0) player.seekTo(index, Math.round(v.offsetSec * 1000))
        if (prepare || player.playbackState == Player.STATE_IDLE) player.prepare()
        player.playWhenReady = wasPlaying
        seekAt = System.currentTimeMillis()
        publish()
    }

    /** A seek asked by the UI in song time: whole files seek in the player, server-made streams restart at the point. */
    private fun seekAbs(positionMs: Long) {
        val t = currentTrack() ?: return
        val v = resolver.variantOf(t.id) ?: resolver.startVariant(t)
        val d = durationSec()
        val sec = (positionMs / 1000.0).coerceAtLeast(0.0).let { if (d > 0) minOf(it, d - 0.25) else it }
        seekAt = System.currentTimeMillis()
        if (!v.serverStream) {
            player.seekTo(Math.round(sec * 1000))
            return
        }
        if (v.dsp && resolver.dspCacheOn && v.dspState != "cached" && System.currentTimeMillis() - (dspAsked[t.id] ?: 0L) > 3000) {
            // maybe the render has finished since: then the player seeks in the pre-rendered file itself
            io.execute {
                val s = prepareDsp(t.id)
                main.post {
                    if (released || currentId != t.id) return@post
                    if (s == "cached" && v.offsetSec == 0.0) player.seekTo(Math.round(sec * 1000))
                    else reopen(t, rung = v.rung, dsp = v.dsp, offsetSec = sec)
                }
            }
            return
        }
        reopen(t, rung = v.rung, dsp = v.dsp, offsetSec = sec)
    }

    /** Every 2 s while something plays: sample, probe when idle, and on Auto follow the estimate. */
    private val adaptTick = object : Runnable {
        override fun run() {
            if (released) return
            try { adapt() } catch (_: Exception) {}
            main.postDelayed(this, 2000L)
        }
    }

    private fun adapt() {
        val t = currentTrack() ?: return
        if (!player.isPlaying) { estimator.upSince = 0L; return }
        if (!player.isLoading) probe(t)
        if (prefs.quality == "auto") decide(t)
        publish()
    }

    /** Probe sizes: 128 KB, and when that arrives in under 0.25 s (too fast to time), 1 MB, then 4 MB. */
    private val probeSizes = listOf(131072L, 1048576L, 4194304L)

    /** Nothing downloading and the estimate is older than 20 s: time a piece of the current or next song. */
    private fun probe(current: StreamTrack) {
        val now = System.currentTimeMillis()
        if (estimator.probing || now - estimator.at < 20_000 || now - estimator.probeAt < 20_000) return
        val nextId = if (player.hasNextMediaItem()) MediaItems.trackId(player.getMediaItemAt(player.nextMediaItemIndex).mediaId) else null
        val candidates = listOfNotNull(current, nextId?.let { resolver.track(it) })
        val t = candidates.firstOrNull { !app.encore.android.data.api.EncoreApi.needsTranscode(it.format) } ?: return
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
        val req = Request.Builder().url(graph.api.url("/api/library/tracks/$id/stream")).header("Range", "bytes=0-${size - 1}").header("Cache-Control", "no-store").build()
        graph.http.newCall(req).execute().use { res ->
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
        val v = resolver.variantOf(t.id) ?: return
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

    // ---- server-side Cadenza: render ahead ----------------------------------------------------------------

    /** Ask the server to render a song ahead (or say how far it is): cached | rendering | queued | busy. Blocking. */
    private fun prepareDsp(id: Long): String {
        dspAsked[id] = System.currentTimeMillis()
        return try {
            val s = graph.api.dspPrepare(id, resolver.dspRequestRate(), prefs.dspFilter, prefs.dspDither)
            resolver.dspStates[id] = s
            s
        } catch (_: Exception) { "" }
    }

    /** The current song started as a processed stream: learn whether it is pre-rendered, and have the next one rendered now. */
    private fun prepareAhead(id: Long) {
        val nextId = if (player.hasNextMediaItem()) MediaItems.trackId(player.getMediaItemAt(player.nextMediaItemIndex).mediaId) else null
        io.execute {
            val s = prepareDsp(id)
            main.post {
                if (released || currentId != id) return@post
                val v = resolver.variantOf(id)
                if (v != null && v.dsp && v.dspState != s) { resolver.force(id, v.copy(dspState = s)); publish() }
            }
            if (nextId != null) {
                val n = resolver.track(nextId)
                if (n != null && resolver.wantDsp(resolver.rungFor(n) > 0)) prepareDsp(nextId)
            }
        }
    }

    // ---- autoplay: when the queue runs out, a shuffle of the whole library ----------------------------------

    private fun maybeAutoplay() {
        if (fetchingMore || !prefs.autoplay) return
        val count = player.mediaItemCount
        if (count == 0 || player.repeatMode != Player.REPEAT_MODE_OFF) return
        if (player.currentMediaItemIndex < count - 1 && player.playbackState != Player.STATE_ENDED) return
        fetchingMore = true
        val exclude = ArrayList<Long>(played)
        for (i in 0 until count) MediaItems.trackId(player.getMediaItemAt(i).mediaId)?.let { exclude.add(it) }
        val cur = currentId
        io.execute {
            var more = try { graph.api.shuffle(50, exclude.distinct()) } catch (_: Exception) { emptyList() }
            var freshRound = false
            if (more.isEmpty() && exclude.isNotEmpty()) {
                // everything has played: start a fresh round
                freshRound = true
                more = try { graph.api.shuffle(50, listOfNotNull(cur)) } catch (_: Exception) { emptyList() }
            }
            main.post {
                if (freshRound) played.clear()
                fetchingMore = false
                if (released || more.isEmpty()) return@post
                val ended = player.playbackState == Player.STATE_ENDED
                for (t in more) resolver.remember(streamTrack(t))
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
            resume.save(tracks, player.currentMediaItemIndex, absPositionMs())
        } catch (_: Exception) {}
    }

    private fun saveResumePosition() {
        try {
            if (player.mediaItemCount > 0) resume.savePosition(player.currentMediaItemIndex, absPositionMs())
        } catch (_: Exception) {}
    }

    // ---- the session -----------------------------------------------------------------------------------------

    private inner class Callback : MediaLibrarySession.Callback {
        /** Android Auto, the system UI and our own app get full control; other apps get what Media3 allows them. */
        override fun onConnect(session: MediaSession, controller: MediaSession.ControllerInfo): MediaSession.ConnectionResult {
            val sessionCommands = MediaSession.ConnectionResult.DEFAULT_SESSION_AND_LIBRARY_COMMANDS.buildUpon()
                .add(SessionCommand(CMD_SEEK, Bundle.EMPTY))
                .build()
            if (controller.isTrusted || controller.packageName in FULL_ACCESS || controller.packageName == packageName) {
                return MediaSession.ConnectionResult.accept(sessionCommands, MediaSession.ConnectionResult.DEFAULT_PLAYER_COMMANDS)
            }
            return MediaSession.ConnectionResult.AcceptedResultBuilder(session).setAvailableSessionCommands(sessionCommands).build()
        }

        override fun onCustomCommand(session: MediaSession, controller: MediaSession.ControllerInfo, customCommand: SessionCommand, args: Bundle): ListenableFuture<SessionResult> {
            if (customCommand.customAction == CMD_SEEK) {
                seekAbs(args.getLong(ARG_POSITION_MS, 0L))
                return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
            }
            return Futures.immediateFuture(SessionResult(SessionResult.RESULT_ERROR_NOT_SUPPORTED))
        }

        /**
         * Always hands out the root, signed in or not: Media3 exempts this call from library-error replication and
         * the legacy stub turns an error here into a refused MediaBrowser connection ("can't use this app right
         * now"). The sign-in message comes from [onGetChildren] on the root instead, which Auto does show.
         */
        override fun onGetLibraryRoot(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, params: LibraryParams?): ListenableFuture<LibraryResult<MediaItem>> {
            val f = SettableFuture.create<LibraryResult<MediaItem>>()
            io.execute {
                try { runBlocking { graph.store.awaitLoaded() } } catch (_: Exception) {}
                f.set(LibraryResult.ofItem(tree.root(), params))
            }
            return f
        }

        override fun onGetChildren(session: MediaLibrarySession, browser: MediaSession.ControllerInfo, parentId: String, page: Int, pageSize: Int, params: LibraryParams?): ListenableFuture<LibraryResult<ImmutableList<MediaItem>>> {
            val f = SettableFuture.create<LibraryResult<ImmutableList<MediaItem>>>()
            io.execute {
                try {
                    runBlocking { graph.store.awaitLoaded() }
                    if (graph.store.current == null) { f.set(signInError()); return@execute }
                    val all = tree.list(parentId)
                    f.set(LibraryResult.ofItemList(ImmutableList.copyOf(pageOf(all, page, pageSize)), params))
                } catch (e: ApiException) {
                    f.set(if (e.code == 401) signInError() else LibraryResult.ofError(LibraryResult.RESULT_ERROR_IO))
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
                rememberAll(out)
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
                    resolver.forgetAll()
                    val single = mediaItems.size == 1 && mediaItems[0].localConfiguration == null
                    if (single && (MediaItems.parentOf(mediaItems[0].mediaId) != null || MediaItems.trackId(mediaItems[0].mediaId) == null)) {
                        val (list, at) = resolveIncoming(mediaItems[0])
                        rememberAll(list)
                        f.set(MediaSession.MediaItemsWithStartPosition(list, at, startPositionMs.coerceAtLeast(0L)))
                    } else {
                        val out = ArrayList<MediaItem>()
                        for (item in mediaItems) {
                            val ready = MediaItems.withUri(graph.api, item)
                            if (ready != null) out.add(ready) else try { out.addAll(resolveIncoming(item).first) } catch (_: Exception) {}
                        }
                        rememberAll(out)
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
                        rememberAll(items)
                        f.set(MediaSession.MediaItemsWithStartPosition(items, snap.index, snap.positionMs))
                    } else if (graph.store.current != null) {
                        val items = graph.api.recentPlays(50).map { MediaItems.track(graph.api, it) }
                        rememberAll(items)
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
     * Not signed in (or the session expired): Android Auto shows this message with a button that opens the app
     * on the phone, instead of a bare "can't use this app right now".
     */
    private fun <T> signInError(): LibraryResult<T> {
        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pi = PendingIntent.getActivity(this, 1, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val extras = Bundle().apply {
            putString(MediaConstants.EXTRAS_KEY_ERROR_RESOLUTION_ACTION_LABEL_COMPAT, "Sign in to Encore on your phone")
            putParcelable(MediaConstants.EXTRAS_KEY_ERROR_RESOLUTION_ACTION_INTENT_COMPAT, pi)
        }
        return LibraryResult.ofError(LibraryResult.RESULT_ERROR_SESSION_AUTHENTICATION_EXPIRED, LibraryParams.Builder().setExtras(extras).build())
    }

    private fun rememberAll(items: List<MediaItem>) {
        for (item in items) MediaItems.toTrack(item)?.let { resolver.remember(streamTrack(it)) }
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

    companion object {
        /** Custom session command: seek in song time (server-made streams restart at `t=`). Args: [ARG_POSITION_MS]. */
        const val CMD_SEEK = "app.encore.android.SEEK"
        const val ARG_POSITION_MS = "position_ms"

        /** Controllers that may play and skip even if Media3 can't verify them as trusted. */
        private val FULL_ACCESS = setOf(
            "com.google.android.projection.gearhead",   // Android Auto
            "com.google.android.autosimulator",          // Desktop Head Unit
            "com.android.systemui",
            "com.google.android.carassistant",
            "com.google.android.googlequicksearchbox",   // Assistant
            "com.google.android.wearable.app",
        )
    }
}
