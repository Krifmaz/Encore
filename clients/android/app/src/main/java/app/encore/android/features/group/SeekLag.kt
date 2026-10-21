package app.encore.android.features.group

/**
 * What a seek made to catch up with the group costs on this phone (pure Kotlin, tested in SyncRulesTest), and when the
 * last one was made (no other drift seek for 8 s after it: [SyncRules.HOLD_MS]).
 *
 * A seek starts playing again some time after it was asked: a whole file buffers at the new spot, and a server-made
 * stream (a transcode, Data saver or Auto's AAC, the server's Cadenza) is asked for again at `t=`. By then the group
 * has moved on by that much, so a seek to the group's position lands behind it.
 *
 * So each seek aims ahead by the lag, and what it really took is measured where it lands (the first look once it
 * plays: the lead it was given minus the drift found). The lag is the running median of the last 5 of those, for whole
 * files and server-made streams apart, between 0 and 3 s: one slow restart on a busy server no longer teaches a lag
 * that makes the next, fast seek overshoot. Until a seek was measured a server-made stream counts with 0.7 s.
 *
 * Main thread only (GroupSync).
 */
class SeekLag {
    /** what the last seeks took, ms, oldest first: [0] whole files (and the songs saved on this phone), [1] server-made */
    private val recent = arrayOf(ArrayList<Long>(), ArrayList<Long>())
    /** the kind of stream of the seek whose result is awaited, or -1 */
    private var waitKind = -1
    private var waitSong = ""
    /** how far ahead that seek aimed (ms) */
    private var waitLead = 0L
    /** when the last seek was made (elapsed ms) */
    private var seekAt = NEVER

    /** A seek's result is awaited: the next look while playing calls [landed]. */
    val waiting: Boolean get() = waitKind >= 0

    /**
     * Where to seek in [song] (the queue entry's id) for the group's [expectedMs], on a whole file or a server-made
     * stream ([serverMade]), at [nowMs]: ahead by the lag of that kind. Its result is awaited ([landed]).
     */
    fun target(song: String, expectedMs: Long, serverMade: Boolean, nowMs: Long): Long {
        val lead = lagMs(serverMade)
        waitKind = kind(serverMade)
        waitSong = song
        waitLead = lead
        seekAt = nowMs
        return expectedMs + lead
    }

    /**
     * The first look while playing after a seek made with [target]: [driftMs] = this phone's position − the group's
     * in [song]. What the seek took (its lead minus the drift) joins the last ones of its kind.
     */
    fun landed(song: String, driftMs: Long) {
        val k = waitKind
        if (k < 0) return
        waitKind = -1
        // another song by now: nothing to learn from it
        if (song != waitSong) return
        val list = recent[k]
        list.add((waitLead - driftMs).coerceIn(0L, MAX_LAG_MS))
        while (list.size > KEEP) list.removeAt(0)
    }

    /** The seek awaited is not one to learn from (a pause, the listener's own seek, leaving the group). */
    fun cancel() {
        waitKind = -1
    }

    /**
     * The group itself moved (someone's seek, play or jump): the next seek follows it at once, without waiting out
     * the hold of the last one.
     */
    fun release() {
        seekAt = NEVER
    }

    /** Time since the last seek (ms). */
    fun sinceSeekMs(nowMs: Long): Long = nowMs - seekAt

    /** The lag of whole files or server-made streams (ms): the median of the last [KEEP] seeks. */
    fun lagMs(serverMade: Boolean): Long {
        val list = recent[kind(serverMade)]
        if (list.isEmpty()) return if (serverMade) SERVER_PRIOR_MS else 0L
        return GroupClock.median(list)
    }

    private fun kind(serverMade: Boolean): Int = if (serverMade) 1 else 0

    companion object {
        /** the seeks the median is taken over */
        const val KEEP = 5
        const val MAX_LAG_MS = 3000L
        /** a server-made stream's lag before one was measured: a new conversion starts in about this */
        const val SERVER_PRIOR_MS = 700L
        private const val NEVER = Long.MIN_VALUE / 4
    }
}
