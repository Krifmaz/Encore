package app.encore.android.playback

import androidx.media3.common.C
import androidx.media3.datasource.HttpDataSource
import androidx.media3.exoplayer.upstream.DefaultLoadErrorHandlingPolicy
import androidx.media3.exoplayer.upstream.LoadErrorHandlingPolicy

/**
 * A dropped connection (a dead zone, Wi-Fi to mobile data, the server restarting) is waited out while the player
 * shows buffering, instead of failing after 3 tries. While it buffers with play on, the player keeps its wake and
 * Wi-Fi locks and the service stays in the foreground, so the music goes on by itself when the server answers
 * again, even with the screen off (a timer would not run while the phone sleeps). An answer from the server (404,
 * 503 "Cadenza busy", ...) still fails after 3 tries, so onPlayerError's fallbacks work as before. Gives up after
 * 60 tries (ExoPlayer waits up to 5 s between them: about 5 minutes, longer when each try hangs until its timeout);
 * Media3 also stops a player that has buffered nothing for 10 minutes. The main player only: the transitions
 * player keeps the defaults.
 */
class PatientLoads : DefaultLoadErrorHandlingPolicy() {
    // the error reaches the player only when the policy below gives up (C.TIME_UNSET)
    override fun getMinimumLoadableRetryCount(dataType: Int): Int = Int.MAX_VALUE

    override fun getRetryDelayMsFor(loadErrorInfo: LoadErrorHandlingPolicy.LoadErrorInfo): Long {
        val delay = super.getRetryDelayMsFor(loadErrorInfo)
        if (delay == C.TIME_UNSET) return delay
        val status = (loadErrorInfo.exception as? HttpDataSource.InvalidResponseCodeException)?.responseCode
        return if (giveUp(loadErrorInfo.errorCount, status)) C.TIME_UNSET else delay
    }

    companion object {
        /** tries for a connection that is gone (no answer, or the proxy in front of a server that is restarting) */
        const val CONNECTION_TRIES = 60

        /**
         * After `errorCount` failed tries, with the server's HTTP status (null: no answer at all), is the load given
         * up? 502 / 504 come from a proxy whose server is restarting, so they wait like a lost connection.
         */
        fun giveUp(errorCount: Int, status: Int?): Boolean {
            val answered = status != null && status != 502 && status != 504
            val limit = if (answered) DefaultLoadErrorHandlingPolicy.DEFAULT_MIN_LOADABLE_RETRY_COUNT else CONNECTION_TRIES
            return errorCount > limit
        }
    }
}
