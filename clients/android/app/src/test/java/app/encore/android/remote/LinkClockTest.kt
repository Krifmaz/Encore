package app.encore.android.remote

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The linked device's seek bar moves on smoothly: its check-ins are the truth, their copies and jitter aren't drawn. */
class LinkClockTest {
    /** A playing song's state at [pos] seconds. */
    private fun playing(pos: Double, id: Int = 7, playing: Boolean = true): JsonObject = buildJsonObject {
        put("id", id)
        put("title", "Last Dance With Mary Jane")
        put("duration", 185.0)
        put("playing", playing)
        put("position", pos)
    }

    /** Where the bar is at [now] (ms), to the millisecond. */
    private fun bar(l: Link, now: Long): Long = Math.round(l.positionSec(now) * 1000)

    @Test
    fun aSecondCopyOfACheckInDoesNotPutTheBarBack() {
        // the server got the check-in (position 60 s) at 10 000 on its clock; the clocks agree
        var l = Link("tv", "Shield", playing(60.0), at = 10_000L, seenMs = 10_000L)
        // the same check-in fetched 1.8 s later: the bar keeps moving on from the first copy
        l = LinkClock.next(l, playing(60.0), seenMs = 10_000L, arrivalMs = 11_800L, offsetMs = 0L)
        assertEquals(61_800L, bar(l, 11_800L))
    }

    @Test
    fun aCheckInIsDatedByWhenTheServerGotItNotWhenItArrived() {
        // shown a little behind (59 s at 10 000), so the check-in below is a step forward, not jitter
        val l0 = Link("tv", "Shield", playing(59.0), at = 10_000L, seenMs = 10_000L)
        // the next check-in (62 s at server 12 000), fetched late at 13 500; the phone's clock runs 50 ms ahead
        val l = LinkClock.next(l0, playing(62.0), seenMs = 12_000L, arrivalMs = 13_500L, offsetMs = 50L)
        assertEquals(63_450L, bar(l, 13_500L))
    }

    @Test
    fun aSmallStepBackWhilePlayingIsNotDrawn() {
        val l0 = Link("tv", "Shield", playing(60.0), at = 10_000L, seenMs = 10_000L)
        // the device says 61.7 s where the bar is at 62 s: jitter, the bar goes on from 62 s
        val l = LinkClock.next(l0, playing(61.7), seenMs = 12_000L, arrivalMs = 12_000L, offsetMs = 0L)
        assertEquals(62_000L, bar(l, 12_000L))
        assertEquals(12_000L, l.seenMs)
    }

    @Test
    fun aRealSeekBackIsShown() {
        val l0 = Link("tv", "Shield", playing(60.0), at = 10_000L, seenMs = 10_000L)
        val l = LinkClock.next(l0, playing(20.0), seenMs = 12_000L, arrivalMs = 12_000L, offsetMs = 0L)
        assertEquals(20_000L, bar(l, 12_000L))
    }

    @Test
    fun aCheckInMadeBeforeOurCommandDoesNotUndoIt() {
        // we sought to 120 s at 11 000; a check-in the device made just before it ran the seek arrives after
        val sought = Link("tv", "Shield", playing(120.0), at = 11_000L, seenMs = 10_000L, commandAt = 11_000L)
        val l = LinkClock.next(sought, playing(61.0), seenMs = 11_200L, arrivalMs = 11_250L, offsetMs = 0L)
        assertEquals(120_250L, bar(l, 11_250L))
        // past the grace the device's word counts again
        val later = LinkClock.next(l, playing(123.0), seenMs = 14_000L, arrivalMs = 14_000L, offsetMs = 0L)
        assertEquals(123_000L, bar(later, 14_000L))
        assertEquals(0L, later.commandAt)
    }

    @Test
    fun aNewSongStartsWhereTheDeviceSays() {
        val l0 = Link("tv", "Shield", playing(180.0), at = 10_000L, seenMs = 10_000L)
        val l = LinkClock.next(l0, playing(0.5, id = 8), seenMs = 12_000L, arrivalMs = 12_000L, offsetMs = 0L)
        assertEquals(500L, bar(l, 12_000L))
    }

    @Test
    fun aFetchedCopyKeepsTheServersExtrasButNotItsStalePosition() {
        val l0 = Link("tv", "Shield", playing(60.0), at = 10_000L, seenMs = 10_000L)
        val stamped = JsonObject(playing(60.0) + ("artists" to JsonPrimitive("stamped")))
        val l = LinkClock.next(l0, stamped, seenMs = 10_000L, arrivalMs = 11_000L, offsetMs = 0L)
        assertEquals(JsonPrimitive("stamped"), l.state["artists"])
        assertEquals(61_000L, bar(l, 11_000L))
    }

    @Test
    fun theOffsetIsTheFastestRecentSample() {
        val c = ClockOffset(keep = 3)
        assertNull(c.offsetMs)
        c.add(arrivalMs = 1_300L, seenMs = 1_000L)
        c.add(arrivalMs = 2_080L, seenMs = 2_000L)
        c.add(arrivalMs = 3_900L, seenMs = 3_000L)
        assertEquals(80L, c.offsetMs)
        // the fast sample ages out
        c.add(arrivalMs = 4_200L, seenMs = 4_000L)
        c.add(arrivalMs = 5_200L, seenMs = 5_000L)
        assertEquals(200L, c.offsetMs)
    }
}
