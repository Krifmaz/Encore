package app.encore.android.playback

import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import app.encore.android.data.api.EncoreApi
import app.encore.android.features.offline.OfflineMode
import java.util.concurrent.ExecutorService

/**
 * Plays and now-playing, with the same rules and endpoints as the web player: "now playing" when a song starts
 * (`POST /api/library/now-playing`), a play (`POST /api/library/plays`) at half the song or 4 minutes, whichever
 * comes first, songs under 30 s at their end; a song that runs to its end (gapless, or faded out by a song
 * transition) counts with its full length. The server passes them on (ListenBrainz, Last.fm). A play the server
 * can't take (offline) waits in the outbox (features/offline) and goes when it answers again. A move to a later song
 * before the play counted (and after the first two seconds) is a skip (`POST /api/taste/skip`), which the
 * recommendations take as a mild "not now". When a song whose play counted is over, how far it got goes to the
 * server too (`POST /api/taste/listened`): a song heard to its end counts for more than one left soon after half.
 * Main thread only.
 */
class Scrobbler(
    private val player: ExoPlayer,
    private val current: CurrentStream,
    private val api: EncoreApi,
    private val io: ExecutorService,
) {
    private var announced: Long? = null
    private var logged = false
    private var lastId: Long? = null
    private var lastDurationSec = 0.0
    /** how far into the last song the player was at the last [check] (seconds) */
    private var lastPositionSec = 0.0
    /** the queue index of the last song at the last [check] */
    private var lastIndex = -1

    /** set by the service: in a group listen now (the group moves songs on, so there are no skips to tell) */
    var inGroup: () -> Boolean = { false }

    /** "Now playing" for the song, once per song. */
    fun announce(id: Long) {
        if (announced == id) return
        announced = id
        // offline: nobody to tell (and no request held up on a server that doesn't answer)
        if (OfflineMode.active.value) return
        io.execute { try { api.nowPlaying(id) } catch (_: Exception) {} }
    }

    /**
     * Another song (not the same one reopened): a song that ran to its end ([Player.MEDIA_ITEM_TRANSITION_REASON_AUTO],
     * a repeat) counts with its full length; the new one starts unplayed.
     */
    fun songChanged(reason: Int) {
        val prev = lastId
        if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_AUTO || reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT) {
            // logPlay sets `logged`: the report below must only follow a play that counted before this moment
            if (!logged && prev != null && lastDurationSec > 0) { logPlay(prev, lastDurationSec); logged = false }
        }
        // a counted play is over: how much of it was heard (all of it when it ran to its end)
        val ranToEnd = reason == Player.MEDIA_ITEM_TRANSITION_REASON_AUTO || reason == Player.MEDIA_ITEM_TRANSITION_REASON_REPEAT
        val wasLogged = logged
        if (wasLogged && prev != null) reportHeard(prev, if (ranToEnd && lastDurationSec > 0) lastDurationSec else lastPositionSec)
        // Next (a seek to a later song) before the play counted: a skip
        if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_SEEK && !wasLogged && prev != null && lastPositionSec >= 2.0 &&
            player.currentMediaItemIndex > lastIndex && !inGroup() && !OfflineMode.active.value) {
            val secs = lastPositionSec
            io.execute { try { api.skip(prev, secs) } catch (_: Exception) {} }
        }
        logged = false
        lastId = null
        lastDurationSec = 0.0
        lastPositionSec = 0.0
        lastIndex = -1
    }

    /** The queue ended: the last song ran to its end. */
    fun ended() {
        val id = lastId
        // counted earlier: it was heard to the end
        if (logged && id != null && lastDurationSec > 0) reportHeard(id, lastDurationSec)
        if (!logged && id != null && lastDurationSec > 0) logPlay(id, lastDurationSec)
        logged = true
    }

    /**
     * A song transition moves the player on to the next song with a seek while the old one fades out: the old song
     * ran to its end, so it counts like a gapless hand-over, with its full length.
     */
    fun handedOver(item: MediaItem) {
        val id = MediaItems.trackId(item.mediaId) ?: return
        if (id != lastId) return
        if (!logged && lastDurationSec > 0) logPlay(id, lastDurationSec)
    }

    /** Once a second while playing: has the song played long enough to count? */
    fun check() {
        val item = player.currentMediaItem ?: return
        val id = MediaItems.trackId(item.mediaId) ?: return
        val p = current.absPositionMs() / 1000.0
        val d = current.durationSec()
        lastId = id
        lastDurationSec = d
        lastPositionSec = p
        lastIndex = player.currentMediaItemIndex
        if (logged) return
        // half the song or 4 minutes, whichever comes first; songs under 30 s at their end
        val hit = if (d > 0) (if (d < 30) p >= d - 1 else p >= minOf(d / 2, 240.0)) else p >= 30
        if (hit) logPlay(id, p)
    }

    /**
     * How far into song `id` the listener got (`seconds`) once its counted play is over, for the recommendations.
     * Offline it is dropped: the play itself waits in the outbox and counts as an ordinary play.
     */
    private fun reportHeard(id: Long, seconds: Double) {
        if (seconds <= 0 || OfflineMode.active.value) return
        io.execute { try { api.listened(id, seconds) } catch (_: Exception) {} }
    }

    private fun logPlay(id: Long, seconds: Double) {
        logged = true
        // the outbox is a file: written on `io`, never on the thread that drives the player
        if (OfflineMode.active.value) { io.execute { OfflineMode.queuePlay(id, seconds) }; return }
        io.execute {
            try {
                api.logPlay(id, seconds)
                // the server answers again: plays and likes from a short gap go now
                OfflineMode.syncSoon()
            } catch (e: Exception) {
                if (OfflineMode.unreachable(e)) OfflineMode.queuePlay(id, seconds)
            }
        }
    }
}
