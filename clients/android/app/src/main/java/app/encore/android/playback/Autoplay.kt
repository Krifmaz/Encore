package app.encore.android.playback

import android.os.Handler
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.features.offline.OfflineMode
import app.encore.android.features.offline.OfflineStore
import java.util.concurrent.ExecutorService

/**
 * Settings → Playback → "Keep going with a shuffle of the whole library": when the queue runs out, 50 more songs
 * from `POST /api/library/shuffle`, never one played in this listening session (a fresh round once everything has
 * played). Offline, the shuffle is of the songs saved on this phone. Main thread only.
 */
class Autoplay(
    private val player: ExoPlayer,
    private val current: CurrentStream,
    private val resolver: StreamResolver,
    private val api: EncoreApi,
    private val io: ExecutorService,
    private val main: Handler,
    private val prefs: () -> PlaybackPrefs,
    private val released: () -> Boolean,
) {
    /** songs played in this listening session: they don't come back in the shuffle */
    private val played = LinkedHashSet<Long>()
    private var fetchingMore = false

    fun played(id: Long) {
        played.add(id)
    }

    /** At the last song (or past it): fetch more and, when the queue had ended, play on. */
    fun maybe() {
        if (fetchingMore || !prefs().autoplay) return
        val count = player.mediaItemCount
        if (count == 0 || player.repeatMode != Player.REPEAT_MODE_OFF) return
        if (player.currentMediaItemIndex < count - 1 && player.playbackState != Player.STATE_ENDED) return
        fetchingMore = true
        val exclude = ArrayList<Long>(played)
        for (i in 0 until count) MediaItems.trackId(player.getMediaItemAt(i).mediaId)?.let { exclude.add(it) }
        val cur = current.id
        io.execute {
            // the server can't be reached: the songs saved here (those not played yet)
            var more = if (OfflineMode.active.value) savedShuffle(exclude.toHashSet())
                else try { api.shuffle(50, exclude.distinct()) } catch (_: Exception) { emptyList() }
            var freshRound = false
            if (more.isEmpty() && exclude.isNotEmpty()) {
                // everything has played: start a fresh round
                freshRound = true
                more = if (OfflineMode.active.value) savedShuffle(listOfNotNull(cur).toHashSet())
                    else try { api.shuffle(50, listOfNotNull(cur)) } catch (_: Exception) { emptyList() }
            }
            main.post {
                if (freshRound) played.clear()
                fetchingMore = false
                if (released() || more.isEmpty()) return@post
                val ended = player.playbackState == Player.STATE_ENDED
                for (t in more) resolver.remember(StreamTrack.of(t))
                player.addMediaItems(more.map { MediaItems.track(api, it) })
                if (ended) {
                    player.seekTo(player.currentMediaItemIndex + 1, 0L)
                    player.prepare()
                    player.play()
                }
            }
        }
    }

    /** 50 songs saved on this phone, shuffled, none of `exclude`. */
    private fun savedShuffle(exclude: Set<Long>): List<app.encore.android.data.api.Track> =
        (OfflineStore.instance?.songsNow() ?: emptyList()).filter { it.id !in exclude }.shuffled().take(50).map { OfflineStore.trackOf(it) }
}
