package app.encore.android.playback

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Putting the queue back after the music stopped without a pause ([ResumeRules]). The owner (Oct 28): "it randomly
 * pauses the song ... it just stops the song and restarts it, but it doesn't continue playback".
 */
class ResumeRulesTest {
    private val minute = 60_000L

    /** A process that died while playing a moment ago plays on; one that was paused or died long ago does not. */
    @Test
    fun aRecentInterruptionPlaysOn() {
        assertTrue(ResumeRules.playOn(interruptedMs = 5_000L, sinceRecoveredMs = Long.MAX_VALUE, afterUpdate = false))
        assertFalse(ResumeRules.playOn(interruptedMs = null, sinceRecoveredMs = Long.MAX_VALUE, afterUpdate = false))
        assertFalse(ResumeRules.playOn(interruptedMs = 3 * minute, sinceRecoveredMs = Long.MAX_VALUE, afterUpdate = false))
        assertFalse(ResumeRules.playOn(interruptedMs = null, sinceRecoveredMs = Long.MAX_VALUE, afterUpdate = true))
    }

    /** A second crash right after a recovery stays paused (no crash loop), but two update installs in a row play on. */
    @Test
    fun theCrashLoopGuardSparesUpdateInstalls() {
        assertFalse(ResumeRules.playOn(interruptedMs = 5_000L, sinceRecoveredMs = 30_000L, afterUpdate = false))
        assertTrue(ResumeRules.playOn(interruptedMs = 5_000L, sinceRecoveredMs = 30_000L, afterUpdate = true))
    }

    /** An update install starts the music again only when the old version was playing just before it. */
    @Test
    fun anUpdateResumesOnlyWhatWasPlaying() {
        assertTrue(ResumeRules.resumeAfterUpdate(playing = true, sinceSavedMs = 4_000L, hasQueue = true))
        assertFalse(ResumeRules.resumeAfterUpdate(playing = false, sinceSavedMs = 4_000L, hasQueue = true))
        assertFalse(ResumeRules.resumeAfterUpdate(playing = true, sinceSavedMs = 4_000L, hasQueue = false))
        assertFalse(ResumeRules.resumeAfterUpdate(playing = true, sinceSavedMs = 10 * minute, hasQueue = true))
        assertFalse(ResumeRules.resumeAfterUpdate(playing = true, sinceSavedMs = Long.MAX_VALUE, hasQueue = true))
    }

    /** The update mark counts for the start right after the install only. */
    @Test
    fun theUpdateMarkIsShortLived() {
        assertTrue(ResumeRules.updateKillCounts(2_000L))
        assertFalse(ResumeRules.updateKillCounts(2 * minute))
        // never marked (0 = epoch): decades ago
        assertFalse(ResumeRules.updateKillCounts(System.currentTimeMillis()))
        assertFalse(ResumeRules.updateKillCounts(-1L))
    }

    /** A whole file seeks itself; a server-made stream is asked for at the spot and the player starts at 0. */
    @Test
    fun aServerMadeStreamStartsAtItsSpotNotAtZero() {
        assertEquals(ResumeRules.Start(83_000L, 0.0), ResumeRules.startAt(83_000L, serverStream = false))
        assertEquals(ResumeRules.Start(0L, 83.0), ResumeRules.startAt(83_000L, serverStream = true))
        // the very start: nothing to reopen at
        assertEquals(ResumeRules.Start(200L, 0.0), ResumeRules.startAt(200L, serverStream = true))
        assertEquals(ResumeRules.Start(0L, 0.0), ResumeRules.startAt(-5L, serverStream = true))
    }
}
