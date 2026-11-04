package app.encore.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

/** [PlaybackClock.positionAt]: how the progress bars move between the player's ticks. */
class PlaybackClockTest {
    private val ms = 1_000_000L

    @Test
    fun pausedStaysPut() {
        val c = PlaybackClock(positionMs = 30_000, atNanos = 0, playing = false, speed = 1f)
        assertEquals(30_000, c.positionAt(5_000 * ms))
    }

    @Test
    fun playingMovesOnWithTime() {
        val c = PlaybackClock(positionMs = 30_000, atNanos = 100 * ms, playing = true, speed = 1f)
        assertEquals(30_000, c.positionAt(100 * ms))
        assertEquals(30_250, c.positionAt(350 * ms))
    }

    @Test
    fun followsThePlaybackSpeed() {
        val c = PlaybackClock(positionMs = 10_000, atNanos = 0, playing = true, speed = 1.5f)
        assertEquals(10_600, c.positionAt(400 * ms))
    }

    @Test
    fun aFrameOlderThanTheReadDoesNotGoBack() {
        val c = PlaybackClock(positionMs = 10_000, atNanos = 500 * ms, playing = true, speed = 1f)
        assertEquals(10_000, c.positionAt(480 * ms))
    }
}
