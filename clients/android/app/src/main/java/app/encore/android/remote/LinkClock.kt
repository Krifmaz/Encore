package app.encore.android.remote

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.doubleOrNull

/**
 * How the linked device's seek bar keeps time on this phone: its check-ins are the truth, and between them the
 * position moves on here.
 *
 * The bar used to jump back and forth because every copy of a check-in restarted the clock. The linked device
 * checks in every 2 s; the server pushes that state at once and this phone also asks for it every 2 s, so the
 * same check-in arrives twice, the second time up to 2 s late. Taking "now" as the moment its position was true
 * put the bar back by that much, and the moving-on put it forward again. Now each state is dated by when the
 * server got it (`seen`, server time, put on this phone's clock by [ClockOffset]), a copy of a check-in already
 * shown changes nothing about the time, a check-in made before a command of ours reached the device doesn't undo
 * what the command showed, and a small step back while it plays (network jitter) is not drawn.
 */
object LinkClock {
    /** A step back smaller than this on the same playing song is jitter, not a seek: the bar doesn't go back. */
    const val JITTER_MS = 1500L

    /**
     * After a command of ours (play, pause, seek …) the check-ins of the next this-many ms may still have been
     * made before the device ran it: they don't undo what the command showed.
     */
    const val COMMAND_GRACE_MS = 1500L

    /** The keys of a state that move with time or that a command of ours sets ahead of the device's own word. */
    private val LIVE_KEYS = listOf("position", "playing", "volume", "shuffle", "repeat")

    /**
     * The link after a check-in of its device arrived.
     *
     * @param cur the link as it shows now.
     * @param state the device's state in this check-in.
     * @param seenMs when the server got this check-in (server clock, ms); 0 when the server didn't say.
     * @param arrivalMs when it reached this phone (this phone's clock, ms).
     * @param offsetMs this phone's clock minus the server's ([ClockOffset.offsetMs]), or null when unknown.
     */
    fun next(cur: Link, state: JsonObject, seenMs: Long, arrivalMs: Long, offsetMs: Long?): Link {
        val sameSong = sameSong(cur.state, state)
        // a copy of a check-in already shown (pushed, then fetched): its time and live keys stay as they are;
        // the rest is taken (a fetched copy carries more: the server stamps artist credits and covers on it)
        if (seenMs > 0 && seenMs == cur.seenMs && sameSong) return keepLive(cur, state)
        // when its position was true, on this phone's clock (never later than it arrived)
        val anchor = if (seenMs > 0 && offsetMs != null) (seenMs + offsetMs).coerceAtMost(arrivalMs) else arrivalMs
        // made before a command of ours reached the device: what the command showed stands
        if (cur.commandAt > 0 && anchor < cur.commandAt + COMMAND_GRACE_MS && sameSong) {
            return keepLive(cur, state).copy(seenMs = seenMs)
        }
        val fresh = cur.copy(state = state, online = true, at = anchor, seenMs = seenMs, commandAt = 0L)
        // a small step back on the same playing song is jitter: keep moving on from where the bar is
        if (sameSong && cur.playing && fresh.playing) {
            val was = cur.positionSec(arrivalMs)
            val now = fresh.positionSec(arrivalMs)
            if (now < was && (was - now) * 1000 < JITTER_MS) return keepLive(cur, state).copy(seenMs = seenMs, commandAt = 0L)
        }
        return fresh
    }

    /** Is [b] the same song as [a] (same id, or the same title when neither has an id)? */
    private fun sameSong(a: JsonObject, b: JsonObject): Boolean {
        val ia = num(a["id"])
        val ib = num(b["id"])
        if (ia != null && ib != null) return ia == ib
        return str(a["title"]) == str(b["title"])
    }

    /** [state] with [cur]'s live keys (see [LIVE_KEYS]) and its time, online: what the bar shows doesn't change. */
    private fun keepLive(cur: Link, state: JsonObject): Link {
        val merged = state.toMutableMap()
        for (k in LIVE_KEYS) {
            val v = cur.state[k]
            if (v != null) merged[k] = v else merged.remove(k)
        }
        return cur.copy(state = JsonObject(merged), online = true)
    }

    /** A JSON number, or null. */
    private fun num(e: JsonElement?): Double? = (e as? JsonPrimitive)?.let { if (it.isString) null else it.doubleOrNull }

    /** A JSON string's text ("" for anything else). */
    private fun str(e: JsonElement?): String = (e as? JsonPrimitive)?.takeIf { it.isString }?.content ?: ""
}

/**
 * This phone's clock minus the server's, from when check-ins arrived here and when the server got them.
 *
 * Each sample is the real difference plus how long the state took to get here, so the smallest of the recent
 * samples is the best guess (the way NTP picks its fastest exchange). Only the last [keep] samples count, so a
 * clock that is set anew (a time zone trip, a network time fix) is followed within a minute or so.
 */
class ClockOffset(private val keep: Int = 30) {
    /** the latest samples, ms, oldest first */
    private val samples = ArrayDeque<Long>()

    /** A check-in the server got at [seenMs] (server clock) reached this phone at [arrivalMs] (its clock). */
    @Synchronized
    fun add(arrivalMs: Long, seenMs: Long) {
        if (seenMs <= 0) return
        samples.addLast(arrivalMs - seenMs)
        while (samples.size > keep) samples.removeFirst()
    }

    /** The best guess so far, or null before the first sample. */
    val offsetMs: Long?
        @Synchronized get() = samples.minOrNull()
}
