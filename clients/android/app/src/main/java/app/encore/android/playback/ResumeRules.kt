package app.encore.android.playback

/**
 * The decisions behind putting a queue back after the playback stopped without anyone pressing pause: the process
 * killed (an update install, a crash, Android short of memory) and a queue started part-way into a song. Kept apart
 * from [Resume], [ResumeAfterUpdateReceiver] and [StreamResolver] (which need Android) so the unit tests can check
 * them (ResumeRulesTest).
 *
 * Bug report (Oct 28): "it randomly pauses the song ... now it just stops the song and restarts it, but it doesn't
 * continue playback". Each install of an Encore update stops the running app by Android's design; nothing started
 * the music again until the app was opened, and a song the server converts (Opus, a lower quality on mobile data,
 * the server's Cadenza) came back at 0:00 instead of its spot.
 */
object ResumeRules {
    /** Crash recovery: playing this recently when the process died → it plays on by itself (ms). */
    const val RECOVER_WINDOW_MS = 2 * 60 * 1000L

    /**
     * An update install found the app playing this recently before it was stopped (ms): the start that follows is
     * that install's, and the crash-loop guard does not apply to it.
     */
    const val UPDATE_WINDOW_MS = 60 * 1000L

    /** A spot closer to the song's start than this (ms) starts the song from the top: nothing worth reopening at. */
    const val MIN_SPOT_MS = 500L

    /**
     * Whether a queue put back at start plays on by itself.
     *
     * @param interruptedMs how long ago the process last played before it died (null: it did not die while playing)
     * @param sinceRecoveredMs how long ago a start last played on by itself (a song that crashes the app must not loop)
     * @param afterUpdate the process was stopped to install an update ([updateKillCounts]): not a crash, so a second
     *   install within the window still plays on
     */
    fun playOn(interruptedMs: Long?, sinceRecoveredMs: Long, afterUpdate: Boolean): Boolean {
        if (interruptedMs == null || interruptedMs >= RECOVER_WINDOW_MS) return false
        return afterUpdate || sinceRecoveredMs > RECOVER_WINDOW_MS
    }

    /**
     * After an update install (Android's MY_PACKAGE_REPLACED): whether the old version was playing when it was
     * stopped, so the playback service is started to play on.
     *
     * @param playing the "playing" flag the old process left behind (it is cleared by a pause or a clean stop)
     * @param sinceSavedMs how long ago the old process last played (its last save)
     * @param hasQueue a queue is saved to put back
     */
    fun resumeAfterUpdate(playing: Boolean, sinceSavedMs: Long, hasQueue: Boolean): Boolean =
        playing && hasQueue && sinceSavedMs in 0 until RECOVER_WINDOW_MS

    /** Whether the update mark written [sinceMarkMs] ago ([ResumeStore.updateKillAt]) still describes this start. */
    fun updateKillCounts(sinceMarkMs: Long): Boolean = sinceMarkMs in 0 until UPDATE_WINDOW_MS

    /**
     * Where a queue that starts [positionMs] into its song begins.
     *
     * @property playerMs the start position handed to the player
     * @property offsetSec where the server is asked to start its stream (`t=`), 0 for none
     */
    data class Start(val playerMs: Long, val offsetSec: Double)

    /**
     * A whole file (the original, a downloaded copy, a finished render) seeks itself, so the player starts at the
     * spot. A stream the server makes as it goes has no Range: the player can't seek into it and started it from
     * 0:00, so the server is asked for it at the spot (`t=`) and the player starts at 0, as a seek in one does.
     */
    fun startAt(positionMs: Long, serverStream: Boolean): Start {
        val ms = positionMs.coerceAtLeast(0L)
        if (!serverStream || ms < MIN_SPOT_MS) return Start(ms, 0.0)
        return Start(0L, ms / 1000.0)
    }
}
