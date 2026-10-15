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
import app.encore.android.AppGraph
import app.encore.android.R
import app.encore.android.car.CarLine
import app.encore.android.car.CarNowPlaying
import app.encore.android.car.LyricsCache
import app.encore.android.car.LyricsCard
import app.encore.android.car.NowPlaying
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
 * artwork ([MODE_CARD], the default) or the current line as the title ([MODE_TITLE]).
 */
object CarPrefs {
    const val MODE_CARD = "card"
    const val MODE_TITLE = "title"
    private const val FILE = "encore_car"
    private const val LIVE_LYRICS = "live_lyrics"
    private const val MODE = "lyrics_mode"
    @Volatile private var flow: MutableStateFlow<Boolean>? = null
    @Volatile private var modeFlow: MutableStateFlow<String>? = null

    /** [MODE_CARD] or [MODE_TITLE]. */
    fun lyricsMode(context: Context): StateFlow<String> = modeState(context).asStateFlow()

    fun setLyricsMode(context: Context, mode: String) {
        val m = if (mode == MODE_TITLE) MODE_TITLE else MODE_CARD
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
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getString(MODE, MODE_CARD)
        } catch (_: Exception) { MODE_CARD }
        return MutableStateFlow(if (m == MODE_TITLE) MODE_TITLE else MODE_CARD).also { modeFlow = it }
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
 * shuffle order, so "Up next" on the phone shows what really comes next.
 */
class CarPlayer(private val exo: Player) : ForwardingPlayer(exo) {
    private val listeners = ArrayList<Player.Listener>()
    private var shownFor: String? = null
    private var shown: MediaMetadata? = null
    /** set by [CarControls]: turn the app shuffle on / off */
    var onShuffle: ((Boolean) -> Unit)? = null
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

    override fun setShuffleModeEnabled(shuffleModeEnabled: Boolean) {
        onShuffle?.invoke(shuffleModeEnabled)
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
 * The buttons next to the transport in the car, the notification and the lock screen (like YouTube Music):
 * Like, Shuffle, Repeat and Lyrics, as media button preferences with custom session commands; and live lyrics:
 * a card with the lines around the current one as the now-playing artwork (title and artist stay as they are),
 * or the current synced line as the title ([CarPrefs.lyricsMode]). It also publishes what plays to
 * [CarNowPlaying] for the Encore Lyrics car app (car/LyricsCarAppService.kt).
 */
class CarControls(
    private val context: Context,
    private val graph: AppGraph,
    private val exo: Player,
    private val io: ExecutorService,
    /** the position in the song (server-made streams count from where they started) */
    private val positionMs: () -> Long,
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

    private val listener = object : Player.Listener {
        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
            if (reason != Player.MEDIA_ITEM_TRANSITION_REASON_PLAYLIST_CHANGED || currentId() != lyricsFor) {
                graph.likes.refresh()
                loadLyrics()
            }
            refreshButtons()
            publish()
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
        exo.addListener(listener)
    }

    /** After the session is built: the buttons, and follow likes and the lyrics setting. */
    fun attach(session: MediaSession) {
        this.session = session
        refreshButtons()
        likesJob = graph.scope.launch { graph.likes.liked.collect { if (!released) refreshButtons() } }
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
                player.showMetadata(null, null)
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
        lyricsJob?.cancel()
        modeJob?.cancel()
        watchJob?.cancel()
        main.removeCallbacks(lyricTick)
        main.removeCallbacks(clearNote)
        main.removeCallbacks(publishTick)
        exo.removeListener(listener)
        player.onShuffle = null
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
            ),
        )
    }

    /** A custom command from a controller; false when it isn't one of ours. Main thread. */
    fun handle(action: String): Boolean {
        when (action) {
            CMD_LIKE -> currentId()?.let { graph.likes.toggle(it) }
            CMD_SHUFFLE -> setShuffle(!player.shuffled)
            CMD_REPEAT -> exo.repeatMode = when (exo.repeatMode) {
                Player.REPEAT_MODE_OFF -> Player.REPEAT_MODE_ALL
                Player.REPEAT_MODE_ALL -> Player.REPEAT_MODE_ONE
                else -> Player.REPEAT_MODE_OFF
            }
            CMD_LYRICS -> CarPrefs.setLiveLyrics(context, !lyricsOn)
            else -> return false
        }
        return true
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

    /** Like, Shuffle, Repeat, Lyrics (YouTube Music's order, with lyrics after repeat). */
    private fun buttons(): List<CommandButton> {
        val liked = currentId()?.let { graph.likes.isLiked(it) } == true
        val like = CommandButton.Builder(if (liked) CommandButton.ICON_HEART_FILLED else CommandButton.ICON_HEART_UNFILLED)
            .setDisplayName(if (liked) "Unlike" else "Like")
            .setCustomIconResId(if (liked) R.drawable.ic_favorite else R.drawable.ic_favorite_border)
            .setSessionCommand(SessionCommand(CMD_LIKE, Bundle.EMPTY))
            .build()
        val shuffle = CommandButton.Builder(if (player.shuffled) CommandButton.ICON_SHUFFLE_ON else CommandButton.ICON_SHUFFLE_OFF)
            .setDisplayName(if (player.shuffled) "Shuffle on" else "Shuffle off")
            .setSessionCommand(SessionCommand(CMD_SHUFFLE, Bundle.EMPTY))
            .build()
        val repeatIcon = when (exo.repeatMode) {
            Player.REPEAT_MODE_ALL -> CommandButton.ICON_REPEAT_ALL
            Player.REPEAT_MODE_ONE -> CommandButton.ICON_REPEAT_ONE
            else -> CommandButton.ICON_REPEAT_OFF
        }
        val repeatName = when (exo.repeatMode) {
            Player.REPEAT_MODE_ALL -> "Repeat all"
            Player.REPEAT_MODE_ONE -> "Repeat one"
            else -> "Repeat off"
        }
        val repeat = CommandButton.Builder(repeatIcon)
            .setDisplayName(repeatName)
            .setSessionCommand(SessionCommand(CMD_REPEAT, Bundle.EMPTY))
            .build()
        val lyrics = CommandButton.Builder(CommandButton.ICON_UNDEFINED)
            .setDisplayName(if (lyricsOn) "Live lyrics on" else "Live lyrics off")
            .setCustomIconResId(if (lyricsOn) R.drawable.ic_lyrics_on else R.drawable.ic_lyrics)
            .setSessionCommand(SessionCommand(CMD_LYRICS, Bundle.EMPTY))
            .build()
        return listOf(like, shuffle, repeat, lyrics)
    }

    // ---- live lyrics -----------------------------------------------------------------------------------------

    private val lyricTick = object : Runnable {
        override fun run() {
            if (released || !lyricsOn || lines.isEmpty()) return
            showLine()
            if (exo.isPlaying) main.postDelayed(this, 250L)
        }
    }

    private val clearNote = Runnable { if (!released && lines.isEmpty()) player.showMetadata(null, null) }

    /** The current song's synced lyrics, fetched once per song while live lyrics are on (LyricsCache). */
    private fun loadLyrics() {
        main.removeCallbacks(lyricTick)
        main.removeCallbacks(clearNote)
        val id = currentId()
        lyricsFor = id
        lines = emptyList()
        lineAt = -2
        cardWanted = ++cardSeq
        player.showMetadata(null, null)
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
        if (mode != CarPrefs.MODE_TITLE) return
        if (!noLyricsToasted.add(id)) return
        val item = exo.currentMediaItem ?: return
        val md = exo.mediaMetadata.buildUpon().setArtist("No synced lyrics for this song").build()
        player.showMetadata(item.mediaId, md)
        main.removeCallbacks(clearNote)
        main.postDelayed(clearNote, 3000L)
    }

    private fun stopLyrics() {
        main.removeCallbacks(lyricTick)
        main.removeCallbacks(clearNote)
        lines = emptyList()
        lyricsFor = null
        lineAt = -2
        cardWanted = ++cardSeq
        player.showMetadata(null, null)
    }

    /**
     * The line at the current position: as the lyrics card (the artwork; title and artist as they are) or as the
     * title with "<title> · <artist>" below it. Before the first line the title mode shows the song as it is and
     * the card shows the lines to come.
     */
    private fun showLine() {
        val item = exo.currentMediaItem ?: return
        val at = LyricsCache.lineAt(lines, positionMs())
        if (at == lineAt) return
        lineAt = at
        if (mode == CarPrefs.MODE_CARD) { showCard(item, at); return }
        if (at < 0) { player.showMetadata(null, null); return }
        val base = exo.mediaMetadata
        val title = base.title?.toString() ?: ""
        val artist = base.artist?.toString() ?: ""
        val text = lines[at].text.ifBlank { "♪" }
        val md = base.buildUpon()
            .setTitle(text)
            .setDisplayTitle(text)
            .setArtist(listOf(title, artist).filter { it.isNotEmpty() }.joinToString(" · "))
            .build()
        player.showMetadata(item.mediaId, md)
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
                    if (released || cardWanted != seq || !lyricsOn || mode != CarPrefs.MODE_CARD) return@post
                    if (exo.currentMediaItem?.mediaId != item.mediaId) return@post
                    val md = exo.mediaMetadata.buildUpon()
                        .setArtworkData(r.bytes, MediaMetadata.PICTURE_TYPE_FRONT_COVER)
                        .setArtworkUri(r.uri)
                        .build()
                    player.showMetadata(item.mediaId, md)
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {}
    }

    companion object {
        const val CMD_LIKE = "app.encore.android.CAR_LIKE"
        const val CMD_SHUFFLE = "app.encore.android.CAR_SHUFFLE"
        const val CMD_REPEAT = "app.encore.android.CAR_REPEAT"
        const val CMD_LYRICS = "app.encore.android.CAR_LYRICS"

        /** The session commands controllers need to see the buttons (MediaSession hides buttons whose command they lack). */
        val COMMANDS = listOf(CMD_LIKE, CMD_SHUFFLE, CMD_REPEAT, CMD_LYRICS)
    }
}
