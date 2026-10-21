package app.encore.android.features.group

import app.encore.android.data.prefs.Quality

// The group listen's sync contract (Oct 21; the same on the web, the guest page, the phone and the desktop) as pure
// Kotlin, tested in SyncRulesTest. GroupSync (the player), GroupListen (the clock) and AdaptiveQuality (the rung)
// act on what these decide. The owner: "small skips/jumps" on every client; the causes are in
// wf/group-choppy-clients.md and wf/group-choppy-server.md.

/**
 * What to do about the distance between this phone and the group, at each look (every 0.5 s while playing).
 *
 * Drift = this phone's position − the group's (positive: ahead).
 * - Under 40 ms: nothing.
 * - 40 ms to 1.5 s: the playback speed is nudged in proportion to the drift, at most ±2 % (0.98–1.02), eased in and
 *   out, and kept until the drift is under 20 ms. The audio track keeps the pitch. Never a seek.
 * - Over 1.5 s: one hard seek (aimed ahead by [SeekLag]), then no other seek for 8 s: nudges only.
 * - A server-made stream (a transcode, Auto's or Data saver's AAC, the server's Cadenza while it renders: anything but
 *   the original file served with Range) is never seeked for drift under 3 s: each seek there is a new conversion on
 *   the server and a gap here.
 * - A player that can't change its speed (bit-perfect USB, Cadenza on this phone) can't nudge: it keeps the same
 *   seek bars (1.5 s, a server-made stream 3 s) and may sit up to that far off, since every seek there is a flush of
 *   the audio track, the skip the contract keeps out of 40 ms – 1.5 s.
 */
object SyncRules {
    /** below this drift (ms) nothing is done */
    const val DEAD_MS = 40L
    /** a nudge goes on until the drift is under this (ms) */
    const val SETTLED_MS = 20L
    /** above this drift (ms): one hard seek */
    const val SEEK_MS = 1500L
    /** a server-made stream is never seeked for drift under this (ms) */
    const val SERVER_SEEK_MS = 3000L
    /** after a hard seek no other drift seek for this long (ms): nudges only */
    const val HOLD_MS = 8000L
    /** the nudge at most: ±2 % */
    const val MAX_NUDGE = 0.02
    /** the nudge per ms of drift: 100 ms → 1 %, 200 ms and more → 2 % (a drift of 100 ms closes in about 16 s) */
    const val NUDGE_PER_MS = 0.0001
    /** how far the speed moves at one look (0.5 s) at most: in and out over about two seconds */
    const val EASE_STEP = 0.004
    /** a speed this close to the one playing is not set (every change costs the audio track a little) */
    const val MIN_CHANGE = 0.002
    /** a paused group: everyone at the same spot, within this (ms) */
    const val PAUSED_PLACE_MS = 500L
    /**
     * A song change up to this long (ms) before or after the group's is let be: the player moves into the next song by
     * itself, gapless, from what it has pre-buffered. Past it the player moves to the group's song.
     */
    const val SONG_GRACE_MS = 3000L

    /** What one look decides. */
    sealed class Act {
        object None : Act()
        /** one hard seek to the group's spot (ahead by the seek lag) */
        object Seek : Act()
        /** the playback speed to set now */
        data class Speed(val rate: Double) : Act()
    }

    /** Above this drift (ms) a hard seek is made (the same whether or not the speed may change). */
    fun seekAboveMs(serverMade: Boolean): Long = if (serverMade) SERVER_SEEK_MS else SEEK_MS

    /**
     * One look: [driftMs] (positive: ahead), the speed set now ([rate], 1.0 when not nudging), the stream's kind,
     * whether the speed may change, and the time since the last hard seek ([sinceSeekMs]).
     */
    fun decide(driftMs: Long, rate: Double, serverMade: Boolean, speedOk: Boolean, sinceSeekMs: Long): Act {
        val d = Math.abs(driftMs)
        if (d > seekAboveMs(serverMade) && sinceSeekMs >= HOLD_MS) return Act.Seek
        if (!speedOk) return if (rate != 1.0) Act.Speed(1.0) else Act.None
        val next = ease(rate, targetRate(driftMs, nudging = rate != 1.0))
        return if (next == rate) Act.None else Act.Speed(next)
    }

    /** The speed the drift asks for: 1.0 when settled, else 1 − drift × 0.01 % per ms, within 0.98–1.02. */
    fun targetRate(driftMs: Long, nudging: Boolean): Double {
        val d = Math.abs(driftMs)
        if (d < SETTLED_MS || (!nudging && d < DEAD_MS)) return 1.0
        val off = (driftMs * NUDGE_PER_MS).coerceIn(-MAX_NUDGE, MAX_NUDGE)
        return round3(1.0 - off)
    }

    /** From [rate] towards [target] by [EASE_STEP] at most; a small change that does not end the nudge is skipped. */
    fun ease(rate: Double, target: Double): Double {
        if (rate == target) return rate
        if (target != 1.0 && Math.abs(target - rate) < MIN_CHANGE) return rate
        return round3(rate + (target - rate).coerceIn(-EASE_STEP, EASE_STEP))
    }

    private fun round3(x: Double): Double = Math.round(x * 1000.0) / 1000.0

    /**
     * Song changes: this phone on entry [cur] at [posMs], the group on [groupIndex] at [groupPosMs]. True: wait, the
     * phone is moving into the group's song (or the group into the phone's) by itself, gapless, from what it has
     * pre-buffered.
     */
    fun waitForSongChange(cur: Int, posMs: Long, groupIndex: Int, groupPosMs: Long): Boolean =
        (groupIndex == cur + 1 && groupPosMs < SONG_GRACE_MS) || (cur == groupIndex + 1 && posMs < SONG_GRACE_MS)

    /**
     * Where to start a song this phone moves to at [ms] (ms, the group's spot plus the lag): a server-made stream
     * starts at 0 from what was pre-buffered when the spot is within [SERVER_SEEK_MS] (asking for it again at `t=`
     * would be a new conversion and a gap; the nudge closes the rest), else at the spot.
     */
    fun startAtMs(ms: Long, serverMade: Boolean): Long = if (serverMade && ms <= SERVER_SEEK_MS) 0L else ms

    /**
     * The group's queue on this player while the song playing stays the same one: how many songs after it
     * ([QueueEdit.keepAfter]) and before it ([QueueEdit.keepBefore]) already match, so only what changed is removed
     * and added. A song added at the end then leaves the next song, and what the player pre-buffered of it, alone.
     * [local] and [group] are track ids; [cur] and [index] the song playing in each.
     */
    fun queueEdit(local: List<Long>, cur: Int, group: List<Long>, index: Int): QueueEdit {
        var after = 0
        while (cur + 1 + after < local.size && index + 1 + after < group.size && local[cur + 1 + after] == group[index + 1 + after]) after++
        var before = 0
        while (cur - 1 - before >= 0 && index - 1 - before >= 0 && local[cur - 1 - before] == group[index - 1 - before]) before++
        return QueueEdit(after, before)
    }

    data class QueueEdit(val keepAfter: Int, val keepBefore: Int)
}

/** One round trip to GET /api/time: sent and received on this phone's clock (ms since boot), the server's time between. */
data class ClockSample(val sentMs: Long, val serverMs: Long, val receivedMs: Long) {
    val rttMs: Long get() = receivedMs - sentMs
    /** the server's clock minus this phone's, taking the server's time as the middle of the round trip */
    val offsetMs: Long get() = serverMs - (sentMs + rttMs / 2)
    val valid: Boolean get() = receivedMs >= sentMs && serverMs > 0
}

/**
 * The server's clock as this phone sees it: the offset (server − this phone's time since boot, ms).
 * - At join: 8 samples; the 3 with the shortest round trip; their median.
 * - Every 60 s: 3 samples; those whose round trip is over twice the best seen are left out (a busy server answers late
 *   and its time is off by up to half the round trip); their median is blended in (30 % of the difference), and the
 *   offset never moves by more than 50 ms at once. So a slow answer or a busy minute can't step every member's
 *   expected position, and an echo between members fades instead of jumping.
 * - On a fast network the bar is at least 20 ms (twice 1–2 ms would turn every sample away). When five refreshes in a
 *   row have nothing to keep (the network changed: Wi-Fi to mobile data), the best seen becomes the best of the last.
 * Any thread.
 */
class GroupClock {
    @Volatile var offsetMs: Long = 0L
        private set
    @Volatile var measured: Boolean = false
        private set
    /** the shortest round trip seen since the join (ms) */
    var bestRttMs: Long = Long.MAX_VALUE
        private set
    private var empty = 0

    /** The first measure (a join, a start, the app back in its group): replaces the offset. False with no sample. */
    @Synchronized
    fun join(samples: List<ClockSample>): Boolean {
        val ok = samples.filter { it.valid }
        if (ok.isEmpty()) return false
        val best = ok.sortedBy { it.rttMs }.take(JOIN_KEEP)
        offsetMs = median(best.map { it.offsetMs })
        bestRttMs = best.first().rttMs
        measured = true
        empty = 0
        return true
    }

    /** A refresh: blended in, at most [MAX_STEP_MS] at once. False when nothing was kept (or no sample). */
    @Synchronized
    fun refresh(samples: List<ClockSample>): Boolean {
        val ok = samples.filter { it.valid }
        if (ok.isEmpty()) return false
        if (!measured) return join(ok)
        val newBest = ok.minOf { it.rttMs }
        if (newBest < bestRttMs) bestRttMs = newBest
        val bar = maxOf(2 * bestRttMs, MIN_BAR_MS)
        val kept = ok.filter { it.rttMs <= bar }
        if (kept.isEmpty()) {
            if (++empty >= RESET_AFTER) { bestRttMs = newBest; empty = 0 }
            return false
        }
        empty = 0
        val est = median(kept.map { it.offsetMs })
        val step = Math.round(WEIGHT * (est - offsetMs)).coerceIn(-MAX_STEP_MS, MAX_STEP_MS)
        offsetMs += step
        return true
    }

    companion object {
        const val JOIN_SAMPLES = 8
        const val JOIN_KEEP = 3
        const val REFRESH_SAMPLES = 3
        const val REFRESH_EVERY_MS = 60_000L
        const val WEIGHT = 0.3
        const val MAX_STEP_MS = 50L
        const val MIN_BAR_MS = 20L
        const val RESET_AFTER = 5

        fun median(xs: List<Long>): Long {
            if (xs.isEmpty()) return 0L
            val s = xs.sorted()
            val m = s.size / 2
            return if (s.size % 2 == 1) s[m] else Math.floorDiv(s[m - 1] + s[m], 2L)
        }
    }
}

/**
 * Auto quality in a group listen: the rung is fixed for the length of a song. No step down or up mid-song for the
 * estimate; a change waits for the next song. The original file is preferred whenever the estimate covers it (a seek
 * in it is cheap, and a server-made stream can't seek). A real stall (the buffer ran empty) may still drop the rung,
 * once per song.
 */
object GroupQuality {
    /** The rung a song starts on in a group: the original when the estimate covers its bitrate (no 25 % margin). */
    fun startRung(estimate: Double?, origKbps: Int): Int {
        if (estimate == null) return 0
        if (Quality.needed(0, origKbps) <= estimate) return 0
        return Quality.chooseRung(estimate, origKbps)
    }

    /** May Auto change the rung of the song playing? Alone: as always. In a group: only for a stall, once per song. */
    fun mayChange(inGroup: Boolean, stall: Boolean, droppedThisSong: Boolean): Boolean = !inGroup || (stall && !droppedThisSong)
}
