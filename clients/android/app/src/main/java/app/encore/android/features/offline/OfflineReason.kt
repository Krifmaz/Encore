package app.encore.android.features.offline

import java.io.InterruptedIOException
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.net.UnknownHostException
import javax.net.ssl.SSLException

/**
 * Why offline mode thinks the server can't be reached, in words the offline sheet (OfflineUi.kt `OfflineSheet`) shows
 * under the banner's full message. The health check ([OfflineMode]) knows more than "no answer": no network on the
 * phone, an address that doesn't resolve, a refused connection, a timeout, a broken HTTPS setup, or a proxy in front
 * of the server saying the server behind it is down (HTTP 502–504). Saying which one turns "it won't connect" into
 * something the person can fix: e.g. a reverse proxy still pointing at a container that `docker compose up -d` just
 * replaced answers 502 while `docker ps` says the new container is healthy.
 *
 * Plain Kotlin (only `java.net` and `javax.net.ssl` exception types), so it is checked by JUnit tests on its own.
 */
object OfflineReason {
    /** The phone itself has no network (no Wi-Fi, no mobile data, flight mode). */
    const val NO_NETWORK = "This phone has no network connection."

    /** Nothing is known yet (offline mode started before a check finished). */
    const val UNKNOWN = "The server didn't answer."

    /**
     * The reason for one failed health check.
     *
     * @param phoneOnline whether the phone has a network at all (the default-network callback of [OfflineMode.start]).
     * @param code the HTTP status the server (or a proxy in front of it) answered, when it answered at all.
     * @param error the exception the call failed with, when it got no HTTP answer.
     * @param seconds how long the check waited before giving up, for the timeout wording.
     * @return one or two short sentences, never empty.
     */
    fun of(phoneOnline: Boolean, code: Int?, error: Throwable?, seconds: Long): String {
        if (!phoneOnline) return NO_NETWORK
        // an HTTP answer wins over any exception: the proxy answered, the server behind it didn't
        if (code != null) {
            return when (code) {
                502, 503, 504 ->
                    "The proxy in front of the server answered HTTP $code: it can't reach Encore behind it. " +
                        "If the container was just recreated, restart the proxy or point it at the container by name."
                else -> "The server answered HTTP $code to the health check."
            }
        }
        // every case is an IOException: the specific types are matched before the generic message
        return when (error) {
            is UnknownHostException -> "The server's address couldn't be found (DNS). Check the address in Settings and the phone's network."
            // a socket timeout, or OkHttp's whole-call timeout (a plain InterruptedIOException "timeout")
            is InterruptedIOException -> "The server didn't answer within $seconds s."
            is ConnectException, is NoRouteToHostException ->
                "Nothing answered at the server's address (connection refused or no route). Is the server, or its proxy, running and reachable from this network?"
            is SSLException -> "The secure (HTTPS) connection failed: check the server's certificate."
            null -> UNKNOWN
            else -> error.message?.takeIf { it.isNotBlank() }?.let { "The server couldn't be reached: $it" } ?: UNKNOWN
        }
    }
}
