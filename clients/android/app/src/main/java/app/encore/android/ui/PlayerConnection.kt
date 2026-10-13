package app.encore.android.ui

import android.content.ComponentName
import android.content.Context
import android.os.Handler
import android.os.Looper
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.playback.EncorePlaybackService
import app.encore.android.playback.MediaItems
import com.google.common.util.concurrent.ListenableFuture
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
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
 */
class PlayerConnection(private val context: Context, private val api: EncoreApi) {
    private val mainHandler = Handler(Looper.getMainLooper())
    private val mainExecutor = Executor { r -> mainHandler.post(r) }
    private var future: ListenableFuture<MediaController>? = null
    private var controller: MediaController? = null

    private val _state = MutableStateFlow(PlayerUiState())
    val state: StateFlow<PlayerUiState> = _state.asStateFlow()

    /** Commands made before the controller is ready run once it is. */
    private val pending = ArrayList<(MediaController) -> Unit>()

    private val listener = object : Player.Listener {
        override fun onEvents(player: Player, events: Player.Events) {
            refresh()
        }
    }

    fun connect() {
        if (future != null) return
        val token = SessionToken(context, ComponentName(context, EncorePlaybackService::class.java))
        val f = MediaController.Builder(context, token).buildAsync()
        future = f
        f.addListener({
            try {
                val c = f.get()
                controller = c
                c.addListener(listener)
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
            connect()
        }
    }

    /** The position right now (the UI polls it while playing). */
    fun positionMs(): Long = controller?.currentPosition ?: 0L

    // ---- commands --------------------------------------------------------------------------------------

    fun play(tracks: List<Track>, startIndex: Int = 0, shuffle: Boolean = false) {
        if (tracks.isEmpty()) return
        val list = if (shuffle) tracks.shuffled() else tracks
        val start = if (shuffle) 0 else startIndex.coerceIn(0, list.size - 1)
        val items = list.map { MediaItems.track(api, it) }
        withController { c ->
            c.setMediaItems(items, start, 0L)
            c.prepare()
            c.play()
        }
    }

    fun playNext(track: Track) {
        withController { c ->
            val at = if (c.mediaItemCount == 0) 0 else c.currentMediaItemIndex + 1
            c.addMediaItem(at, MediaItems.track(api, track))
            if (c.mediaItemCount == 1) { c.prepare(); c.play() }
        }
    }

    fun addToQueue(track: Track) {
        withController { c ->
            c.addMediaItem(MediaItems.track(api, track))
            if (c.mediaItemCount == 1) { c.prepare(); c.play() }
        }
    }

    fun togglePlay() {
        withController { c ->
            if (c.isPlaying) c.pause() else {
                if (c.playbackState == Player.STATE_IDLE) c.prepare()
                if (c.playbackState == Player.STATE_ENDED) c.seekTo(0L)
                c.play()
            }
        }
    }

    fun next() { withController { c -> if (c.hasNextMediaItem()) c.seekToNextMediaItem() } }

    /** Like the web player: back to the start after 3 s, otherwise the previous song. */
    fun previous() {
        withController { c ->
            if (c.currentPosition > 3000L || !c.hasPreviousMediaItem()) c.seekTo(0L) else c.seekToPreviousMediaItem()
        }
    }

    fun seekTo(positionMs: Long) { withController { c -> c.seekTo(positionMs.coerceAtLeast(0L)) } }

    fun skipTo(index: Int) {
        withController { c ->
            if (index in 0 until c.mediaItemCount) {
                c.seekTo(index, 0L)
                c.play()
            }
        }
    }

    fun removeAt(index: Int) { withController { c -> if (index in 0 until c.mediaItemCount) c.removeMediaItem(index) } }

    /** Signing out: stop and forget the queue. */
    fun stopAndClear() {
        withController { c ->
            c.stop()
            c.clearMediaItems()
        }
    }

    private fun refresh() {
        val c = controller ?: return
        val queue = ArrayList<Track>(c.mediaItemCount)
        for (i in 0 until c.mediaItemCount) {
            val item: MediaItem = c.getMediaItemAt(i)
            MediaItems.toTrack(item)?.let { queue.add(it) }
        }
        val current = c.currentMediaItem?.let { MediaItems.toTrack(it) }
        _state.value = PlayerUiState(
            connected = true,
            current = current,
            isPlaying = c.isPlaying,
            buffering = c.playbackState == Player.STATE_BUFFERING,
            durationMs = if (c.duration > 0) c.duration else ((current?.durationSec ?: 0.0) * 1000).toLong(),
            index = c.currentMediaItemIndex,
            queue = queue,
            hasNext = c.hasNextMediaItem(),
            error = c.playerError?.message,
        )
    }
}
