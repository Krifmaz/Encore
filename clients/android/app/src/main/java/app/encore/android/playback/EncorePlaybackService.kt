package app.encore.android.playback

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.util.StuckPlayerException
import androidx.media3.datasource.DataSourceBitmapLoader
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.HttpDataSource
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.datasource.okhttp.OkHttpDataSource
import androidx.media3.exoplayer.DefaultLoadControl
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.ExoPlayer
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
import app.encore.android.CrashLog
import app.encore.android.EncoreApp
import app.encore.android.R
import app.encore.android.cadenza.CadenzaEngine
import app.encore.android.data.api.ApiException
import app.encore.android.data.prefs.NormPrefs
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.data.prefs.Quality
import app.encore.android.features.group.GroupListen
import app.encore.android.features.group.GroupRules
import app.encore.android.features.group.GroupSync
import app.encore.android.features.offline.OfflineMode
import app.encore.android.features.offline.OfflineStore
import app.encore.android.playback.output.EffectsAudioProcessor
import app.encore.android.playback.output.EffectsChain
import app.encore.android.playback.output.OutputRouter
import app.encore.android.remote.RemoteDevice
import app.encore.android.remote.ServiceHost
import app.encore.android.ui.MainActivity
import com.google.common.collect.ImmutableList
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import com.google.common.util.concurrent.MoreExecutors
import com.google.common.util.concurrent.SettableFuture
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import java.io.File
import java.io.IOException
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * The one playback service: ExoPlayer + a Media3 library session. The phone UI connects with a MediaController; the
 * notification, lock screen, Bluetooth and Android Auto talk to the same session.
 *
 * This file is the wiring: the player and its audio path (the stream resolver, the effects processor, song
 * transitions), the session and its callback (Android Auto's browse tree, voice search, custom commands, playback
 * resumption), the session's extras the UI reads ([StreamInfo]), and the player events handed to the parts that
 * act on them:
 *
 * - [Scrobbler]: now playing and plays, the web player's rules
 * - [AdaptiveQuality]: Auto quality on a weak connection (stalls, probes, step down / up)
 * - [ServerCadenza]: the server's Cadenza (render ahead, fall back to the original)
 * - [Autoplay]: a shuffle of the library when the queue ends
 * - [Resume]: the queue and spot on this phone, and crash recovery
 * - [OutputRouter] (output/): Cadenza on this phone and bit-perfect USB output
 * - [CurrentStream]: the playing song in song time, and reopening it at a spot on another variant
 */
class EncorePlaybackService : MediaLibraryService() {
    private lateinit var graph: AppGraph
    private lateinit var player: ExoPlayer
    private lateinit var session: MediaLibrarySession
    private lateinit var tree: BrowseTree
    private lateinit var resolver: StreamResolver
    private lateinit var estimator: BandwidthEstimator
    private lateinit var effects: EffectsAudioProcessor
    private lateinit var current: CurrentStream
    private lateinit var output: OutputRouter
    private lateinit var scrobbler: Scrobbler
    private lateinit var adaptive: AdaptiveQuality
    private lateinit var serverCadenza: ServerCadenza
    private lateinit var autoplay: Autoplay
    private lateinit var resume: Resume
    private val io: ExecutorService = Executors.newFixedThreadPool(4)
    private val main = Handler(Looper.getMainLooper())
    private val searches = ConcurrentHashMap<String, List<MediaItem>>()
    private var released = false
    private var prefsJob: Job? = null
    private var dspJob: Job? = null
    private var phoneJob: Job? = null
    private var normJob: Job? = null
    /** volume normalization (Settings → Playback → Sound, this device only: PrefsRepository.norm) */
    @Volatile private var norm: NormPrefs = NormPrefs()
    /** the song the effects processor's gain is for (set at its configure, on the playback thread) */
    @Volatile private var gainTrackId: Long = -1L
    private var prefs: PlaybackPrefs = PlaybackPrefs()

    /** the song the phone couldn't decode and the server was asked to convert (once per song) */
    private var transcodeRetried: Long? = null

    /** the song that last stopped on an error, and how often it was opened again since (PlayerErrors.MAX_REOPENS) */
    private var brokenSong: Long? = null
    private var brokenTries = 0
    /**
     * songs skipped in a row because they would not play, until one plays to its end: once round the queue at most
     * (a song that starts and then stalls does not count as playing, or a stuck output would skip forever)
     */
    private var skipped = 0

    /** crash recovery: the notification goes up in the foreground while the restored queue is still paused */
    private var recovering = false
    /** when crash recovery pressed play (elapsedRealtime), for one more try if Android refuses audio focus */
    private var recoveryPlayedAt = 0L

    /** this phone as an Encore remote device: check-ins and commands from other devices (remote/ServiceHost.kt) */
    private var remoteHost: ServiceHost? = null

    /** like / shuffle / repeat / lyrics buttons and live lyrics for the car, notification and lock screen (CarControls.kt) */
    private lateinit var car: CarControls

    /** crossfade / overlap / smart overlap between songs (Transitions.kt) */
    private var transitions: Transitions? = null

    /** a group listen: the player follows the group (features/group/GroupSync.kt) */
    private var group: GroupSync? = null

    override fun onCreate() {
        super.onCreate()
        graph = (application as EncoreApp).graph
        // songs saved on this phone play from their files, and Android Auto lists them under Downloaded
        val offline = OfflineStore.of(this)
        tree = BrowseTree(graph.api, offline)
        estimator = BandwidthEstimator(this)
        resolver = StreamResolver(graph.api, estimator)
        // "Play downloaded songs from this device" (Settings → This device → Downloads, on by default); without a
        // server to ask a copy always plays
        resolver.localCopy = { id -> if (offline.preferLocal.value || OfflineMode.noServer()) offline.localUri(id) else null }
        // offline mode and the outbox also when only the car is in use (MainActivity never opens)
        OfflineMode.start(this)
        // Cadenza on this phone + bit-perfect USB: the device's own settings (PrefsRepository.phone)
        output = OutputRouter(this, resolver, graph.prefs.phone.value, released = { released }, publish = { publish() })
        // long Cadenza filters are designed once and kept here
        effects = EffectsAudioProcessor(CadenzaEngine(File(cacheDir, "cadenza-fir").path))
        // per song, on the playback thread: the server's render wins over the phone's Cadenza
        effects.beforeConfigure = { inRate -> output.setupFor(inRate) }
        effects.directOutput = { rate, channels, upsampled -> output.directOutput(rate, channels, upsampled) }
        // volume normalization: the gain of the song the sink is configured for (the one the resolver opened last)
        norm = graph.prefs.norm.value
        effects.gainAtConfigure = { configuredGain() }
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
                    // the audio track changes the speed (a group listen nudges it to 0.97 / 1.03 to catch up, the pitch
                    // stays): the processor chain itself keeps 1.0
                    .setEnableAudioOutputPlaybackParameters(true)
                    // only the effects: Media3's silence skipping / speed processors take 16-bit PCM only, and phone
                    // Cadenza and bit-perfect USB hand the sink 24 / 32-bit
                    .setAudioProcessorChain(EffectsChain(fx))
                    .build()
        }
        // pre-load: with it the next song is fetched while this one plays (60 s ahead); without it the buffer
        // stays short so all the bandwidth goes to the song playing (applied when the service starts)
        val loadControl = DefaultLoadControl.Builder()
            .setBufferDurationsMs(if (prefs.preload) 30_000 else 15_000, if (prefs.preload) 60_000 else 15_000, 2_500, 5_000)
            .build()
        player = ExoPlayer.Builder(this, renderers)
            // a lost connection is waited out while buffering instead of stopping the music after 3 tries (PatientLoads)
            .setMediaSourceFactory(DefaultMediaSourceFactory(dataSourceFactory).setLoadErrorHandlingPolicy(PatientLoads()))
            .setLoadControl(loadControl)
            .setAudioAttributes(audioAttributes, true)
            .setHandleAudioBecomingNoisy(true)
            .setWakeMode(C.WAKE_MODE_NETWORK)
            .build()

        // in a group listen the group's queue is the queue: no library shuffle after it
        val prefsNow: () -> PlaybackPrefs = { if (group?.active == true) prefs.copy(autoplay = false) else prefs }
        val gone: () -> Boolean = { released }
        current = CurrentStream(player, resolver)
        scrobbler = Scrobbler(player, current, graph.api, io)
        adaptive = AdaptiveQuality(player, current, resolver, estimator, graph.api, graph.http, io, main, prefsNow, gone) { publish() }
        serverCadenza = ServerCadenza(player, current, resolver, graph.api, io, main, prefsNow, gone) { publish() }
        autoplay = Autoplay(player, current, resolver, graph.api, io, main, prefsNow, gone)
        resume = Resume(this, player, current, graph.api, graph.store, io, main, prefsNow, gone, startPlaying = { playFromBackground() }) { items -> rememberAll(items) }
        // a reopen (another rung, a seek in a server-made stream) is a seek as far as stalls go, and a new variant
        current.onReplaced = { adaptive.noteSeek(); publish() }

        player.addListener(listener)
        player.addAnalyticsListener(adaptive.analytics)
        // song transitions (Settings → Playback): a second player fades the old song's tail out (Transitions.kt)
        transitions = Transitions(
            context = this,
            main = player,
            // the old song's tail is fetched directly at the URL it is playing from: through the resolver it would
            // become the song "opened last" (serverRenderAt) and could get another variant after resolver.forget
            auxSources = DefaultMediaSourceFactory(upstream),
            auxItem = { item -> auxItem(item) },
            api = graph.api,
            io = io,
            prefs = prefsNow,
            // crossfade is off in a group listen (gapless stays)
            allowed = { !resume.held && !output.bitPerfectRoute && current.variant()?.serverStream != true && group?.active != true },
            durationSec = { current.durationSec() },
            // (a song that played to its end also ends a run of skips, see skip)
            onHandOver = { item -> skipped = 0; scrobbler.handedOver(item) },
            // the old song's tail keeps its own normalization gain
            gainFor = { item -> Normalization.decide(norm, MediaItems.toTrack(item)?.gain).linear },
        )

        val notifications = DefaultMediaNotificationProvider.Builder(this).build()
        notifications.setSmallIcon(R.drawable.ic_note)
        setMediaNotificationProvider(notifications)

        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val sessionActivity = PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        car = CarControls(this, graph, player, io) { current.absPositionMs() }
        session = MediaLibrarySession.Builder(this, car.player, Callback())
            .setSessionActivity(sessionActivity)
            // covers are content:// URIs from CoverProvider, read with the default data source
            .setBitmapLoader(CacheBitmapLoader(DataSourceBitmapLoader.Builder(this).build()))
            .build()
        car.attach(session)
        // Home starts with "More by <artist>" for the playing song's artist; the car is told when it changes
        car.onArtist = { a, art ->
            if (tree.playingArtist(a, art)) {
                try { session.notifyChildrenChanged("home", tree.homeSize(), null) } catch (_: Exception) {}
            }
        }
        // an artist page's Popular songs that came after the page was shown: the car asks for it again
        tree.onLate = { id, count ->
            main.post { if (!released) try { session.notifyChildrenChanged(id, count, null) } catch (_: Exception) {} }
        }

        // the preferences follow the account; the UI changes them through PrefsRepository in this process
        prefsJob = graph.scope.launch { graph.prefs.playback.collect { p -> if (!released) applyPrefs(p) } }
        dspJob = graph.scope.launch { graph.prefs.dsp.collect { d -> if (!released) { resolver.dspInfo = d; publish() } } }
        phoneJob = graph.scope.launch { graph.prefs.phone.collect { p -> if (!released) output.phoneChanged(p) } }
        normJob = graph.scope.launch { graph.prefs.norm.collect { n -> if (!released) applyNorm(n) } }
        output.start(player, current)
        graph.prefs.sync()
        adaptive.start()
        CrashLog.log("playback service started")
        resume.restoreAtStart()
        remoteHost = ServiceHost(
            remote = RemoteDevice.get(this),
            // the session's player: shuffle / repeat from other devices are the same the car and the phone use
            player = car.player,
            positionMs = { current.absPositionMs() },
            durationSec = { current.durationSec() },
            seekTo = { ms -> seekAbs(ms) },
            signal = {
                val i = lastPublished
                val out = effects.configuredOutputRate
                Triple(i?.qualityBadge ?: "", i?.transportKind ?: "", if (out > 0) out else output.deviceRate)
            },
            items = { tracks -> tracks.map { MediaItems.track(graph.api, it) }.also { rememberAll(it) } },
            newQueue = { resolver.forgetAll(); car.queueReplaced() },
            volume = { transitions?.volume() ?: player.volume },
            fading = { transitions?.fading == true },
            // normalization for a device controlling this one: the snapshot's `norm` and the `prefs` command (§5.1)
            norm = { graph.prefs.norm.value },
            setNorm = { n -> graph.prefs.setNorm(n) },
            normLine = { current.track()?.let { normRow(it).first } ?: "" },
        ).also { it.start() }
        // a group listen: the player follows the group's queue and clock, and this phone's controls go to the group
        GroupListen.init(this)
        group = GroupSync(
            player = player,
            positionMs = { current.absPositionMs() },
            seekTo = { ms -> seekAbs(ms) },
            items = { tracks -> tracks.map { MediaItems.track(graph.api, it) }.also { rememberAll(it) } },
            newQueue = { resolver.forgetAll(); car.queueReplaced() },
            // the speed stays 1.0 for bit-perfect USB and Cadenza on this phone (seek only)
            speedOk = { !output.bitPerfectRoute && lastPublished?.phoneCadenza != true },
            // the variant the song plays (or will start) with: a server-made stream reopens at t= to seek
            serverMade = { t -> (resolver.variantOf(t.id) ?: resolver.startVariant(resolver.track(t.id) ?: StreamTrack.of(t))).serverStream },
            onActive = { on -> car.setGroup(on) },
        ).also { it.start() }
        GroupListen.restore()
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaLibrarySession = session

    /**
     * Sticky: when the process dies while playing (a crash, or the system closing it), Android starts the service
     * again and Resume.restoreAtStart puts the queue back (playing on when it was playing under 2 minutes ago).
     */
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        super.onStartCommand(intent, flags, startId)
        if (intent == null) CrashLog.log("playback service restarted by the system")
        return START_STICKY
    }

    /**
     * Encore swiped out of the recent apps: the music goes on while it plays or is about to (buffering, or held by a
     * short interruption such as a call). Media3's own rule keeps it only while isPlaying, so a swipe during a second
     * of buffering stopped the music. Paused, ended, failed or never in the foreground: the service stops, as before.
     */
    override fun onTaskRemoved(rootIntent: Intent?) {
        val wanted = player.playWhenReady &&
            (player.playbackState == Player.STATE_READY || player.playbackState == Player.STATE_BUFFERING)
        val keep = wanted && isPlaybackOngoing()
        CrashLog.log("task removed: ${if (keep) "playing on" else "stopping"}")
        if (!keep) pauseAllPlayersAndStopSelf()
    }

    /** Crash recovery ([playFromBackground]) asks for the notification in the foreground while still paused. */
    override fun onUpdateNotificationAsync(session: MediaSession, startInForegroundRequired: Boolean): ListenableFuture<Void?> {
        val f = super.onUpdateNotificationAsync(session, startInForegroundRequired || recovering)
        // the service is in the foreground now: the restored queue plays at once, before a later notification
        // update (a cover that finished loading) can take the paused service out of the foreground again
        if (recovering) f.addListener({ if (recovering && isPlaybackOngoing()) recoveryPlay() }, MoreExecutors.directExecutor())
        return f
    }

    /**
     * Crash recovery plays on by itself, with no screen and nothing bound to the service (a sticky restart). Android
     * 16 refuses audio focus to an app that is neither on screen nor a foreground service, so a plain play() here
     * was turned straight back off, and Media3 shows the notification only for a session something has connected
     * to. So the session is added here, the notification goes up in the foreground while the queue is still paused,
     * and the music starts the moment it is up (after 5 s at most, when the system does not allow it).
     */
    private fun playFromBackground() {
        recovering = true
        addSession(session)
        // already added (the app is open): the notification is asked for again now
        triggerNotificationUpdate()
        main.postDelayed({
            if (recovering && !released) {
                CrashLog.log("recovery: not in the foreground after 5 s")
                recoveryPlay()
            }
        }, 5000L)
    }

    private fun recoveryPlay() {
        if (!recovering || released) return
        recovering = false
        recoveryPlayedAt = SystemClock.elapsedRealtime()
        player.play()
    }

    override fun onDestroy() {
        released = true
        group?.stop()
        group = null
        remoteHost?.stop()
        if (this::car.isInitialized) car.release()
        remoteHost = null
        main.removeCallbacks(tick)
        adaptive.stop()
        prefsJob?.cancel()
        dspJob?.cancel()
        phoneJob?.cancel()
        normJob?.cancel()
        output.stop()
        // a clean stop: the spot is saved and nothing is left to recover at the next start
        resume.stopped()
        CrashLog.log("playback service stopped")
        session.release()
        transitions?.release()
        transitions = null
        player.removeListener(listener)
        player.removeAnalyticsListener(adaptive.analytics)
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
        // (the first call, from onCreate, comes before anything was rendered: nothing to forget yet)
        if (this::serverCadenza.isInitialized) serverCadenza.prefsChanged(before, p)
        publish()
    }

    // ---- volume normalization ------------------------------------------------------------------------------

    /** At the effects processor's configure (playback thread): the gain of the song it is configured for. */
    private fun configuredGain(): Float {
        val t = resolver.lastOpenedTrack()
        gainTrackId = t?.id ?: -1L
        return Normalization.decide(norm, t?.gain).linear
    }

    /**
     * The song in front of you is not the one the processor's gain was chosen for (a very short song let the next one
     * be opened first): its own level, reached with a short ramp.
     */
    private fun gainFollows(id: Long) {
        if (id == gainTrackId) return
        val t = resolver.track(id) ?: return
        gainTrackId = id
        effects.gain = Normalization.decide(norm, t.gain).linear
    }

    /** New normalization settings: the song that plays changes level at once (ramped), the next ones at their start. */
    private fun applyNorm(n: NormPrefs) {
        if (n == norm) return
        norm = n
        val id = current.id
        effects.gain = Normalization.decide(n, id?.let { resolver.track(it) }?.gain).linear
        publish()
    }

    /** The Signal path's "Normalization" row for this song (text, note, dB). */
    private fun normRow(t: StreamTrack): Triple<String, String, Double> {
        if (!norm.on) return Triple("", "", 0.0)
        if (output.bitPerfectNow) return Triple("Off for bit-perfect output", "The DAC gets the samples untouched.", 0.0)
        val d = Normalization.decide(norm, t.gain)
        // every note that applies, joined by a space (client-shapes §5.7); the Signal path adds the gain's source after them
        val notes = if (d.noData) listOf("It plays at its own level until Encore has measured it.") else listOfNotNull(
            if (d.limited) "Lowered to keep the peaks from clipping (the song asked for ${Normalization.dbText(d.requestedDb)})." else null,
            if (d.basis == "track" && norm.mode == "album") "No album gain for this song, so its track gain is used." else null,
            if (d.basis == "album" && norm.mode == "track") "No track gain for this song, so its album gain is used." else null,
        )
        return Triple(d.line(), notes.joinToString(" "), if (d.noData) 0.0 else d.db)
    }

    // ---- what the UI knows about the stream (session extras) ---------------------------------------------

    /** The web player's quality() / qualityTitle() / transport(), and what this phone does with the sound. */
    private fun describe(t: StreamTrack, v: StreamVariant): StreamInfo {
        val f = t.format.uppercase()
        val khz = Quality.khz(v.dspRate)
        // a downloaded copy: the quality it was saved in ("FLAC 16/44.1", "FLAC (converted)")
        val copy = if (v.local.isNotEmpty()) OfflineStore.instance?.record(t.id)?.quality?.takeIf { it.isNotBlank() } ?: f else ""
        val badge = when {
            v.local.isNotEmpty() -> copy
            v.rung > 0 -> "$f → AAC ${v.rung}k"
            v.dsp -> "$f → Cadenza $khz kHz"
            v.transcode -> "$f → FLAC"
            else -> f + (if (t.bitrate > 0) " ${t.bitrate}k" else "")
        }
        val title = when {
            v.local.isNotEmpty() -> "Downloaded copy on this device"
            v.rung > 0 -> if (prefs.quality == "saver") "Data saver: a small 96 kbps stream" else "Adaptive quality: ${v.rung} kbps while the connection can't keep up with the original"
            v.dsp -> "Upsampled on the server by Cadenza (lossless 24-bit FLAC)"
            v.transcode -> "Converted to FLAC for this device (lossless)"
            else -> "Original file, no conversion"
        }
        // phone Cadenza: the engine's output rate differs from the song's (a server render passes through)
        val outRate = effects.configuredOutputRate
        val upsampled = CadenzaEngine.available && !v.dsp && outRate > 0 && effects.configuredInputRate > 0 && outRate != effects.configuredInputRate
        val note = when (v.dspState) { "cached" -> " (pre-rendered)"; "rendering", "queued" -> " (rendering ahead)"; else -> "" }
        val text = when {
            v.local.isNotEmpty() -> "Downloaded copy on this device · $copy"
            v.rung > 0 -> "AAC ${v.rung} kbps, lossy " + (if (prefs.quality == "saver") "(Data saver is on)" else "(adaptive – the connection could not keep up)")
            v.dsp -> "Upsampled on the server by Cadenza to 24-bit / $khz kHz (lossless FLAC)$note"
            v.transcode -> "Converted to FLAC by the server (lossless) – this device can't play $f"
            else -> "The original file, byte for byte – nothing was converted or resampled on the way"
        }
        val nr = normRow(t)
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
            phoneCadenza = upsampled,
            phoneRate = if (upsampled) outRate else 0,
            phoneFilterLabel = if (upsampled) output.filterLabel(effects.configuredFilterId) else "",
            phoneDitherLabel = if (upsampled) output.ditherLabel(effects.configuredDitherId) else "",
            usbBitPerfect = output.bitPerfectNow,
            usbDevice = output.usbDeviceName,
            usbNote = output.usbNote,
            normText = nr.first,
            normNote = nr.second,
            normDb = nr.third,
        )
    }

    private var lastPublished: StreamInfo? = null

    private fun publish() {
        if (released || !this::session.isInitialized) return
        val t = current.track()
        val info = if (t == null) StreamInfo() else describe(t, resolver.variantOf(t.id) ?: resolver.startVariant(t))
        if (info == lastPublished) return
        lastPublished = info
        try { session.setSessionExtras(info.toBundle()) } catch (_: Exception) {}
        // the car's quality badge follows how the song reaches this phone
        car.streamChanged(info)
    }

    // ---- player events -------------------------------------------------------------------------------------

    private val listener = object : Player.Listener {
        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
            val id = mediaItem?.let { MediaItems.trackId(it.mediaId) }
            if (current.arrivedAgain(id)) {
                // the same song again at another rung / position: nothing starts anew
                publish()
                return
            }
            scrobbler.songChanged(reason)
            // the next song gets its own tries (onPlayerError); a song that played to its end ends a run of skips
            brokenSong = null
            if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_AUTO || reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT) skipped = 0
            val previous = current.id
            if (previous != null && previous != id) resolver.forget(previous)
            adaptive.songChanged()
            current.id = id
            if (mediaItem != null && id != null) {
                MediaItems.toTrack(mediaItem)?.let { resolver.remember(StreamTrack.of(it)) }
                gainFollows(id)
                if (!resume.held) scrobbler.announce(id)
                autoplay.played(id)
                val v = resolver.variantOf(id) ?: resolver.startVariant(resolver.track(id) ?: StreamTrack.of(MediaItems.toTrack(mediaItem)!!)).also { resolver.force(id, it) }
                adaptive.songStarted(v)
                if (v.dsp && resolver.dspCacheOn) serverCadenza.prepareAhead(id)
                adaptive.watchStart()
            }
            publish()
            resume.save()
            if (!resume.held) autoplay.maybe()
        }

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            main.removeCallbacks(tick)
            if (isPlaying && resume.unhold()) {
                // the queue put back at start plays now: it counts as playing here from this moment
                current.id?.let { scrobbler.announce(it) }
                resume.save()
                autoplay.maybe()
            }
            if (isPlaying) main.post(tick) else resume.savePosition()
        }

        override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) {
            // crash recovery: "playing" (or about to, e.g. buffering) survives the process; see Resume.restoreAtStart
            resume.markPlaying(playWhenReady && player.playbackState != Player.STATE_ENDED)
            CrashLog.log("play ${if (playWhenReady) "on" else "off"} (reason $reason)")
            // crash recovery's play was refused audio focus at once (the system did not count the service as in
            // the foreground yet): once more, half a second later
            if (!playWhenReady && reason == Player.PLAY_WHEN_READY_CHANGE_REASON_AUDIO_FOCUS_LOSS &&
                recoveryPlayedAt > 0L && SystemClock.elapsedRealtime() - recoveryPlayedAt < 2000L) {
                recoveryPlayedAt = 0L
                CrashLog.log("recovery: no audio focus, trying once more")
                main.postDelayed({ if (!released) player.play() }, 500L)
            }
        }

        override fun onPlaybackStateChanged(playbackState: Int) {
            if (playbackState == Player.STATE_BUFFERING) adaptive.stalled()
            if (playbackState == Player.STATE_READY) adaptive.ready()
            if (playbackState == Player.STATE_ENDED) {
                resume.markPlaying(false)
                scrobbler.ended()
                autoplay.maybe()
            }
        }

        override fun onPlayerError(error: PlaybackException) {
            CrashLog.log("player error ${error.errorCodeName}: ${error.message}")
            val t = current.track() ?: return
            val v = resolver.variantOf(t.id) ?: return
            val at = current.absPositionMs() / 1000.0
            if (brokenSong != t.id) { brokenSong = t.id; brokenTries = 0 }
            // a lost connection used to be taken for "the phone can't decode it" (a FLAC conversion that failed the
            // same way) and anything else stopped the music for good: in the background nobody pressed play again
            val status = (error.cause as? HttpDataSource.InvalidResponseCodeException)?.responseCode ?: 0
            val bufferedNothing = (error.cause as? StuckPlayerException)?.stuckType == StuckPlayerException.STUCK_BUFFERING_NO_PROGRESS
            val action = PlayerErrors.decide(error.errorCode, status, bufferedNothing, v, transcodeRetried == t.id, brokenTries)
            CrashLog.log("song ${t.id}: ${action.name.lowercase()}" + (if (status > 0) " (HTTP $status)" else ""))
            when (action) {
                PlayerErrors.Action.STREAM -> {
                    // the downloaded copy would not play: this session the song streams from the server, from the same spot
                    resolver.copyFailed(t.id)
                    app.encore.android.ui.components.Toasts.show("This download would not play – playing the song from the server", "info")
                    current.reopen(t, rung = null, dsp = null, offsetSec = if (at > 0.5) at else 0.0, prepareFirst = true)
                }
                PlayerErrors.Action.NO_DSP -> {
                    // the server's Cadenza is off, busy or failed → the same spot again without processing, and no
                    // more processing this session
                    serverCadenza.failed()
                    current.reopen(t, rung = v.rung, dsp = false, offsetSec = if (at > 0.5) at else 0.0, prepareFirst = true)
                }
                PlayerErrors.Action.REOPEN -> {
                    // the same spot again at once: the player buffers (holding its wake and Wi-Fi locks) and
                    // ExoPlayer's own waits pace the tries, so no timer has to run while the phone sleeps
                    brokenTries++
                    resolver.force(t.id, v.copy(offsetSec = if (at > 0.5) at else 0.0))
                    current.switchingTo = t.id
                    current.replace(prepare = true)
                }
                PlayerErrors.Action.TRANSCODE -> {
                    // the phone couldn't decode the original (e.g. an odd M4A) → ask the server for FLAC
                    transcodeRetried = t.id
                    resolver.force(t.id, v.copy(transcode = true, offsetSec = at))
                    current.switchingTo = t.id
                    current.replace(prepare = true)
                }
                PlayerErrors.Action.SKIP -> skip(t.id)
                PlayerErrors.Action.STOP -> {}
            }
        }

        override fun onPositionDiscontinuity(oldPosition: Player.PositionInfo, newPosition: Player.PositionInfo, reason: Int) {
            if (reason == Player.DISCONTINUITY_REASON_SEEK) adaptive.noteSeek()
        }
    }

    /** The song won't play: on to the next one (once round the queue at most, then it stays stopped). */
    private fun skip(id: Long) {
        brokenSong = null
        // in a group listen the queue is the group's: this phone does not move on alone
        if (group?.active == true || !player.hasNextMediaItem() || ++skipped > player.mediaItemCount) {
            CrashLog.log("song $id would not play: stopped")
            return
        }
        CrashLog.log("song $id would not play: on to the next one")
        player.seekTo(player.nextMediaItemIndex, 0L)
        player.prepare()
    }

    /** Runs once a second while playing (and only then: no requests while paused). */
    private val tick = object : Runnable {
        override fun run() {
            if (released) return
            scrobbler.check()
            resume.tick()
            if (player.isPlaying) main.postDelayed(this, 1000L)
        }
    }

    /** The old song as the transitions' second player opens it: the concrete URL of the variant it is playing. */
    private fun auxItem(item: MediaItem): MediaItem? {
        val uri = item.localConfiguration?.uri ?: return null
        // not a library stream (an offline file): the same URI
        if (StreamResolver.trackIdOf(uri) == null) return item
        val id = MediaItems.trackId(item.mediaId) ?: return null
        val v = resolver.variantOf(id) ?: return null
        return item.buildUpon().setUri(resolver.url(id, v)).build()
    }

    /** A seek asked by the UI in song time: whole files seek in the player, server-made streams restart at the point. */
    private fun seekAbs(positionMs: Long) {
        val t = current.track() ?: return
        val v = resolver.variantOf(t.id) ?: resolver.startVariant(t)
        val d = current.durationSec()
        val sec = (positionMs / 1000.0).coerceAtLeast(0.0).let { if (d > 0) minOf(it, d - 0.25) else it }
        adaptive.noteSeek()
        if (!v.serverStream) {
            player.seekTo(Math.round(sec * 1000))
            return
        }
        // a render still in the making may have finished since: then the player seeks in the pre-rendered file
        if (serverCadenza.seekInRender(t, v, sec)) return
        current.reopen(t, rung = v.rung, dsp = v.dsp, offsetSec = sec)
    }

    // ---- the session -----------------------------------------------------------------------------------------

    private inner class Callback : MediaLibrarySession.Callback {
        /**
         * Android Auto, the system UI and our own app get full control; other apps get what Media3 allows them. No
         * controller may change the playback speed: the audio track applies it for real (a group listen's catch-up
         * nudge, GroupSync, is the only one that sets it), and the app has no speed setting.
         */
        override fun onConnect(session: MediaSession, controller: MediaSession.ControllerInfo): MediaSession.ConnectionResult {
            val sessionCommands = MediaSession.ConnectionResult.DEFAULT_SESSION_AND_LIBRARY_COMMANDS.buildUpon()
                .add(SessionCommand(CMD_SEEK, Bundle.EMPTY))
                .apply { for (c in CarControls.COMMANDS) add(SessionCommand(c, Bundle.EMPTY)) }
                .build()
            val playerCommands = MediaSession.ConnectionResult.DEFAULT_PLAYER_COMMANDS.buildUpon()
                .remove(Player.COMMAND_SET_SPEED_AND_PITCH)
                .build()
            if (controller.isTrusted || controller.packageName in FULL_ACCESS || controller.packageName == packageName) {
                return MediaSession.ConnectionResult.accept(sessionCommands, playerCommands)
            }
            return MediaSession.ConnectionResult.AcceptedResultBuilder(session, controller)
                .setAvailableSessionCommands(sessionCommands)
                .setAvailablePlayerCommands(playerCommands)
                .build()
        }

        /** Android Auto came or went: the car's button order and the quality on its subtitle line follow (CarControls). */
        override fun onPostConnect(session: MediaSession, controller: MediaSession.ControllerInfo) {
            car.controller(controller.packageName in CarControls.CAR_PACKAGES, connected = true)
        }

        override fun onDisconnected(session: MediaSession, controller: MediaSession.ControllerInfo) {
            car.controller(controller.packageName in CarControls.CAR_PACKAGES, connected = false)
        }

        override fun onCustomCommand(session: MediaSession, controller: MediaSession.ControllerInfo, customCommand: SessionCommand, args: Bundle): ListenableFuture<SessionResult> {
            car.result(customCommand.customAction)?.let { return Futures.immediateFuture(it) }
            if (customCommand.customAction == CMD_SEEK) {
                val ms = args.getLong(ARG_POSITION_MS, 0L)
                val g = group
                if (g != null && g.active) {
                    // in a group listen the seek is the group's (or refused when only the host controls playback)
                    if (GroupListen.deniedHere()) return Futures.immediateFuture(SessionResult(SessionResult.RESULT_SUCCESS))
                    g.userSeek(ms)
                    g.applying { seekAbs(ms) }
                } else {
                    seekAbs(ms)
                }
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
                // Android Auto shows its search button only when the root says search is supported
                val extras = Bundle(params?.extras ?: Bundle.EMPTY).apply { putBoolean(BrowseTree.EXTRAS_SEARCH_SUPPORTED, true) }
                val rootParams = LibraryParams.Builder()
                    .setExtras(extras)
                    .setOffline(params?.isOffline == true)
                    .setRecent(params?.isRecent == true)
                    .setSuggested(params?.isSuggested == true)
                    .build()
                // a browser that asks for what plays offline gets the songs saved on this phone
                f.set(LibraryResult.ofItem(if (params?.isOffline == true) tree.downloadedRoot() else tree.root(), rootParams))
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

        /**
         * Items a controller adds ("play this", "add to queue"): give them their stream URIs. In a group listen they
         * are added to the group's Up next instead ([toGroup]).
         */
        override fun onAddMediaItems(mediaSession: MediaSession, controller: MediaSession.ControllerInfo, mediaItems: MutableList<MediaItem>): ListenableFuture<MutableList<MediaItem>> {
            val f = SettableFuture.create<MutableList<MediaItem>>()
            io.execute {
                val out = ArrayList<MediaItem>()
                for (item in mediaItems) {
                    val ready = MediaItems.withUri(graph.api, item)
                    if (ready != null) { out.add(ready); continue }
                    try { out.addAll(resolveIncoming(item).first) } catch (_: Exception) {}
                }
                if (GroupListen.inGroup) {
                    f.setException(toGroup(out, null))
                    return@execute
                }
                rememberAll(out)
                f.set(out)
            }
            return f
        }

        /**
         * "Play" on a browsed song plays the whole list it came from, starting there; folders play all their songs.
         * In a group listen the list becomes the group's queue instead ([toGroup]); the queue here stays.
         */
        override fun onSetMediaItems(mediaSession: MediaSession, controller: MediaSession.ControllerInfo, mediaItems: MutableList<MediaItem>, startIndex: Int, startPositionMs: Long): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val inGroup = GroupListen.inGroup
            val f = SettableFuture.create<MediaSession.MediaItemsWithStartPosition>()
            io.execute {
                try {
                    runBlocking { graph.store.awaitLoaded() }
                    val single = mediaItems.size == 1 && mediaItems[0].localConfiguration == null
                    val (list, at, pos) = if (single && (MediaItems.parentOf(mediaItems[0].mediaId) != null || MediaItems.trackId(mediaItems[0].mediaId) == null)) {
                        val (list, at) = resolveIncoming(mediaItems[0])
                        Triple(list, at, startPositionMs.coerceAtLeast(0L))
                    } else {
                        val out = ArrayList<MediaItem>()
                        for (item in mediaItems) {
                            val ready = MediaItems.withUri(graph.api, item)
                            if (ready != null) out.add(ready) else try { out.addAll(resolveIncoming(item).first) } catch (_: Exception) {}
                        }
                        Triple(out, if (out.isEmpty()) 0 else startIndex.coerceIn(0, out.size - 1), startPositionMs)
                    }
                    if (inGroup) {
                        f.setException(toGroup(list, at, pos))
                        return@execute
                    }
                    // nothing to play (an empty playlist, a voice request that matched nothing): the request fails
                    // and the queue that is playing stays (an empty queue stopped the music and took the
                    // notification down)
                    if (list.isEmpty()) throw IOException("Nothing to play")
                    resolver.forgetAll()
                    // before the new queue is set: Media3 sets it in a later post on the same main thread
                    main.post { if (!released) car.queueReplaced() }
                    rememberAll(list)
                    f.set(MediaSession.MediaItemsWithStartPosition(list, at, pos))
                } catch (e: Exception) {
                    CrashLog.log("play request from ${controller.packageName} failed: $e")
                    // never the controller's own items: they have no stream address, and for a car's, a Bluetooth
                    // browse list's or the Assistant's request ExoPlayer threw on the main thread and closed the app
                    f.setException(if (inGroup) UnsupportedOperationException(GROUP_QUEUE) else e)
                }
            }
            return f
        }

        /** "Resume" from the lock screen, Bluetooth or the car: the last queue (stored on the phone), or recently played. */
        override fun onPlaybackResumption(mediaSession: MediaSession, controller: MediaSession.ControllerInfo, isForPlayback: Boolean): ListenableFuture<MediaSession.MediaItemsWithStartPosition> {
            val f = SettableFuture.create<MediaSession.MediaItemsWithStartPosition>()
            // in a group listen the group's queue comes from the group (GroupSync), never the one saved here
            if (GroupListen.inGroup) return Futures.immediateFailedFuture(UnsupportedOperationException(GROUP_QUEUE))
            io.execute {
                try {
                    runBlocking { graph.store.awaitLoaded() }
                    val snap = resume.store.load()
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
    private fun <T : Any> signInError(): LibraryResult<T> {
        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pi = PendingIntent.getActivity(this, 1, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val extras = Bundle().apply {
            putString(MediaConstants.EXTRAS_KEY_ERROR_RESOLUTION_ACTION_LABEL_COMPAT, "Sign in to Encore on your phone")
            putParcelable(MediaConstants.EXTRAS_KEY_ERROR_RESOLUTION_ACTION_INTENT_COMPAT, pi)
        }
        return LibraryResult.ofError<T>(LibraryResult.RESULT_ERROR_SESSION_AUTHENTICATION_EXPIRED, LibraryParams.Builder().setExtras(extras).build())
    }

    private fun rememberAll(items: List<MediaItem>) {
        for (item in items) MediaItems.toTrack(item)?.let { resolver.remember(StreamTrack.of(it)) }
    }

    /**
     * In a group listen the queue is the group's: what a controller asks to play or add (Android Auto's browse tree, a
     * voice request, the Encore Lyrics car app) goes to the group, which plays it on every device,
     * or is refused with the reason ("Only Alex controls playback", also on the car's subtitle line). The queue here is
     * never replaced: the returned exception fails the controller's request. [index] null: added to Up next (every
     * member may).
     */
    private fun toGroup(items: List<MediaItem>, index: Int?, positionMs: Long = 0L): Exception {
        val all = items.map { MediaItems.toTrack(it) }
        val tracks = all.filterNotNull()
        val at = if (index == null) 0 else all.take(index).count { it != null }
        main.post {
            if (released) return@post
            val s = GroupListen.state.value ?: return@post
            if (index == null) {
                GroupListen.enqueue(tracks)
            } else {
                if (!s.youCanControl) car.say(GroupRules.onlyHost(s))
                GroupListen.playList(tracks, at, if (positionMs > 0L) positionMs / 1000.0 else 0.0)
            }
        }
        return UnsupportedOperationException(GROUP_QUEUE)
    }

    /**
     * What a controller's item stands for: a browse id or song id (via the tree), or – with an empty id – a voice
     * request ("play <album>", "play <artist>", or just "play music"), answered from the library.
     */
    private fun resolveIncoming(item: MediaItem): Pair<List<MediaItem>, Int> {
        if (item.mediaId.isNotEmpty()) return tree.resolve(item.mediaId)
        return playFromSearch(item.requestMetadata.searchQuery ?: "") to 0
    }

    private fun playFromSearch(query: String): List<MediaItem> =
        try {
            playFromLibrary(query)
        } catch (e: Exception) {
            // the server can't be reached: the songs saved on this phone that match
            tree.downloadedFor(query.trim()).ifEmpty { throw e }
        }

    private fun playFromLibrary(query: String): List<MediaItem> {
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

        /** why a controller's play or add fails in a group listen (only logged by Media3) */
        private const val GROUP_QUEUE = "In a group listen the queue is the group's"

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
