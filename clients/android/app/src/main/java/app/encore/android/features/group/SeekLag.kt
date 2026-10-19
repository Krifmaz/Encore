package app.encore.android.features.group

/**
 * What a seek made to catch up with the group costs on this phone (pure Kotlin, tested in GroupRulesTest).
 *
 * A seek starts playing again some time after it was asked: a whole file buffers at the new spot, and a server-made
 * stream (a transcode, Data saver or adaptive AAC, the server's Cadenza) is asked for again at `t=`. By then the group
 * has moved on by that much, so a seek to the group's position lands behind it, and with a lag above 0.5 s the next
 * look would seek again, and again (an audible restart every few seconds on a slow connection).
 *
 * So the lag is learnt from where each seek lands (the first look once it plays: this phone's position minus the
 * group's), for whole files and server-made streams apart, capped at 3 s, and added to the next seek's target. When
 * two seeks within 15 s still land more than 0.5 s off (a lag that varies too much to learn), seeking stops for that
 * song: only a jump of more than 3 s seeks (the host seeked), and the speed nudge closes the rest.
 *
 * Main thread only (GroupSync).
 */
class SeekLag {
    /** the lag learnt, ms: [0] whole files (and the songs saved on this phone), [1] server-made streams */
    private val lag = longArrayOf(0L, 0L)
    /** the kind of stream of the seek whose result is awaited, or -1 */
    private var waitKind = -1
    private var waitSong = ""
    /** the last seek that landed more than 0.5 s off (any monotonic clock, ms) */
    private var missAt = 0L
    private var missed = false
    /** the song (its queue entry) where seeking stopped */
    private var offFor: String? = null

    /** A seek's result is awaited: the next look while playing calls [landed]. */
    val waiting: Boolean get() = waitKind >= 0

    /**
     * Where to seek in [song] (the queue entry's id) for the group's [expectedMs], on a whole file or a server-made
     * stream ([serverMade]): ahead by the lag learnt for that kind. Its result is awaited ([landed]).
     */
    fun target(song: String, expectedMs: Long, serverMade: Boolean): Long {
        val k = if (serverMade) 1 else 0
        waitKind = k
        waitSong = song
        return expectedMs + lag[k]
    }

    /**
     * The first look while playing after a seek made with [target]: [driftMs] = this phone's position − the group's
     * in [song], at [nowMs]. The lag of that kind of stream is corrected by what the seek missed.
     */
    fun landed(song: String, driftMs: Long, nowMs: Long) {
        val k = waitKind
        if (k < 0) return
        waitKind = -1
        // another song by now: nothing to learn from it
        if (song != waitSong) return
        lag[k] = (lag[k] - driftMs).coerceIn(0L, MAX_LAG_MS)
        if (Math.abs(driftMs) > SEEK_MS) {
            if (missed && nowMs - missAt <= MISS_WINDOW_MS) offFor = song
            missed = true
            missAt = nowMs
        }
    }

    /** The seek awaited is not one to learn from (a pause, the listener's own seek, leaving the group). */
    fun cancel() {
        waitKind = -1
    }

    /** Above this drift (ms) a seek is made: 0.5 s, or 3 s in a song where seeking did not work. */
    fun seekAboveMs(song: String): Long = if (song == offFor) BACKED_OFF_MS else SEEK_MS

    /** The lag learnt for whole files or server-made streams (ms). */
    fun lagMs(serverMade: Boolean): Long = lag[if (serverMade) 1 else 0]

    companion object {
        /** the sync rule's seek threshold */
        const val SEEK_MS = 500L
        /** the threshold in a song where seeking did not work */
        const val BACKED_OFF_MS = 3000L
        const val MAX_LAG_MS = 3000L
        const val MISS_WINDOW_MS = 15_000L
    }
}
