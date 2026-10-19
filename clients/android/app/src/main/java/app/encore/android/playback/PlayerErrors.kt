package app.encore.android.playback

import androidx.media3.common.PlaybackException

/**
 * What the playback service does when the player stops on an error (onPlayerError), from the error and the
 * variant the song was playing. In the background nobody sees the error and presses play, so the music used to
 * stop for good after any error but the three fallbacks; now a song that keeps failing is skipped and the music
 * stops only when nothing else can help. Pure, so it is unit-tested (PlayerErrorsTest).
 */
object PlayerErrors {
    enum class Action {
        /** the downloaded copy would not play: the song streams from the server, from the same spot */
        STREAM,
        /** the server's Cadenza is off, busy or failed: the same spot without processing, none for the rest of the session */
        NO_DSP,
        /** the same song again at the same spot, in the same variant */
        REOPEN,
        /** the phone can't decode the original: the server converts it to FLAC (once per song) */
        TRANSCODE,
        /** the song won't play: on to the next one */
        SKIP,
        /** the music stops: the connection is gone (PatientLoads already waited for it) or the account can't play */
        STOP,
    }

    /** a song is opened again at most this often for a stalled output or a server error, then skipped */
    const val MAX_REOPENS = 10

    /**
     * @param code the error's PlaybackException.errorCode
     * @param status the server's HTTP status when it answered with an error, else 0
     * @param bufferedNothing Media3 stopped a player that had buffered nothing for 10 minutes (a lost connection
     *   that PatientLoads was still waiting for), not one whose output stalled
     * @param transcodeTried the server was already asked to convert this song
     * @param reopens how often this song was opened again after an error already
     */
    fun decide(code: Int, status: Int, bufferedNothing: Boolean, v: StreamVariant, transcodeTried: Boolean, reopens: Int): Action {
        if (v.local.isNotEmpty()) return Action.STREAM
        // the connection, not the song: PatientLoads already waited it out for minutes with the player buffering.
        // The music stops, and the server's Cadenza stays on for the session.
        val lost = code == PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED ||
            code == PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT ||
            status == 502 || status == 504 ||
            (code == PlaybackException.ERROR_CODE_TIMEOUT && bufferedNothing)
        if (lost) return Action.STOP
        // a stalled output is not the server's processing failing
        if (v.dsp && code != PlaybackException.ERROR_CODE_TIMEOUT) return Action.NO_DSP
        // signed out or not allowed: it stays stopped (the app asks to sign in again)
        if (status == 401 || status == 403) return Action.STOP
        // the song is no longer on the server (a rescan, a merge), or shorter than it was
        if (status == 404 || status == 410 || code == PlaybackException.ERROR_CODE_IO_READ_POSITION_OUT_OF_RANGE) return Action.SKIP
        // the output stalled (Media3 stops a player that plays without moving for 10 s) or the server answered
        // with an error that may pass: the same spot again, a few times
        val passing = code == PlaybackException.ERROR_CODE_TIMEOUT || code == PlaybackException.ERROR_CODE_IO_UNSPECIFIED ||
            status == 408 || status == 429 || status >= 500
        if (passing) return if (reopens < MAX_REOPENS) Action.REOPEN else Action.SKIP
        // the phone couldn't decode the original (e.g. an odd M4A): decoder, container and audio output errors
        // only, never a connection or a server answer (2000-2999)
        if (code !in 2000..2999 && !v.transcode && !v.lossy && !transcodeTried) return Action.TRANSCODE
        return Action.SKIP
    }
}
