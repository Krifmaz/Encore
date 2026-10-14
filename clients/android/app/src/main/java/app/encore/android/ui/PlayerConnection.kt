package app.encore.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionCommand
import androidx.media3.session.SessionToken
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.data.prefs.PrefsRepository
import app.encore.android.graph
import app.encore.android.playback.EncorePlaybackService
import app.encore.android.playback.MediaItems
import app.encore.android.playback.StreamInfo
import app.encore.android.remote.Link
import app.encore.android.remote.RemoteDevice
import app.encore.android.ui.components.PlayerMenuActions
import app.encore.android.ui.components.Toasts
import com.google.common.util.concurrent.ListenableFuture
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonObjectBuilder
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.concurrent.Executor

/** What the player UI shows; rebuilt from the MediaController whenever the player changes. */
data class PlayerUiState(
    val connected: Boolean = false,
    val current: Track? = null,
    val isPlaying: Boolean = false,
    val buffering: Boolean = false,
    val durationMs: Long = 0L,
    val index: Int = -1,
    val queue: List<Track> = emptyList(),
    val hasNext: Boolean = false,
    val error: String? = null,
)

/**
 * The phone UI's side of playback: a Media3 MediaController bound to [EncorePlaybackService]. The activity
 * connects in onStart and releases in onStop, so the service can stop when nothing plays.
 *
 * Shared seam (android-m2m3 contract): the design's screens read [nowPlaying], [isPlaying], [positionMs],
 * [durationMs], [queue], [queueIndex], [qualityBadge] and [qualityTitle]; the "⋯" menu forwards Lyrics, Share link
 * and Signal path to [menuActions], which this class implements. The service reports what it chose for the stream
 * (badge, transport, the start offset of a server-made stream) in the session extras ([streamInfo]); the position
 * of such a stream includes that offset, and seeks in it go back to the service (`t=`) instead of the player.
 */
class PlayerConnection(private val context: Context, private val api: EncoreApi) : PlayerMenuActions {
    private val mainHandler = Handler(Looper.getMainLooper())
    private val mainExecutor = Executor { r -> mainHandler.post(r) }
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var future: ListenableFuture<MediaController>? = null
    private var controller: MediaController? = null
    private val prefs: PrefsRepository? by lazy { try { context.graph.prefs } catch (_: Exception) { null } }

    /**
     * This phone as an Encore remote device (remote/RemoteDevice.kt). While it is linked to another device, the
     * player flows below show that device and every command here (play, pause, seek, playing a list …) goes there.
     */
    val remote: RemoteDevice by lazy { RemoteDevice.get(context) }
    private var linkJob: Job? = null
    /** the player flows show the linked device's state (applyLink), not this phone's */
    private var showingLink = false

    private val _state = MutableStateFlow(PlayerUiState())
    val state: StateFlow<PlayerUiState> = _state.asStateFlow()

    // ---- shared seam (android-m2m3 contract): what the design's player screens read -----------------------------
    private val _nowPlaying = MutableStateFlow<Track?>(null)
    /** The song in front of you, or null when nothing is loaded. */
    val nowPlaying: StateFlow<Track?> = _nowPlaying.asStateFlow()
    private val _isPlaying = MutableStateFlow(false)
    val isPlaying: StateFlow<Boolean> = _isPlaying.asStateFlow()
    private val _positionMs = MutableStateFlow(0L)
    /** The position in song time, refreshed twice a second while playing and on every player event. */
    val positionMs: StateFlow<Long> = _positionMs.asStateFlow()
    private val _durationMs = MutableStateFlow(0L)
    val durationMs: StateFlow<Long> = _durationMs.asStateFlow()
    private val _queue = MutableStateFlow<List<Track>>(emptyList())
    val queue: StateFlow<List<Track>> = _queue.asStateFlow()
    private val _queueIndex = MutableStateFlow(-1)
    val queueIndex: StateFlow<Int> = _queueIndex.asStateFlow()
    private val _qualityBadge = MutableStateFlow<String?>(null)
    /** web/js/player.js `quality()`: "FLAC 1431k", "FLAC → AAC 96k", "FLAC → Cadenza 96 kHz"; null when nothing plays. */
    val qualityBadge: StateFlow<String?> = _qualityBadge.asStateFlow()
    private val _qualityTitle = MutableStateFlow<String?>(null)
    /** web/js/player.js `qualityTitle()`: the long explanation behind the badge. */
    val qualityTitle: StateFlow<String?> = _qualityTitle.asStateFlow()
    private val _streamInfo = MutableStateFlow<StreamInfo?>(null)
    /** Everything the service reports about the stream that plays (the Signal path sheet reads it). */
    val streamInfo: StateFlow<StreamInfo?> = _streamInfo.asStateFlow()

    /**
     * The "⋯" menu's lyrics / share / signal path, implemented here. The design's menu
     * (ui/screens/PlayerScreens.kt `rememberPlayerMenuActions`) handles navigation itself and forwards the rest.
     */
    var menuActions: PlayerMenuActions? = this

    // sheets the ⋯ menu opens; the root draws LyricsScreen / SignalPathSheet while these are true
    private val _lyricsOpen = MutableStateFlow(false)
    val lyricsOpen: StateFlow<Boolean> = _lyricsOpen.asStateFlow()
    private val _signalPathOpen = MutableStateFlow(false)
    val signalPathOpen: StateFlow<Boolean> = _signalPathOpen.asStateFlow()
    private val _upNextOpen = MutableStateFlow(false)
    val upNextOpen: StateFlow<Boolean> = _upNextOpen.asStateFlow()

    /** Optional hooks for the navigation entries when this class is used as the whole menu (the root's menu normally does these). */
    var onOpenArtist: ((String) -> Unit)? = null
    var onOpenAlbum: ((String) -> Unit)? = null
    var onAddToPlaylist: ((Track) -> Unit)? = null

    private var qualityOverride: Pair<String, String?>? = null

    /** Lets another part pin a badge (e.g. a remote device's); null goes back to what the service reports. */
    fun setQuality(badge: String?, title: String?) {
        qualityOverride = if (badge != null) badge to title else null
        updateQuality()
    }

    private val positionTick = object : Runnable {
        override fun run() {
            if (linked() != null) return
            val c = controller ?: return
            _positionMs.value = c.currentPosition.coerceAtLeast(0L) + offsetMs()
            if (c.isPlaying) mainHandler.postDelayed(this, 500L)
        }
    }

    /** Commands made before the controller is ready run once it is. */
    private val pending = ArrayList<(MediaController) -> Unit>()

    private val listener = object : Player.Listener {
        override fun onEvents(player: Player, events: Player.Events) {
            // a song transition steps the volume many times a second: nothing shown here changes with it
            if (events.size() == 1 && events.contains(Player.EVENT_VOLUME_CHANGED)) return
            refresh()
        }
    }

    private val controllerListener = object : MediaController.Listener {
        override fun onExtrasChanged(controller: MediaController, extras: Bundle) {
            _streamInfo.value = StreamInfo.from(extras)
            refresh()
        }
    }

    fun connect() {
        // check in and listen for commands while the app is in front (the service keeps it going while it plays)
        remote.retain("ui")
        watchLink()
        connectController()
    }

    private fun connectController() {
        // the account's playback settings win over this device's (playback.js adoptAccount), checked on every foreground
        prefs?.sync()
        if (future != null) return
        val token = SessionToken(context, ComponentName(context, EncorePlaybackService::class.java))
        val f = MediaController.Builder(context, token).setListener(controllerListener).buildAsync()
        future = f
        f.addListener({
            try {
                val c = f.get()
                controller = c
                c.addListener(listener)
                _streamInfo.value = StreamInfo.from(c.sessionExtras)
                refresh()
                val queued = ArrayList(pending)
                pending.clear()
                for (cmd in queued) cmd(c)
            } catch (_: Exception) {
                _state.value = PlayerUiState(connected = false)
            }
        }, mainExecutor)
    }

    fun release() {
        remote.release("ui")
        linkJob?.cancel()
        linkJob = null
        mainHandler.removeCallbacks(positionTick)
        mainHandler.removeCallbacks(linkTick)
        controller?.removeListener(listener)
        controller = null
        future?.let { MediaController.releaseFuture(it) }
        future = null
        _state.value = _state.value.copy(connected = false)
    }

    private fun withController(cmd: (MediaController) -> Unit) {
        val c = controller
        if (c != null) cmd(c) else {
            pending.add(cmd)
            connectController()
        }
    }

    /** Where a server-made stream started: the player counts from there. */
    private fun offsetMs(): Long {
        val info = _streamInfo.value ?: return 0L
        val t = _nowPlaying.value ?: return 0L
        return if (info.trackId == t.id && info.serverStream) info.offsetMs else 0L
    }

    private fun serverStream(): Boolean {
        val info = _streamInfo.value ?: return false
        val t = _nowPlaying.value ?: return false
        return info.trackId == t.id && info.serverStream
    }

    /** The position right now in song time (the UI polls it while playing); the linked device's when linked. */
    fun positionMs(): Long {
        linked()?.let { return Math.round(it.positionSec() * 1000) }
        return (controller?.currentPosition ?: 0L).coerceAtLeast(0L) + offsetMs()
    }

    // ---- linked: this phone is a remote for another device (web/js/player.js setLink / viaLink) -------------

    /** The device this phone controls, or null. */
    fun linked(): Link? = remote.link.value

    private fun watchLink() {
        if (linkJob != null) return
        linkJob = scope.launch { remote.link.collect { applyLink(it) } }
    }

    /** Send a command to the linked device; false when this phone isn't linked (play here then). */
    private fun viaLink(action: String, build: JsonObjectBuilder.() -> Unit = {}): Boolean {
        if (linked() == null) return false
        val body: JsonObject = buildJsonObject {
            put("action", action)
            build()
        }
        return remote.sendToLink(body)
    }

    /** player.js playQueue() while linked: the library songs of the list go to the linked device. */
    private fun sendQueue(list: List<Track>, start: Int, positionSec: Double) {
        val lib = list.filter { it.id > 0L }
        if (lib.isEmpty()) {
            Toasts.show("Only songs from your library can be played on another device", "err")
            return
        }
        val idx = list.take(start.coerceAtLeast(0)).count { it.id > 0L }.coerceIn(0, lib.size - 1)
        val (win, at) = RemoteDevice.window(lib, idx, 1000)
        viaLink("play_queue") {
            put("tracks", RemoteDevice.slimList(win))
            put("index", at)
            if (positionSec > 1) put("position", positionSec)
        }
    }

    /** Between check-ins the linked device's seek bar moves on here. */
    private val linkTick = object : Runnable {
        override fun run() {
            val l = linked() ?: return
            _positionMs.value = Math.round(l.positionSec() * 1000)
            if (l.playing) mainHandler.postDelayed(this, 500L)
        }
    }

    /** The player flows from the linked device's state (or back to this phone's player when unlinked). */
    private fun applyLink(l: Link?) {
        mainHandler.removeCallbacks(linkTick)
        val wasLinked = showingLink
        showingLink = l != null
        if (l == null) {
            qualityOverride = null
            if (controller != null) {
                refresh()
            } else if (wasLinked) {
                // only a real unlink empties the player; on a foreground the link's first value (not linked) comes
                // while the controller is still connecting, and the last values stay until refresh() runs
                _nowPlaying.value = null
                _isPlaying.value = false
                _durationMs.value = 0L
                _queue.value = emptyList()
                _queueIndex.value = -1
                _positionMs.value = 0L
                _state.value = PlayerUiState(connected = false)
                updateQuality()
            }
            return
        }
        mainHandler.removeCallbacks(positionTick)
        val t = l.track
        val queue = l.queue
        var index = if (t == null) -1 else l.index
        if (t != null && (index !in queue.indices || queue[index].id != t.id)) index = queue.indexOfFirst { it.id == t.id }
        val dur = Math.round(l.durationSec * 1000)
        _nowPlaying.value = t
        _isPlaying.value = l.playing
        _durationMs.value = dur
        _queue.value = queue
        _queueIndex.value = index
        _positionMs.value = Math.round(l.positionSec() * 1000)
        val label = l.signalLabel
        qualityOverride = if (label.isNotEmpty()) label to "Playing on ${l.name}" else ("" to null)
        updateQuality()
        if (_qualityBadge.value.isNullOrEmpty()) _qualityBadge.value = null
        _state.value = PlayerUiState(
            connected = true,
            current = t,
            isPlaying = l.playing,
            durationMs = dur,
            index = index,
            queue = queue,
            hasNext = index >= 0 && index < queue.lastIndex,
        )
        if (t == null) { _lyricsOpen.value = false; _signalPathOpen.value = false }
        if (l.playing) mainHandler.postDelayed(linkTick, 500L)
    }

    /** The volume of the player (0..1): this phone's player, or the linked device's. */
    fun setVolume(v: Float) {
        val x = v.coerceIn(0f, 1f)
        if (viaLink("volume") { put("value", x.toDouble()) }) return
        withController { c -> c.volume = x }
    }

    private fun notWhileLinked(): Boolean {
        val l = linked() ?: return false
        Toasts.show("Change the queue on ${l.name} itself, or unlink first", "info")
        return true
    }

    // ---- commands --------------------------------------------------------------------------------------

    fun play(tracks: List<Track>, startIndex: Int = 0, shuffle: Boolean = false) {
        if (tracks.isEmpty()) return
        val list = if (shuffle) tracks.shuffled() else tracks
        val start = if (shuffle) 0 else startIndex.coerceIn(0, list.size - 1)
        if (linked() != null) { sendQueue(list, start, 0.0); return }
        val items = list.map { MediaItems.track(api, it) }
        withController { c ->
            c.setMediaItems(items, start, 0L)
            c.prepare()
            c.play()
        }
    }

    /** Play `tracks` from `index` at `positionMs` (picking up another device's spot). */
    fun playFrom(tracks: List<Track>, index: Int, positionMs: Long) {
        if (tracks.isEmpty()) return
        val start = index.coerceIn(0, tracks.size - 1)
        if (linked() != null) { sendQueue(tracks, start, positionMs / 1000.0); return }
        val items = tracks.map { MediaItems.track(api, it) }
        withController { c ->
            c.setMediaItems(items, start, positionMs.coerceAtLeast(0L))
            c.prepare()
            c.play()
        }
    }

    fun playNext(track: Track) {
        if (viaLink("play_next") { put("tracks", RemoteDevice.slimList(listOf(track))) }) return
        withController { c ->
            val at = if (c.mediaItemCount == 0) 0 else c.currentMediaItemIndex + 1
            c.addMediaItem(at, MediaItems.track(api, track))
            if (c.mediaItemCount == 1) { c.prepare(); c.play() }
        }
    }

    fun addToQueue(track: Track) {
        if (viaLink("enqueue") { put("tracks", RemoteDevice.slimList(listOf(track))) }) return
        withController { c ->
            c.addMediaItem(MediaItems.track(api, track))
            if (c.mediaItemCount == 1) { c.prepare(); c.play() }
        }
    }

    fun togglePlay() {
        if (viaLink("toggle")) return
        withController { c ->
            if (c.isPlaying) c.pause() else {
                if (c.playbackState == Player.STATE_IDLE) c.prepare()
                if (c.playbackState == Player.STATE_ENDED) c.seekTo(0L)
                c.play()
            }
        }
    }

    fun next() {
        if (viaLink("next")) return
        withController { c -> if (c.hasNextMediaItem()) c.seekToNextMediaItem() }
    }

    /** Like the web player: back to the start after 3 s, otherwise the previous song. */
    fun previous() {
        if (viaLink("prev")) return
        withController { c ->
            if (positionMs() > 3000L || !c.hasPreviousMediaItem()) seekTo(0L) else c.seekToPreviousMediaItem()
        }
    }

    /** Seek in song time. A server-made stream can't seek in the player: the service re-opens it at the point. */
    fun seekTo(positionMs: Long) {
        if (viaLink("seek") { put("value", positionMs.coerceAtLeast(0L) / 1000.0) }) {
            _positionMs.value = positionMs.coerceAtLeast(0L)
            return
        }
        withController { c ->
            val pos = positionMs.coerceAtLeast(0L)
            if (serverStream()) {
                val args = Bundle().apply { putLong(EncorePlaybackService.ARG_POSITION_MS, pos) }
                c.sendCustomCommand(SessionCommand(EncorePlaybackService.CMD_SEEK, Bundle.EMPTY), args)
            } else {
                c.seekTo(pos)
            }
            _positionMs.value = pos
        }
    }

    fun skipTo(index: Int) {
        // linked: a tap in its queue plays that queue from there (player.js queue taps → play_queue)
        if (linked() != null) {
            val q = _queue.value
            if (index in q.indices) sendQueue(q, index, 0.0)
            return
        }
        withController { c ->
            if (index in 0 until c.mediaItemCount) {
                c.seekTo(index, 0L)
                c.play()
            }
        }
    }

    fun removeAt(index: Int) {
        if (notWhileLinked()) return
        withController { c -> if (index in 0 until c.mediaItemCount) c.removeMediaItem(index) }
    }

    /** Move a queued song (not the one playing) to another place after the current one. */
    fun move(from: Int, to: Int) {
        if (notWhileLinked()) return
        withController { c ->
            val cur = c.currentMediaItemIndex
            if (from == cur || from !in 0 until c.mediaItemCount || to <= cur || to >= c.mediaItemCount) return@withController
            c.moveMediaItem(from, to)
        }
    }

    /** Shuffle what comes after the current song. */
    fun shuffleUpNext() {
        if (notWhileLinked()) return
        withController { c ->
            val cur = c.currentMediaItemIndex
            val n = c.mediaItemCount
            if (n - cur - 1 < 2) return@withController
            val rest = ArrayList<MediaItem>()
            for (i in cur + 1 until n) rest.add(c.getMediaItemAt(i))
            rest.shuffle()
            c.removeMediaItems(cur + 1, n)
            c.addMediaItems(rest)
        }
    }

    /** Signing out: stop and forget the queue. */
    fun stopAndClear() {
        withController { c ->
            c.stop()
            c.clearMediaItems()
        }
    }

    // ---- the ⋯ menu (PlayerMenuActions) ---------------------------------------------------------------

    override fun goToArtist() {
        val t = _nowPlaying.value ?: return
        val name = t.albumArtist.ifEmpty { t.artist }
        if (name.isNotEmpty()) onOpenArtist?.invoke(name)
    }

    override fun viewAlbum() {
        val t = _nowPlaying.value ?: return
        if (t.albumId.isNotEmpty()) onOpenAlbum?.invoke(t.albumId)
    }

    override fun addToPlaylist() {
        val t = _nowPlaying.value ?: return
        onAddToPlaylist?.invoke(t)
    }

    override fun lyrics() { if (_nowPlaying.value != null) _lyricsOpen.value = true }

    /** POST /api/library/tracks/{id}/share → copy the link and offer the system share sheet (web/js/ui.js shareTrack). */
    override fun shareLink() {
        val t = _nowPlaying.value ?: return
        scope.launch {
            try {
                val url = withContext(Dispatchers.IO) { api.shareLink(t.id) }
                try {
                    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                    cm?.setPrimaryClip(ClipData.newPlainText("Encore", url))
                } catch (_: Exception) {}
                Toasts.show("Link copied – anyone with it can play this song", "ok")
                val send = Intent(Intent.ACTION_SEND).apply {
                    type = "text/plain"
                    putExtra(Intent.EXTRA_SUBJECT, t.title)
                    putExtra(Intent.EXTRA_TEXT, "${t.title} – ${t.artist}\n$url")
                }
                context.startActivity(Intent.createChooser(send, "Share link").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't create the link", "err")
            }
        }
    }

    override fun signalPath() {
        if (_nowPlaying.value == null) return
        // the sheet describes this phone's stream; linked, say what the other device reports
        val l = linked()
        if (l != null) { Toasts.show(l.signalLabel.ifEmpty { "Playing on ${l.name}" }, "info"); return }
        _signalPathOpen.value = true
    }

    override fun upNext() { _upNextOpen.value = true }

    fun closeLyrics() { _lyricsOpen.value = false }
    fun closeSignalPath() { _signalPathOpen.value = false }
    fun closeUpNext() { _upNextOpen.value = false }

    // ---- state -----------------------------------------------------------------------------------------

    private fun updateQuality() {
        val o = qualityOverride
        if (o != null) {
            _qualityBadge.value = o.first
            _qualityTitle.value = o.second
            return
        }
        val current = _nowPlaying.value
        val info = _streamInfo.value
        if (current == null) {
            _qualityBadge.value = null
            _qualityTitle.value = null
        } else if (info != null && info.trackId == current.id && info.qualityBadge.isNotEmpty()) {
            _qualityBadge.value = info.qualityBadge
            _qualityTitle.value = info.qualityTitle
        } else {
            // until the service reports the stream it chose: the file as it is (web `quality()` for the original)
            _qualityBadge.value = (current.format.uppercase() + if (current.bitrate > 0) " ${current.bitrate}k" else "").trim().ifEmpty { null }
            _qualityTitle.value = "Original file, no conversion"
        }
    }

    private fun refresh() {
        linked()?.let { applyLink(it); return }
        val c = controller ?: return
        val queue = ArrayList<Track>(c.mediaItemCount)
        for (i in 0 until c.mediaItemCount) {
            val item: MediaItem = c.getMediaItemAt(i)
            MediaItems.toTrack(item)?.let { queue.add(it) }
        }
        val current = c.currentMediaItem?.let { MediaItems.toTrack(it) }
        _nowPlaying.value = current
        // a server-made stream reports only the length it has made so far: trust the library's duration
        val knownMs = ((current?.durationSec ?: 0.0) * 1000).toLong()
        val duration = if (serverStream() && knownMs > 0) knownMs else if (c.duration > 0) c.duration else knownMs
        _state.value = PlayerUiState(
            connected = true,
            current = current,
            isPlaying = c.isPlaying,
            buffering = c.playbackState == Player.STATE_BUFFERING,
            durationMs = duration,
            index = c.currentMediaItemIndex,
            queue = queue,
            hasNext = c.hasNextMediaItem(),
            error = c.playerError?.message,
        )
        // the seam flows (read by the design's screens)
        _isPlaying.value = c.isPlaying
        _durationMs.value = duration
        _queue.value = queue
        _queueIndex.value = if (current == null) -1 else c.currentMediaItemIndex
        _positionMs.value = c.currentPosition.coerceAtLeast(0L) + offsetMs()
        if (current == null) { _lyricsOpen.value = false; _signalPathOpen.value = false; _upNextOpen.value = false }
        updateQuality()
        mainHandler.removeCallbacks(positionTick)
        if (c.isPlaying) mainHandler.postDelayed(positionTick, 500L)
    }
}
