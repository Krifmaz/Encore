package app.encore.android.data.api

import android.content.Context
import android.os.Build
import app.encore.android.BuildConfig
import app.encore.android.data.session.AuthKind
import app.encore.android.data.session.SessionStore
import okhttp3.Cache
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import okhttp3.Response
import java.io.File
import java.util.concurrent.TimeUnit

/** `EncoreAndroid/<version> (Android <release>; <model>)` – the server's Sessions page knows this name. */
val USER_AGENT: String = "EncoreAndroid/${BuildConfig.VERSION_NAME} (Android ${Build.VERSION.RELEASE}; ${Build.MODEL})"

/**
 * The one OkHttpClient the app uses: API calls, cover images (Coil) and audio streams (Media3) all go through
 * it, so they share the auth header, the User-Agent, the connection pool and the cache.
 */
fun buildHttpClient(context: Context, store: SessionStore): OkHttpClient =
    OkHttpClient.Builder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(30, TimeUnit.SECONDS)
        .followRedirects(true)
        .followSslRedirects(true)
        .cache(Cache(File(context.cacheDir, "http"), 50L * 1024L * 1024L))
        .addInterceptor(AuthInterceptor(store))
        // after the session is added for the home address: to the local network address while it answers
        .addInterceptor(RouteInterceptor())
        // every hop of a redirect: the session stays on the server's own host
        .addNetworkInterceptor(SessionOffHostInterceptor(store))
        .build()

/**
 * Adds the session to requests for the signed-in server (never to other hosts, e.g. a playlist cover on a CDN):
 * `Authorization: Bearer` for app tokens; the `encore_session` cookie plus the CSRF header on writes otherwise.
 * A 401 on an authenticated call signs the app out.
 */
class AuthInterceptor(private val store: SessionStore) : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        val req = chain.request()
        val b = req.newBuilder().header("User-Agent", USER_AGENT)
        val s = store.current
        val path = req.url.encodedPath
        val isAuthCall = path.startsWith("/api/auth/login") || path.startsWith("/api/auth/info") || path.startsWith("/api/health")
        val su = s?.server?.toHttpUrlOrNull()
        val authed = su != null && !isAuthCall && SessionRules.sameHost(req.url, su) && path.startsWith(su.encodedPath.trimEnd('/'))
        if (s != null && authed) {
            if (s.kind == AuthKind.BEARER) {
                b.header("Authorization", "Bearer " + s.token)
            } else {
                b.header("Cookie", "encore_session=" + s.token)
            }
        }
        // the server's CSRF check for cookie sessions; harmless with a bearer token
        if (req.method != "GET" && req.method != "HEAD") b.header("X-Requested-With", "encore")
        val res = chain.proceed(b.build())
        // only the server's own 401 signs out (at its home or its route address, ServerRoute), not one from the host a
        // redirect went on to
        val own = su != null && (SessionRules.sameHost(res.request.url, su) || ServerRoute.isRouteHost(res.request.url))
        if (s != null && authed && res.code == 401 && own) store.onUnauthorized(s.token)
        return res
    }
}

/**
 * The session never leaves the server's host. AuthInterceptor adds it once, to the request as the app made it;
 * OkHttp then follows redirects with that request's headers, and drops `Authorization` on another host but
 * keeps `Cookie`. So on every network hop (a network interceptor sees each redirect) a request for another host
 * loses Encore's own `encore_session` cookie and its own bearer token, and nothing else: other cookies and the
 * headers of calls made for other hosts (the GitHub update check's) stay as they are.
 */
class SessionOffHostInterceptor(private val store: SessionStore) : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        val req = chain.request()
        val s = store.current ?: return chain.proceed(req)
        val server = s.server.toHttpUrlOrNull()
        // the server's own host, or the address ServerRoute moved the request to (it answered as this server)
        if (server != null && (SessionRules.sameHost(req.url, server) || ServerRoute.isRouteHost(req.url))) return chain.proceed(req)
        val cookie = req.header("Cookie")
        val auth = req.header("Authorization")
        val keptCookie = cookie?.let { SessionRules.cookieWithout(it, s.token) }
        val dropAuth = auth != null && SessionRules.isOwnBearer(auth, s.token)
        if (cookie == keptCookie && !dropAuth) return chain.proceed(req)
        val b = req.newBuilder()
        if (keptCookie == null) b.removeHeader("Cookie") else b.header("Cookie", keptCookie)
        if (dropAuth) b.removeHeader("Authorization")
        return chain.proceed(b.build())
    }
}

/** What goes with the session and what never does (unit-tested in SessionRulesTest). */
object SessionRules {
    const val COOKIE = "encore_session"

    /**
     * The same server: scheme, host and port (OkHttp's HttpUrl lower-cases the host and fills in default ports,
     * so a stored "HTTPS://Music.Example.org:443" matches "https://music.example.org").
     */
    fun sameHost(url: HttpUrl, server: HttpUrl): Boolean =
        url.scheme == server.scheme && url.host == server.host && url.port == server.port

    /**
     * A `Cookie` header without Encore's own `encore_session=<token>` pair, the other cookies as they were; null
     * when nothing is left. An `encore_session` with another value is not Encore's session and stays.
     */
    fun cookieWithout(cookie: String, token: String): String? {
        val own = "$COOKIE=$token"
        val rest = cookie.split(';').map { it.trim() }.filter { it.isNotEmpty() && it != own }
        return if (rest.isEmpty()) null else rest.joinToString("; ")
    }

    /** `Authorization: Bearer <token>` with the app's own token (another value is someone else's header). */
    fun isOwnBearer(authorization: String, token: String): Boolean {
        val value = authorization.trim()
        val scheme = "Bearer "
        return token.isNotEmpty() && value.startsWith(scheme, ignoreCase = true) && value.substring(scheme.length).trim() == token
    }
}
