package app.encore.android.playback

import androidx.media3.common.PlaybackException
import app.encore.android.playback.PlayerErrors.Action
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What the playback service does when the player stops on an error (PlayerErrors), and how long a lost connection
 * is waited out (PatientLoads). The owner: "the app will randomly close and stop playing the music", only during
 * background play, where nobody presses play again after an error.
 */
class PlayerErrorsTest {
    private val original = StreamVariant()
    private val lossy = StreamVariant(rung = 128)
    private val converted = StreamVariant(transcode = true)
    private val cadenza = StreamVariant(dsp = true, dspRate = 96000)
    private val copy = StreamVariant(local = "content://app.encore.client.offline/1")

    private fun decide(code: Int, v: StreamVariant = original, status: Int = 0, bufferedNothing: Boolean = false, transcodeTried: Boolean = false, reopens: Int = 0) =
        PlayerErrors.decide(code, status, bufferedNothing, v, transcodeTried, reopens)

    @Test
    fun aLostConnectionStopsAndIsNeverTakenForADecodeError() {
        // PatientLoads already waited it out: no FLAC conversion (it failed the same way), no reopen loop
        assertEquals(Action.STOP, decide(PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED))
        assertEquals(Action.STOP, decide(PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT))
        // a server restarting behind a proxy
        assertEquals(Action.STOP, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, status = 502))
        assertEquals(Action.STOP, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, status = 504))
        // Media3 gave up on a player that buffered nothing for 10 minutes
        assertEquals(Action.STOP, decide(PlaybackException.ERROR_CODE_TIMEOUT, bufferedNothing = true))
        // and the server's Cadenza stays on: the connection failed, not the processing
        assertEquals(Action.STOP, decide(PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED, cadenza))
    }

    @Test
    fun theFallbacksStay() {
        assertEquals(Action.STREAM, decide(PlaybackException.ERROR_CODE_DECODING_FAILED, copy))
        assertEquals(Action.STREAM, decide(PlaybackException.ERROR_CODE_IO_FILE_NOT_FOUND, copy))
        // 503 "Cadenza busy" or a failed render: the original instead
        assertEquals(Action.NO_DSP, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, cadenza, status = 503))
        assertEquals(Action.NO_DSP, decide(PlaybackException.ERROR_CODE_PARSING_CONTAINER_MALFORMED, cadenza))
        // an original the phone can't decode: converted once
        assertEquals(Action.TRANSCODE, decide(PlaybackException.ERROR_CODE_DECODING_FAILED))
        assertEquals(Action.TRANSCODE, decide(PlaybackException.ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED))
        assertEquals(Action.TRANSCODE, decide(PlaybackException.ERROR_CODE_AUDIO_TRACK_INIT_FAILED))
    }

    @Test
    fun aSongThatCannotPlayIsSkippedInsteadOfStoppingTheQueue() {
        // converted already, or a lossy stream: nothing more to try for this song
        assertEquals(Action.SKIP, decide(PlaybackException.ERROR_CODE_DECODING_FAILED, transcodeTried = true))
        assertEquals(Action.SKIP, decide(PlaybackException.ERROR_CODE_DECODING_FAILED, converted))
        assertEquals(Action.SKIP, decide(PlaybackException.ERROR_CODE_DECODING_FAILED, lossy))
        // gone from the server (a rescan, a merge) or shorter than it was
        assertEquals(Action.SKIP, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, status = 404))
        assertEquals(Action.SKIP, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, status = 410))
        assertEquals(Action.SKIP, decide(PlaybackException.ERROR_CODE_IO_READ_POSITION_OUT_OF_RANGE))
        // another answer the server won't change its mind about
        assertEquals(Action.SKIP, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, status = 400))
    }

    @Test
    fun aStalledOutputOrAServerHiccupOpensTheSongAgainAFewTimes() {
        // the output stopped moving for 10 s (Media3's stuck-playing check), even on the server's Cadenza
        assertEquals(Action.REOPEN, decide(PlaybackException.ERROR_CODE_TIMEOUT))
        assertEquals(Action.REOPEN, decide(PlaybackException.ERROR_CODE_TIMEOUT, cadenza))
        assertEquals(Action.REOPEN, decide(PlaybackException.ERROR_CODE_TIMEOUT, lossy))
        for (status in listOf(408, 429, 500, 503)) {
            assertEquals("HTTP $status", Action.REOPEN, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, status = status))
        }
        assertEquals(Action.REOPEN, decide(PlaybackException.ERROR_CODE_IO_UNSPECIFIED, converted))
        // then the song is skipped, never an endless loop
        assertEquals(Action.REOPEN, decide(PlaybackException.ERROR_CODE_TIMEOUT, reopens = PlayerErrors.MAX_REOPENS - 1))
        assertEquals(Action.SKIP, decide(PlaybackException.ERROR_CODE_TIMEOUT, reopens = PlayerErrors.MAX_REOPENS))
        assertEquals(Action.SKIP, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, status = 500, reopens = PlayerErrors.MAX_REOPENS))
    }

    @Test
    fun signedOutStops() {
        assertEquals(Action.STOP, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, status = 401))
        assertEquals(Action.STOP, decide(PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS, status = 403))
    }

    @Test
    fun aLostConnectionIsWaitedOutAndAnAnswerIsNot() {
        // no answer at all (a dead zone, Wi-Fi to mobile data): 60 tries
        assertFalse(PatientLoads.giveUp(errorCount = 4, status = null))
        assertFalse(PatientLoads.giveUp(errorCount = PatientLoads.CONNECTION_TRIES, status = null))
        assertTrue(PatientLoads.giveUp(errorCount = PatientLoads.CONNECTION_TRIES + 1, status = null))
        // the proxy in front of a server that is restarting
        assertFalse(PatientLoads.giveUp(errorCount = 30, status = 502))
        assertFalse(PatientLoads.giveUp(errorCount = 30, status = 504))
        // the server answered: 3 tries as before, so onPlayerError's fallbacks come as quickly as before
        assertFalse(PatientLoads.giveUp(errorCount = 3, status = 503))
        assertTrue(PatientLoads.giveUp(errorCount = 4, status = 503))
        assertTrue(PatientLoads.giveUp(errorCount = 4, status = 404))
    }
}
