package app.encore.android.playback

import android.content.Context
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.media3.common.ForwardingPlayer
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.session.CommandButton
import androidx.media3.session.MediaSession
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionResult
import app.encore.android.AppGraph
import app.encore.android.car.CarLine
import app.encore.android.car.CarNowPlaying
import app.encore.android.car.LyricsCache
import app.encore.android.car.LyricsCard
import app.encore.android.car.NowPlaying
import app.encore.android.features.group.GroupListen
import app.encore.android.features.group.GroupRules
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * "Live lyrics in the car": device-only settings (SharedPreferences "encore_car"), shared in this process by
 * the playback service and Settings → Playback. On / off (the Lyrics button), and how: a lyrics card as the
 * artwork ([MODE_CARD]), the current line as the title ([MODE_TITLE]) or both ([MODE_WIDE], the default). Also the Car buttons page
 * (CarButtonsScreen): which buttons Android Auto gets in what order ([carButtons]) and the car's number of spots
 * for its preview ([carSpots]), and the Encore Lyrics car app's control strip ([lyricsStrip]).
 */
object CarPrefs {
    const val MODE_CARD = "card"
    const val MODE_TITLE = "title"
    /** Wide: the card as the artwork and the lyrics on the title lines too, so the lyrics run across the player. */
    const val MODE_WIDE = "wide"

    /** A stored or picked mode, one of the three; anything else (nothing stored yet included) is [MODE_WIDE]. */
    fun modeOf(m: String?): String = when (m) {
        MODE_TITLE, MODE_CARD -> m
        else -> MODE_WIDE
    }

    /** Whether [m] draws the lyrics card as the artwork. */
    fun cardMode(m: String): Boolean = m == MODE_CARD || m == MODE_WIDE

    /** Whether [m] puts the line playing on the title. */
    fun titleMode(m: String): Boolean = m == MODE_TITLE || m == MODE_WIDE
    private const val FILE = "encore_car"
    private const val LIVE_LYRICS = "live_lyrics"
    private const val MODE = "lyrics_mode"
    private const val BUTTONS = "buttons"
    private const val SPOTS = "spots"
    private const val LYRICS_STRIP = "lyrics_strip"
    @Volatile private var flow: MutableStateFlow<Boolean>? = null
    @Volatile private var modeFlow: MutableStateFlow<String>? = null
    @Volatile private var buttonsFlow: MutableStateFlow<CarButtonRules.Layout>? = null
    @Volatile private var spotsFlow: MutableStateFlow<Int>? = null
    @Volatile private var stripFlow: MutableStateFlow<List<LyricsStripRules.Btn>>? = null

    /** The Encore Lyrics car app's control strip (LyricsStripRules.encode in the file); an open car app follows at once. */
    fun lyricsStrip(context: Context): StateFlow<List<LyricsStripRules.Btn>> = stripState(context).asStateFlow()

    fun setLyricsStrip(context: Context, strip: List<LyricsStripRules.Btn>) {
        val f = stripState(context)
        if (f.value == strip) return
        f.value = strip
        try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putString(LYRICS_STRIP, LyricsStripRules.encode(strip)).apply()
        } catch (_: Exception) {}
    }

    @Synchronized
    private fun stripState(context: Context): MutableStateFlow<List<LyricsStripRules.Btn>> {
        stripFlow?.let { return it }
        val s = try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getString(LYRICS_STRIP, null)
        } catch (_: Exception) { null }
        return MutableStateFlow(LyricsStripRules.decode(s)).also { stripFlow = it }
    }

    /** The car's buttons in their three groups (CarButtonRules.encode in the file); the service follows changes at once. */
    fun carButtons(context: Context): StateFlow<CarButtonRules.Layout> = buttonsState(context).asStateFlow()

    fun setCarButtons(context: Context, layout: CarButtonRules.Layout) {
        val f = buttonsState(context)
        if (f.value == layout) return
        f.value = layout
        try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putString(BUTTONS, CarButtonRules.encode(layout)).apply()
        } catch (_: Exception) {}
    }

    @Synchronized
    private fun buttonsState(context: Context): MutableStateFlow<CarButtonRules.Layout> {
        buttonsFlow?.let { return it }
        val s = try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getString(BUTTONS, null)
        } catch (_: Exception) { null }
        return MutableStateFlow(CarButtonRules.decode(s)).also { buttonsFlow = it }
    }

    /** How many button spots the listener's car has (only the Car buttons page's preview uses it). */
    fun carSpots(context: Context): StateFlow<Int> = spotsState(context).asStateFlow()

    fun setCarSpots(context: Context, spots: Int) {
        val n = CarButtonRules.spots(spots)
        val f = spotsState(context)
        if (f.value == n) return
        f.value = n
        try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putInt(SPOTS, n).apply()
        } catch (_: Exception) {}
    }

    @Synchronized
    private fun spotsState(context: Context): MutableStateFlow<Int> {
        spotsFlow?.let { return it }
        val n = try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getInt(SPOTS, CarButtonRules.DEFAULT_SPOTS)
        } catch (_: Exception) { CarButtonRules.DEFAULT_SPOTS }
        return MutableStateFlow(CarButtonRules.spots(n)).also { spotsFlow = it }
    }

    /** [MODE_WIDE] (the default), [MODE_CARD] or [MODE_TITLE]. */
    fun lyricsMode(context: Context): StateFlow<String> = modeState(context).asStateFlow()

    fun setLyricsMode(context: Context, mode: String) {
        val m = modeOf(mode)
        val f = modeState(context)
        if (f.value == m) return
        f.value = m
        try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putString(MODE, m).apply()
        } catch (_: Exception) {}
    }

    @Synchronized
    private fun modeState(context: Context): MutableStateFlow<String> {
        modeFlow?.let { return it }
        val m = try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getString(MODE, null)
        } catch (_: Exception) { null }
        return MutableStateFlow(modeOf(m)).also { modeFlow = it }
    }

    fun liveLyrics(context: Context): StateFlow<Boolean> = state(context).asStateFlow()

    fun setLiveLyrics(context: Context, on: Boolean) {
        val f = state(context)
        if (f.value == on) return
        f.value = on
        try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putBoolean(LIVE_LYRICS, on).apply()
        } catch (_: Exception) {}
    }

    @Synchronized
    private fun state(context: Context): MutableStateFlow<Boolean> {
        flow?.let { return it }
        val on = try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getBoolean(LIVE_LYRICS, false)
        } catch (_: Exception) { false }
        return MutableStateFlow(on).also { flow = it }
    }
}

/**
 * The player the session (notification, lock screen, Android Auto) sees: the service's ExoPlayer with two things
 * of its own. The now-playing metadata can carry a lyric line or the lyrics card as its artwork ([showMetadata]) –
 * only what the session reports
 * changes; the queue's items keep their real titles, so the phone app's own player, scrobbling, the remote
 * snapshot and resume (which read the items or the ExoPlayer itself) never see it. And shuffle is the app's
 * shuffle (the rest of the queue in a new order, the old order back when it goes off), not ExoPlayer's
 * shuffle order, so "Up next" on the phone shows what really comes next. Every seek through it is a person's own (the
 * app, the notification, the lock screen, headphones, the car), so it runs inside [aroundUser]: in a group listen only
 * those go to the group, never the service's own seeks on the ExoPlayer itself.
 */
class CarPlayer(private val exo: Player) : ForwardingPlayer(exo) {
    /** set by the service: runs a person's seek or skip (GroupSync.fromUser in a group listen) */
    var aroundUser: (() -> Unit) -> Unit = { it() }

    private val listeners = ArrayList<Player.Listener>()
    private var shownFor: String? = null
    private var shown: MediaMetadata? = null
    /** set by [CarControls]: turn the app shuffle on / off */
    var onShuffle: ((Boolean) -> Unit)? = null
    /** set by [CarControls]: a controller's repeat change (Media3's own repeat command); false refuses it */
    var onRepeat: ((Int) -> Boolean)? = null
    /** the app shuffle's state */
    var shuffled = false
        private set

    override fun addListener(listener: Player.Listener) {
        listeners.add(listener)
        super.addListener(listener)
    }

    override fun removeListener(listener: Player.Listener) {
        listeners.remove(listener)
        super.removeListener(listener)
    }

    override fun getMediaMetadata(): MediaMetadata {
        val md = shown
        if (md != null && shownFor != null && exo.currentMediaItem?.mediaId == shownFor) return md
        return super.getMediaMetadata()
    }

    override fun getShuffleModeEnabled(): Boolean = shuffled

    override fun seekTo(positionMs: Long) = aroundUser { super.seekTo(positionMs) }
    override fun seekTo(mediaItemIndex: Int, positionMs: Long) = aroundUser { super.seekTo(mediaItemIndex, positionMs) }
    override fun seekToDefaultPosition() = aroundUser { super.seekToDefaultPosition() }
    override fun seekToDefaultPosition(mediaItemIndex: Int) = aroundUser { super.seekToDefaultPosition(mediaItemIndex) }
    override fun seekBack() = aroundUser { super.seekBack() }
    override fun seekForward() = aroundUser { super.seekForward() }
    override fun seekToPrevious() = aroundUser { super.seekToPrevious() }
    override fun seekToNext() = aroundUser { super.seekToNext() }
    override fun seekToPreviousMediaItem() = aroundUser { super.seekToPreviousMediaItem() }
    override fun seekToNextMediaItem() = aroundUser { super.seekToNextMediaItem() }

    override fun setShuffleModeEnabled(shuffleModeEnabled: Boolean) {
        onShuffle?.invoke(shuffleModeEnabled)
    }

    override fun setRepeatMode(repeatMode: Int) {
        if (onRepeat?.invoke(repeatMode) == false) return
        super.setRepeatMode(repeatMode)
    }

    /** What the session shows for the song [mediaId] (null: its own metadata again). Main thread. */
    fun showMetadata(mediaId: String?, md: MediaMetadata?) {
        if (shownFor == mediaId && shown == md) return
        shownFor = if (md == null) null else mediaId
        shown = md
        val now = getMediaMetadata()
        for (l in ArrayList(listeners)) l.onMediaMetadataChanged(now)
    }

    /** The app shuffle went on or off. Main thread. */
    fun setShuffled(on: Boolean) {
        if (shuffled == on) return
        shuffled = on
        for (l in ArrayList(listeners)) l.onShuffleModeEnabledChanged(on)
    }
}

/**
 * The buttons next to the transport in the car, the notification and the lock screen: in Android Auto the
 * listener's Car buttons (Settings → Playback → In the car; [CarPrefs.carButtons], [CarButtonRules.sent]; by default
 * Shuffle, Repeat, Like, Lyrics), Like first on the phone ([CarRules.PHONE]), as media button preferences with custom
 * session commands; the quality of the song on the car's subtitle line ("DROELOE · FLAC 24/96", [CarRules.subtitle]);
 * short notes on that line (a button's answer: the Signal path, "Added 25 songs by …"), which win over the lyrics
 * while they last; and live lyrics: a card with the current line as large as it fits as the now-playing artwork
 * (title and artist stay as they are), or the current synced line as the title ([CarPrefs.lyricsMode]). It also
 * publishes what plays to [CarNowPlaying] for the Encore Lyrics car app (car/LyricsCarAppService.kt), and the playing
 * song's artist for Home's "More by <artist>" ([onArtist]).
 */
class CarControls(
    private val context: Context,
    private val graph: AppGraph,
    private val exo: Player,
    private val io: ExecutorService,
    /** the position in the song (server-made streams count from where they started) */
    private val positionMs: () -> Long,
    /** the rate Android gets from the player (Hz, 0 not known), for the Signal path button */
    private val outputRate: () -> Int = { 0 },
    /** Data saver is on (an AAC stream is the saver's, not adaptive quality's), for the Signal path button */
    private val dataSaver: () -> Boolean = { false },
) {
    val player = CarPlayer(exo)
    private val main = Handler(Looper.getMainLooper())
    private var session: MediaSession? = null
    private var released = false
    private var likesJob: Job? = null
    private var lyricsJob: Job? = null

    /** the order of the songs after the current one before shuffle went on (media ids) */
    private var unshuffled: List<String> = emptyList()

    // live lyrics (main thread)
    private var lyricsOn = CarPrefs.liveLyrics(context).value
    private var mode = CarPrefs.lyricsMode(context).value
    private var modeJob: Job? = null
    private var watchJob: Job? = null
    private var lyricsFor: Long? = null
    private var lines: List<CarLine> = emptyList()
    private var lineAt = -2
    private val noLyricsToasted = HashSet<Long>()

    // the lyrics card: drawn on its own low-priority thread, newest request wins
    private val card = LyricsCard(context)
    private val cardThread: ExecutorService = Executors.newSingleThreadExecutor { r ->
        Thread(r, "car-lyrics-card").apply { priority = Thread.NORM_PRIORITY - 2; isDaemon = true }
    }
    private var cardSeq = 0
    @Volatile private var cardWanted = 0

    // Android Auto (main thread): while a car is connected the buttons take the car's order and the quality detail
    // goes on the subtitle line (the phone's notification stays as it was)
    private var cars = 0
    private var stream: StreamInfo? = null
    /** in a group listen: the car's subtitle line starts with "Group listen" */
    private var group = false
    private var quality: CarQuality? = null

    /**
     * Set by the service: the playing song's artist or its cover changed (null: nothing loaded). Home's
     * "More by <artist>" follows it (Android Auto's now-playing screen can't open a page from a button). Called
     * once when set, then on every change. Main thread.
     */
    var onArtist: ((ArtistNode?, android.net.Uri?) -> Unit)? = null
        set(value) {
            field = value
            artistNow = null
            artNow = null
            artistShown = false
            artistChanged()
        }
    private var artistNow: ArtistNode? = null
    private var artNow: android.net.Uri? = null
    private var artistShown = false

    /** the Car buttons setting: what Android Auto gets while a car is connected */
    private var layout = CarPrefs.carButtons(context).value
    private var buttonsJob: Job? = null

    /**
     * Set by the service: the Artist button was pressed for [ArtistNode]. It adds up to [CarRules.MORE_BY_SONGS] of
     * the artist's songs after the playing one (a group's Up next in a group listen) and calls back with how many
     * (null: they couldn't be loaded). Main thread, the callback too.
     */
    var onArtistPress: ((ArtistNode, (Int?) -> Unit) -> Unit)? = null
    /** the press loading now (artist key + song) and the last press that added songs */
    private var artistLoading: Pair<String, String>? = null
    private var addedKey: String? = null
    private var addedSong: String? = null

    // the note on the subtitle line (main thread): [noteText] for the song [noteFor] until [noteUntil] (elapsed ms)
    private var noteText: String? = null
    private var noteFor: String? = null
    private var noteUntil = 0L
    private var noteIsSignal = false
    /** what the session shows without the note ([shownMd] null: the song as it is), for the song [shownFor] */
    private var shownFor: String? = null
    private var shownMd: MediaMetadata? = null

    private val listener = object : Player.Listener {
        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
            if (reason != Player.MEDIA_ITEM_TRANSITION_REASON_PLAYLIST_CHANGED || currentId() != lyricsFor) {
                graph.likes.refresh()
                loadLyrics()
            }
            updateQuality()
            refreshButtons()
            publish()
            artistChanged()
        }

        override fun onRepeatModeChanged(repeatMode: Int) = refreshButtons()

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            main.removeCallbacks(lyricTick)
            if (isPlaying && lyricsOn && lines.isNotEmpty()) main.post(lyricTick)
            publish()
            main.removeCallbacks(publishTick)
            if (isPlaying && CarNowPlaying.watchers.value > 0) main.postDelayed(publishTick, 1000L)
        }

        override fun onPositionDiscontinuity(oldPosition: Player.PositionInfo, newPosition: Player.PositionInfo, reason: Int) {
            if (lyricsOn && lines.isNotEmpty()) showLine()
            publish()
        }

        override fun onPlaybackParametersChanged(playbackParameters: androidx.media3.common.PlaybackParameters) = publish()
    }

    init {
        player.onShuffle = { on -> setShuffle(on) }
        player.onRepeat = { mode -> mode == Player.REPEAT_MODE_OFF || !groupRefused() }
        exo.addListener(listener)
    }

    /** After the session is built: the buttons, and follow likes and the lyrics setting. */
    fun attach(session: MediaSession) {
        this.session = session
        refreshButtons()
        likesJob = graph.scope.launch { graph.likes.liked.collect { if (!released) refreshButtons() } }
        // Settings → Car buttons: a connected car gets the new list at once
        buttonsJob = graph.scope.launch {
            CarPrefs.carButtons(context).collect { l ->
                if (released || l == layout) return@collect
                layout = l
                refreshButtons()
            }
        }
        lyricsJob = graph.scope.launch {
            CarPrefs.liveLyrics(context).collect { on ->
                if (released || on == lyricsOn) return@collect
                lyricsOn = on
                if (on) loadLyrics() else stopLyrics()
                refreshButtons()
            }
        }
        modeJob = graph.scope.launch {
            CarPrefs.lyricsMode(context).collect { m ->
                if (released || m == mode) return@collect
                mode = m
                // the other look from the line playing now on
                cardWanted = ++cardSeq
                lineAt = -2
                plain()
                if (lyricsOn && lines.isNotEmpty()) showLine()
            }
        }
        // the Encore Lyrics car app is on screen: keep its position fresh
        watchJob = graph.scope.launch {
            CarNowPlaying.watchers.collect { n ->
                if (released) return@collect
                main.removeCallbacks(publishTick)
                publish()
                if (n > 0 && exo.isPlaying) main.postDelayed(publishTick, 1000L)
            }
        }
        if (graph.store.current != null) graph.likes.refresh()
        if (lyricsOn) loadLyrics()
        publish()
    }

    fun release() {
        released = true
        likesJob?.cancel()
        buttonsJob?.cancel()
        lyricsJob?.cancel()
        modeJob?.cancel()
        watchJob?.cancel()
        main.removeCallbacks(lyricTick)
        main.removeCallbacks(clearNote)
        main.removeCallbacks(publishTick)
        exo.removeListener(listener)
        player.onShuffle = null
        player.onRepeat = null
        session = null
        cardWanted = -1
        cardThread.execute { card.release() }
        cardThread.shutdown()
        CarNowPlaying.publish(null)
    }

    // ---- what plays, for the Encore Lyrics car app -----------------------------------------------------------

    private val publishTick = object : Runnable {
        override fun run() {
            if (released) return
            publish()
            if (exo.isPlaying && CarNowPlaying.watchers.value > 0) main.postDelayed(this, 1000L)
        }
    }

    /** The song (its own metadata, never the lyric line or card), whether it plays and where. Main thread. */
    private fun publish() {
        if (released) return
        val item = exo.currentMediaItem
        if (item == null) { CarNowPlaying.publish(null); return }
        val md = item.mediaMetadata
        CarNowPlaying.publish(
            NowPlaying(
                mediaId = item.mediaId,
                trackId = MediaItems.trackId(item.mediaId),
                title = md.title?.toString() ?: "",
                artist = md.artist?.toString() ?: "",
                album = md.albumTitle?.toString() ?: "",
                artworkUri = md.artworkUri,
                playing = exo.isPlaying,
                positionMs = positionMs(),
                atElapsedMs = SystemClock.elapsedRealtime(),
                speed = exo.playbackParameters.speed,
                artistNode = ArtistNode.of(item),
            ),
        )
    }

    /** A custom command from a controller; false when it isn't one of ours. Main thread. */
    fun handle(action: String): Boolean {
        when (action) {
            CMD_LIKE -> currentId()?.let { graph.likes.toggle(it) }
            CMD_SHUFFLE -> setShuffle(!player.shuffled)
            CMD_REPEAT -> if (!groupRefused()) exo.repeatMode = when (exo.repeatMode) {
                Player.REPEAT_MODE_OFF -> Player.REPEAT_MODE_ALL
                Player.REPEAT_MODE_ALL -> Player.REPEAT_MODE_ONE
                else -> Player.REPEAT_MODE_OFF
            }
            CMD_LYRICS -> CarPrefs.setLiveLyrics(context, !lyricsOn)
            CMD_ARTIST -> moreByArtist()
            CMD_SIGNAL -> signalPath()
            else -> return false
        }
        return true
    }

    /**
     * The Signal path button: the whole path on the subtitle line for 10 s ([CarButtonRules.signalLine]); pressed
     * again while it shows, the line goes back. Title, cover or lyrics and playback stay as they are.
     */
    private fun signalPath() {
        val item = exo.currentMediaItem ?: return
        if (noteIsSignal && noteFor == item.mediaId && noteText != null && SystemClock.elapsedRealtime() < noteUntil) {
            main.removeCallbacks(clearNote)
            clearNote.run()
            return
        }
        note(signalText(item), CarButtonRules.SIGNAL_NOTE_MS, signal = true)
    }

    /** The Signal path of the song playing, from the same StreamInfo the phone's Signal path sheet reads. */
    private fun signalText(item: MediaItem): String {
        val x = item.mediaMetadata.extras
        val id = MediaItems.trackId(item.mediaId)
        // the stream info of another song (just before the service published this one): not known yet
        val s = stream?.takeIf { it.trackId == id } ?: return CarButtonRules.SIGNAL_UNKNOWN
        val file = CarQuality.ofFile(
            x?.getString(MediaItems.EXTRA_FORMAT) ?: "",
            x?.getInt(MediaItems.EXTRA_BITRATE) ?: 0,
            x?.getInt(MediaItems.EXTRA_SAMPLE_RATE) ?: 0,
            x?.getInt(MediaItems.EXTRA_BIT_DEPTH) ?: 0,
        )?.detail ?: ""
        val local = if (s.transportKind == "local") app.encore.android.features.offline.OfflineStore.instance?.record(id ?: 0L)?.quality ?: "" else ""
        return CarButtonRules.signalLine(
            CarButtonRules.Signal(
                file = file,
                transportKind = s.transportKind,
                dataSaver = dataSaver(),
                localLabel = local,
                phoneRate = s.phoneRate,
                usbBitPerfect = s.usbBitPerfect,
                usbDevice = s.usbDevice,
                effects = s.effectsEngine.isNotEmpty(),
                outputRate = outputRate(),
                normDb = s.normDb,
            ),
        )
    }

    /**
     * The Artist button: up to 25 songs by the playing song's artist right after it (in a group listen, to the
     * group's Up next), said on the subtitle line. The song playing goes on as it is. A second press for the same
     * song adds nothing.
     */
    private fun moreByArtist() {
        val item = exo.currentMediaItem ?: return
        val a = currentArtist()
        if (a == null || a.name.isEmpty()) { note(CarButtonRules.NO_ARTIST); return }
        val inGroup = group || app.encore.android.features.group.GroupListen.inGroup
        if (CarButtonRules.alreadyAdded(addedKey, addedSong, a.key, item.mediaId)) {
            note(CarButtonRules.alreadyAddedMessage(a.name, inGroup))
            return
        }
        val press = a.key to item.mediaId
        note(CarButtonRules.addingMessage(a.name))
        if (artistLoading == press) return
        val cb = onArtistPress ?: return
        artistLoading = press
        cb(a) { added ->
            if (artistLoading == press) artistLoading = null
            if (released) return@cb
            if (added == null) { note(CarRules.moreByFailed(a.name)); return@cb }
            if (added > 0) { addedKey = a.key; addedSong = item.mediaId }
            note(CarRules.moreByMessage(a.name, added, group || app.encore.android.features.group.GroupListen.inGroup))
        }
    }

    /** [handle] with the result a controller gets back; null when it isn't one of ours. Main thread. */
    fun result(action: String): SessionResult? =
        if (handle(action)) SessionResult(SessionResult.RESULT_SUCCESS) else null

    /** The playing song's artist: the first credit in the library, else the first one. */
    fun currentArtist(): ArtistNode? {
        val item = exo.currentMediaItem ?: return null
        return ArtistNode.of(item)
    }

    /**
     * Tells [onArtist] when the playing song's artist is another one (or none), or its cover changed (the same
     * artist, another album: the "More by" row shows the new cover). Main thread.
     */
    private fun artistChanged() {
        if (released) return
        val cb = onArtist ?: return
        val a = currentArtist()
        val art = if (a == null) null else exo.currentMediaItem?.mediaMetadata?.artworkUri
        if (artistShown && a == artistNow && art == artNow) return
        artistNow = a
        artNow = art
        artistShown = true
        cb(a, art)
    }

    /** A controller connected or went away; [car]: it is Android Auto (or the desktop head unit). Main thread. */
    fun controller(car: Boolean, connected: Boolean) {
        if (!car) return
        val before = cars > 0
        cars = (cars + if (connected) 1 else -1).coerceAtLeast(0)
        if (before != cars > 0) {
            refreshButtons()
            refreshText()
        }
    }

    /** How the song reaches this phone (the service's StreamInfo, when it changes): the badge follows it. Main thread. */
    fun streamChanged(info: StreamInfo) {
        stream = info
        updateQuality()
    }

    /** The quality of the song playing now; the subtitle when it changed. */
    private fun updateQuality() {
        if (released) return
        val item = exo.currentMediaItem
        val q = if (item == null) null else {
            val x = item.mediaMetadata.extras
            val id = MediaItems.trackId(item.mediaId)
            // the stream info of another song (just before the service published this one): the file alone
            val s = stream?.takeIf { it.trackId == id }
            val local = if (s?.transportKind == "local") app.encore.android.features.offline.OfflineStore.instance?.record(id ?: 0L)?.quality ?: "" else ""
            CarQuality.of(
                format = x?.getString(MediaItems.EXTRA_FORMAT) ?: "",
                bitrate = x?.getInt(MediaItems.EXTRA_BITRATE) ?: 0,
                sampleRate = x?.getInt(MediaItems.EXTRA_SAMPLE_RATE) ?: 0,
                bitDepth = x?.getInt(MediaItems.EXTRA_BIT_DEPTH) ?: 0,
                transportKind = s?.transportKind ?: "",
                phoneRate = s?.phoneRate ?: 0,
                localQuality = local,
            )
        }
        if (q == quality) return
        quality = q
        refreshText()
    }

    /** The quality detail for the subtitle line ("FLAC 24/96"), only while a car is connected. */
    private fun detail(): String? = if (cars > 0) quality?.detail?.takeIf { it.isNotEmpty() } else null

    /** "<artist> · FLAC 24/96": the subtitle line with the quality detail after it (when there is one); "Group listen · " before it in a group. */
    private fun subtitleOf(artist: String): String = CarRules.subtitle(artist, detail(), group && cars > 0)

    /**
     * In a group listen or not (the service's GroupSync, before it takes the group's queue): the car's subtitle says
     * so, and repeat and the app shuffle go off (a group has no repeat; its Shuffle is a one-time shuffle of its Up
     * next, [setShuffle]). Main thread.
     */
    fun setGroup(on: Boolean) {
        if (group == on) return
        group = on
        if (on) {
            // the group's queue replaces this one right after: no reorder back, the shuffle just ends
            unshuffled = emptyList()
            if (player.shuffled) player.setShuffled(false)
            if (exo.repeatMode != Player.REPEAT_MODE_OFF) exo.repeatMode = Player.REPEAT_MODE_OFF
            refreshButtons()
        }
        refreshText()
    }

    /** In a group listen (the car's, notification's or lock screen's Repeat): refused, and said why. */
    private fun groupRefused(): Boolean {
        if (!group && !GroupListen.inGroup) return false
        note("Repeat is off during a group listen")
        return true
    }

    /**
     * In a group listen the Shuffle button shuffles the group's Up next once, for everyone (those who control; the
     * others are told who does). True when in a group.
     */
    private fun groupShuffle(): Boolean {
        if (!group && !GroupListen.inGroup) return false
        val s = GroupListen.state.value ?: return true
        if (!s.youCanControl) { note(GroupRules.onlyHost(s)); return true }
        GroupListen.shuffleUpNext()
        return true
    }

    /** The song as it is, with the quality detail on the subtitle line in the car. Main thread. */
    private fun plain() {
        val item = exo.currentMediaItem
        if (item == null) { player.showMetadata(null, null); return }
        if (detail() == null && !(group && cars > 0)) { show(item, null); return }
        val base = exo.mediaMetadata
        show(item, base.buildUpon().setArtist(subtitleOf(base.artist?.toString() ?: "")).build())
    }

    /**
     * What the session shows for [item]: [md] (null: the song's own metadata), with the note on the subtitle line
     * while there is one for this song ([CarButtonRules.subtitleNow]). Every lyric line, card and plain update goes
     * through here, so a note keeps its line until it ends, then the line underneath comes back. Main thread.
     */
    private fun show(item: MediaItem, md: MediaMetadata?) {
        shownFor = item.mediaId
        shownMd = md
        val base = md ?: exo.mediaMetadata
        val normal = base.artist?.toString() ?: ""
        val note = if (noteFor == item.mediaId) noteText else null
        val line = CarButtonRules.subtitleNow(note, noteUntil, SystemClock.elapsedRealtime(), normal)
        if (line == normal) player.showMetadata(if (md == null) null else item.mediaId, md)
        else player.showMetadata(item.mediaId, base.buildUpon().setArtist(line).build())
    }

    /** The subtitle again after the car or the quality changed: the lyric line or card drawn anew, else [plain]. */
    private fun refreshText() {
        if (released) return
        if (lyricsOn && lines.isNotEmpty()) {
            lineAt = -2
            showLine()
        } else {
            plain()
        }
    }

    /** [note] for the service: why the car's request was not done (in a group listen, "Only Alex controls playback"). */
    fun say(text: String) = note(text)

    /**
     * A note on the subtitle line for [ms] (title, cover, lyric line or card stay; a new song ends it). It wins over
     * the lyric lines and cards that come while it shows ([show]). [signal]: the Signal path's, which a second
     * press takes away. Main thread.
     */
    private fun note(text: String, ms: Long = CarButtonRules.NOTE_MS, signal: Boolean = false) {
        val item = exo.currentMediaItem ?: return
        noteText = text
        noteFor = item.mediaId
        noteUntil = SystemClock.elapsedRealtime() + ms
        noteIsSignal = signal
        main.removeCallbacks(clearNote)
        main.postDelayed(clearNote, ms)
        if (shownFor == item.mediaId) show(item, shownMd) else refreshText()
    }

    /** A new queue replaced the old one (any controller, a remote device): shuffle starts off. Main thread. */
    fun queueReplaced() {
        unshuffled = emptyList()
        if (player.shuffled) {
            player.setShuffled(false)
            refreshButtons()
        }
    }

    private fun currentId(): Long? = exo.currentMediaItem?.let { MediaItems.trackId(it.mediaId) }

    // ---- shuffle: the app's shuffle of what comes next -------------------------------------------------------

    private fun setShuffle(on: Boolean) {
        if (groupShuffle()) return
        if (on == player.shuffled) return
        val cur = exo.currentMediaItemIndex
        val n = exo.mediaItemCount
        val rest = ArrayList<MediaItem>()
        if (n > 0 && cur >= 0) for (i in cur + 1 until n) rest.add(exo.getMediaItemAt(i))
        if (on) {
            unshuffled = rest.map { it.mediaId }
            if (rest.size >= 2) {
                val mixed = rest.shuffled()
                exo.removeMediaItems(cur + 1, n)
                exo.addMediaItems(mixed)
            }
        } else {
            // what is left goes back to the order it had; songs added since stay at the end
            val rank = HashMap<String, Int>()
            unshuffled.forEachIndexed { i, id -> if (!rank.containsKey(id)) rank[id] = i }
            val sorted = rest.withIndex()
                .sortedWith(compareBy<IndexedValue<MediaItem>>({ rank[it.value.mediaId] ?: Int.MAX_VALUE }, { it.index }))
                .map { it.value }
            if (rest.size >= 2 && sorted.map { it.mediaId } != rest.map { it.mediaId }) {
                exo.removeMediaItems(cur + 1, n)
                exo.addMediaItems(sorted)
            }
            unshuffled = emptyList()
        }
        player.setShuffled(on)
        refreshButtons()
    }

    // ---- the buttons -----------------------------------------------------------------------------------------

    private fun refreshButtons() {
        if (released) return
        val s = session ?: return
        try { s.setMediaButtonPreferences(buttons()) } catch (_: Exception) {}
    }

    /**
     * [CarRules.buttons] as Media3 buttons: in the car the Car buttons setting's list ([CarButtonRules.sent]: the
     * screen group, then the ⋯ menu group, never the hidden ones), on the phone Like first ([CarRules.PHONE]). Each
     * one is in the overflow slot only ([CarRules.SLOTS]), so Media3 hands them to the platform session as custom
     * actions in exactly this order and none takes the place of Previous or Next. Every one is enabled and sent even
     * when it can't act, so no button moves into another spot between songs.
     */
    private fun buttons(): List<CommandButton> {
        val liked = currentId()?.let { graph.likes.isLiked(it) } == true
        val state = CarRules.State(liked, player.shuffled, exo.repeatMode, lyricsOn, currentArtist()?.name ?: "")
        val ids = if (cars > 0) CarButtonRules.sent(layout) else CarRules.PHONE
        return CarRules.buttons(ids, state).map { b ->
            val builder = CommandButton.Builder(b.icon)
                .setDisplayName(b.name)
                .setSessionCommand(SessionCommand(b.command, Bundle.EMPTY))
                .setSlots(*CarRules.SLOTS)
            if (b.customIcon != 0) builder.setCustomIconResId(b.customIcon)
            builder.build()
        }
    }

    // ---- live lyrics -----------------------------------------------------------------------------------------

    private val lyricTick = object : Runnable {
        override fun run() {
            if (released || !lyricsOn || lines.isEmpty()) return
            showLine()
            if (exo.isPlaying) main.postDelayed(this, 250L)
        }
    }

    /**
     * A note ended: what was under it comes back as it was (the lyric line or card, else the song with its quality
     * detail), without drawing the card again.
     */
    private val clearNote = Runnable {
        if (released) return@Runnable
        noteText = null
        noteFor = null
        noteIsSignal = false
        val item = exo.currentMediaItem ?: return@Runnable
        if (shownFor == item.mediaId) show(item, shownMd) else refreshText()
    }

    /** The current song's synced lyrics, fetched once per song while live lyrics are on (LyricsCache). */
    private fun loadLyrics() {
        main.removeCallbacks(lyricTick)
        val id = currentId()
        lyricsFor = id
        lines = emptyList()
        lineAt = -2
        cardWanted = ++cardSeq
        plain()
        if (!lyricsOn || id == null) return
        io.execute {
            val parsed = LyricsCache.synced(graph.api, id)
            main.post {
                if (released || !lyricsOn || lyricsFor != id || currentId() != id) return@post
                lines = parsed
                if (parsed.isEmpty()) {
                    noSyncedLyrics(id)
                } else {
                    showLine()
                    main.removeCallbacks(lyricTick)
                    if (exo.isPlaying) main.post(lyricTick)
                }
            }
        }
    }

    /**
     * No synced lyrics: the normal title and cover. With the title line, "No synced lyrics for this song" in the
     * subtitle for 3 s, once per song (the card leaves title and artist alone).
     */
    private fun noSyncedLyrics(id: Long) {
        if (!CarPrefs.titleMode(mode)) return
        if (!noLyricsToasted.add(id)) return
        note("No synced lyrics for this song")
    }

    private fun stopLyrics() {
        main.removeCallbacks(lyricTick)
        lines = emptyList()
        lyricsFor = null
        lineAt = -2
        cardWanted = ++cardSeq
        plain()
    }

    /**
     * The line at the current position: as the lyrics card (the artwork; title and artist as they are) or as the
     * title with "<title> · <artist>" below it. Before the first line the title mode shows the song as it is and
     * the card shows the lines to come. In the car the quality detail stays on the subtitle line after the artist
     * ("<title> · <artist> · FLAC 24/96" while the lyric line owns the title).
     */
    private fun showLine() {
        val item = exo.currentMediaItem ?: return
        val at = LyricsCache.lineAt(lines, positionMs())
        if (at == lineAt) return
        lineAt = at
        if (CarPrefs.cardMode(mode)) { showCard(item, at); return }
        if (at < 0) { plain(); return }
        val base = exo.mediaMetadata
        val title = base.title?.toString() ?: ""
        val artist = base.artist?.toString() ?: ""
        val text = lines[at].text.ifBlank { "♪" }
        val md = base.buildUpon()
            .setTitle(text)
            .setDisplayTitle(text)
            .setArtist(CarRules.lineSubtitle(title, artist, detail(), group && cars > 0))
            .build()
        show(item, md)
    }

    /**
     * Draws the card for line [at] on the card thread and shows it as the session's artwork: the bytes as
     * artworkData and a new content:// URI per card as artworkUri (hosts that load by URI see a new picture each
     * time). Only the reported metadata changes, so playback never rebuffers. A newer request drops this one.
     */
    private fun showCard(item: MediaItem, at: Int) {
        val seq = ++cardSeq
        cardWanted = seq
        val snapshot = lines
        val itemMd = item.mediaMetadata
        val coverUri = itemMd.artworkUri
        val coverKey = itemMd.extras?.getString(MediaItems.EXTRA_ALBUM_ID) ?: coverUri?.toString() ?: ""
        try {
            cardThread.execute {
                if (cardWanted != seq) return@execute
                val r = try { card.render(coverKey, coverUri, snapshot, at) } catch (_: Throwable) { null }
                if (r == null) return@execute
                main.post {
                    if (released || cardWanted != seq || !lyricsOn || !CarPrefs.cardMode(mode)) return@post
                    if (exo.currentMediaItem?.mediaId != item.mediaId) return@post
                    val base = exo.mediaMetadata
                    val b = base.buildUpon()
                        .setArtworkData(r.bytes, MediaMetadata.PICTURE_TYPE_FRONT_COVER)
                        .setArtworkUri(r.uri)
                        .setArtist(subtitleOf(base.artist?.toString() ?: ""))
                    // Wide: the lyrics run across the player too. The line playing is the title (Android Auto's
                    // largest text, the wide space beside the card) and the next line sits under it; before the
                    // first line the song stays as it is
                    if (mode == CarPrefs.MODE_WIDE && at >= 0 && at < snapshot.size) {
                        val text = snapshot[at].text.ifBlank { "♪" }
                        val next = snapshot.getOrNull(at + 1)?.text?.takeIf { it.isNotBlank() }
                        val title = base.title?.toString() ?: ""
                        val artist = base.artist?.toString() ?: ""
                        b.setTitle(text).setDisplayTitle(text)
                            .setArtist(next ?: CarRules.lineSubtitle(title, artist, detail(), group && cars > 0))
                    }
                    show(item, b.build())
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {}
    }

    companion object {
        const val CMD_LIKE = "app.encore.android.CAR_LIKE"
        const val CMD_SHUFFLE = "app.encore.android.CAR_SHUFFLE"
        const val CMD_REPEAT = "app.encore.android.CAR_REPEAT"
        const val CMD_LYRICS = "app.encore.android.CAR_LYRICS"
        /** Car buttons: "More by <artist>" after this song, and the Signal path on the subtitle line */
        const val CMD_ARTIST = "app.encore.android.CAR_ARTIST"
        const val CMD_SIGNAL = "app.encore.android.CAR_SIGNAL"

        /** The session commands controllers need to see the buttons (MediaSession hides buttons whose command they lack). */
        val COMMANDS = listOf(CMD_LIKE, CMD_SHUFFLE, CMD_REPEAT, CMD_LYRICS, CMD_ARTIST, CMD_SIGNAL)

        /** Android Auto and the desktop head unit: the controllers the car's button order and the subtitle detail are for. */
        val CAR_PACKAGES = setOf("com.google.android.projection.gearhead", "com.google.android.autosimulator")
    }
}
