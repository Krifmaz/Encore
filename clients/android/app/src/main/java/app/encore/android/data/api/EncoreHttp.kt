package app.encore.android.data.api

import android.content.Context
import android.os.Build
import app.encore.android.BuildConfig
import app.encore.android.data.session.AuthKind
import app.encore.android.data.session.SessionStore
import okhttp3.Cache
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
        // compare by parts: HttpUrl lower-cases the host and drops default ports, a stored server may not
        val su = s?.server?.toHttpUrlOrNull()
        val authed = su != null && !isAuthCall &&
            req.url.scheme == su.scheme && req.url.host == su.host && req.url.port == su.port &&
            path.startsWith(su.encodedPath.trimEnd('/'))
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
        if (s != null && authed && res.code == 401) store.onUnauthorized(s.token)
        return res
    }
}
