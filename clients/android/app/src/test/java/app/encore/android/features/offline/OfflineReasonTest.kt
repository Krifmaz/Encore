package app.encore.android.features.offline

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException
import java.io.InterruptedIOException
import java.net.ConnectException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import javax.net.ssl.SSLHandshakeException

/** The words of [OfflineReason]: what the offline sheet says about why the server can't be reached. */
class OfflineReasonTest {
    /** Shorthand: a check from a phone with a network, waiting 8 s. */
    private fun of(code: Int? = null, error: Throwable? = null) = OfflineReason.of(phoneOnline = true, code = code, error = error, seconds = 8)

    @Test
    fun noNetworkOnThePhoneWinsOverEverything() {
        assertEquals(OfflineReason.NO_NETWORK, OfflineReason.of(false, null, UnknownHostException("x"), 8))
        assertEquals(OfflineReason.NO_NETWORK, OfflineReason.of(false, 502, null, 8))
    }

    @Test
    fun aProxyAnswering502To504NamesTheProxy() {
        for (code in 502..504) {
            val r = of(code = code)
            assertTrue(r, r.contains("HTTP $code") && r.contains("proxy"))
        }
    }

    @Test
    fun eachFailureHasItsOwnWords() {
        assertTrue(of(error = UnknownHostException("music.example.com")).contains("DNS"))
        // both kinds of timeout: the socket's, and OkHttp's whole-call one
        assertEquals("The server didn't answer within 8 s.", of(error = SocketTimeoutException()))
        assertEquals("The server didn't answer within 8 s.", of(error = InterruptedIOException("timeout")))
        assertTrue(of(error = ConnectException("refused")).contains("connection refused"))
        assertTrue(of(error = SSLHandshakeException("bad cert")).contains("HTTPS"))
    }

    @Test
    fun anythingElseSaysItsMessageOrTheDefault() {
        assertEquals("The server couldn't be reached: reset", of(error = IOException("reset")))
        assertEquals(OfflineReason.UNKNOWN, of(error = IOException()))
        assertEquals(OfflineReason.UNKNOWN, of())
    }
}
