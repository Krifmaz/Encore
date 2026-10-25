package app.encore.android.data.api

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import app.encore.android.CrashLog
import app.encore.android.core.EncoreJson
import app.encore.android.core.str
import app.encore.android.graph
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import java.io.IOException
import java.util.concurrent.TimeUnit

/** Which of the server's addresses requests go to. */
enum class Via {
    /** The address the app was signed in to (nothing else known, or nothing else answers). */
    HOME,
    /** The server's Local network address (Settings → General on the server). */
    LOCAL,
    /** The server's Public URL. */
    PUBLIC,
}

/**
 * What the app knows about the server's addresses.
 *
 * @property home the address the app was signed in to (the session's `server`, no trailing `/`): every URL the app
 *   builds starts with it, so the kept answers, the covers' cache and the offline songs stay under one name
 * @property local the server's Local network address ("" when not set)
 * @property public the server's Public URL ("" when not set)
 * @property serverId the id `/api/auth/info` must show at an address before the session goes there
 * @property via the address in use
 */
data class Route(
    val home: String = "",
    val local: String = "",
    val public: String = "",
    val serverId: String = "",
    val via: Via = Via.HOME,
) {
    /** The address requests go to now. */
    val target: String
        get() = when {
            via == Via.LOCAL && local.isNotEmpty() -> local
            via == Via.PUBLIC && public.isNotEmpty() -> public
            else -> home
        }

    /** The indicator's words (Settings): which address and what it is. */
    val label: String
        get() = when {
            RouteRules.same(target, local) -> "Local network ($target)"
            RouteRules.same(target, public) -> "Public address ($target)"
            else -> target
        }
}

/** The pure part of the switch (unit-tested in RouteRulesTest): rewriting URLs, reading the server's answer. */
object RouteRules {
    /** The same address, ignoring case and a trailing `/` ("" never matches). */
    fun same(a: String, b: String): Boolean =
        a.isNotEmpty() && a.trimEnd('/').equals(b.trimEnd('/'), ignoreCase = true)

    /** An address from the server's answer: trimmed, no trailing `/`, "" unless it is http(s). */
    fun clean(raw: String): String {
        val s = raw.trim().trimEnd('/')
        return if (s.startsWith("http://") || s.startsWith("https://")) s else ""
    }

    /**
     * [url] moved from the route's home address to its target: the same path, query and fragment on the target's
     * scheme, host, port and path prefix. Null when nothing changes (the target is home, or [url] is on another
     * host, e.g. a cover on a CDN).
     */
    fun rewrite(route: Route, url: HttpUrl): HttpUrl? {
        if (route.via == Via.HOME) return null
        val home = route.home.toHttpUrlOrNull() ?: return null
        val target = route.target.toHttpUrlOrNull() ?: return null
        if (!SessionRules.sameHost(url, home) || SessionRules.sameHost(target, home)) return null
        // a server behind a path ("https://x.org/encore") keeps the rest of the path after it
        val homePath = home.encodedPath.trimEnd('/')
        val path = url.encodedPath
        if (homePath.isNotEmpty() && path != homePath && !path.startsWith("$homePath/")) return null
        val rest = path.removePrefix(homePath)
        val b = url.newBuilder().scheme(target.scheme).host(target.host).port(target.port)
        b.encodedPath(target.encodedPath.trimEnd('/') + rest.ifEmpty { "/" })
        return b.build()
    }

    /**
     * The Settings indicator: the host in use without its scheme, and "local network" or "public address" when the
     * app moved to it; [server] (the signed-in address) alone before the route knows it.
     */
    fun summary(route: Route, server: String): String {
        if (!same(route.home, server.trimEnd('/'))) return server.substringAfter("://")
        val host = route.target.substringAfter("://")
        return when {
            same(route.target, route.local) -> "$host (local network)"
            route.via == Via.PUBLIC -> "$host (public address)"
            else -> host
        }
    }

    /** The route with the addresses of `/api/auth/addresses` (`{public, local, server_id}`). */
    fun withAnswer(route: Route, answer: JsonObject): Route =
        route.copy(local = clean(answer.str("local")), public = clean(answer.str("public")), serverId = answer.str("server_id"))

    /**
     * Which address to use, given whether the local and the public address answered as this server. The local one
     * is asked even when it is home: away from home it doesn't answer, and then the public one is the way in.
     */
    fun pick(route: Route, localAnswers: () -> Boolean, publicAnswers: () -> Boolean): Via = when {
        route.local.isNotEmpty() && localAnswers() -> if (same(route.local, route.home)) Via.HOME else Via.LOCAL
        route.public.isNotEmpty() && !same(route.public, route.home) && publicAnswers() -> Via.PUBLIC
        else -> Via.HOME
    }
}

/**
 * The address the app reaches the server at: the Local network address while the phone (or TV) is on the home
 * network, the public one otherwise.
 *
 * The server publishes both at `GET /api/auth/addresses` (signed-in users only) with the `server_id` its
 * `/api/auth/info` shows. Every URL in the app stays on the address it was signed in to; [RouteInterceptor] moves a
 * request to the address in use just before it goes out, so API calls, covers (Coil) and songs (Media3) all follow.
 * [choose] decides: when the app starts, whenever the default network changes, and when a call to the address in
 * use can't connect ([fallBack], which also retries the call on the home address). An address is used only after
 * its `/api/auth/info` answered with this server's id, so the session never goes to another machine that has the
 * same home-network address. The addresses are kept on the phone, so one signed in at home finds the public one
 * while away even when it can't ask the server.
 */
object ServerRoute {
    /** How long a probe waits: a home network answers in milliseconds, a network without the address times out. */
    private const val PROBE_SECONDS = 3L
    /** The network settles before it is asked (Wi-Fi joins, then gets its address). */
    private const val SETTLE_MS = 1500L
    private const val PREFS = "encore_route"

    private val _route = MutableStateFlow(Route())
    /** The route now, for the Settings indicator. */
    val route: StateFlow<Route> = _route.asStateFlow()

    /** Synchronous snapshot for the interceptors. */
    fun current(): Route = _route.value

    private val choosing = Mutex()
    @Volatile private var app: Context? = null
    private var pending: Job? = null

    /** A client of its own for the probes: no session, no cache, no route (it asks the address as given). */
    private val probeClient: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .connectTimeout(PROBE_SECONDS, TimeUnit.SECONDS)
            .callTimeout(PROBE_SECONDS, TimeUnit.SECONDS)
            .followRedirects(false)
            .build()
    }

    /**
     * From the main screen (signed in) and the playback service: watch the default network and choose now. The
     * watch starts once per process; later calls only choose again.
     */
    @Synchronized
    fun start(context: Context) {
        val a = context.applicationContext
        if (app != null) { chooseSoon(0L); return }
        app = a
        try {
            a.getSystemService(ConnectivityManager::class.java)?.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) = chooseSoon(SETTLE_MS)
                override fun onLost(network: Network) = chooseSoon(SETTLE_MS)
            })
        } catch (e: Exception) {
            CrashLog.log("route: no network callback: ${e.message}")
        }
        chooseSoon(0L)
    }

    /** [choose] after [afterMs], replacing one still waiting (a network change brings several callbacks). */
    @Synchronized
    private fun chooseSoon(afterMs: Long) {
        val a = app ?: return
        pending?.cancel()
        pending = a.graph.scope.launch {
            if (afterMs > 0) delay(afterMs)
            choose(a)
        }
    }

    /** Asks the server for its addresses and picks the one to use; returns the route chosen. */
    suspend fun choose(a: Context): Route = choosing.withLock {
        val graph = a.graph
        graph.store.awaitLoaded()
        val home = graph.store.current?.server?.trimEnd('/').orEmpty()
        if (home.isEmpty()) {
            _route.value = Route()
            return@withLock _route.value
        }
        var r = _route.value
        // a server not asked in this run yet: what it said last time
        if (r.home != home) r = load(a, home)
        val answer = withContext(Dispatchers.IO) {
            try { graph.api.getObject("/api/auth/addresses") } catch (e: ApiException) {
                // an old server without the endpoint: nothing to switch to
                if (e.code == 404) JsonObject(emptyMap()) else null
            } catch (_: IOException) { null }
        }
        if (answer != null) {
            val next = RouteRules.withAnswer(r, answer)
            if (next != r) save(a, next)
            r = next
        }
        val probed = r
        val via = withContext(Dispatchers.IO) {
            RouteRules.pick(probed, { probe(probed.local, probed.serverId) }, { probe(probed.public, probed.serverId) })
        }
        r = r.copy(via = via)
        // signed out or another server meanwhile: keep the newer state
        if (graph.store.current?.server?.trimEnd('/') == home) {
            if (_route.value.via != via) CrashLog.log("route: ${r.label}")
            _route.value = r
        }
        r
    }

    /** Whether [base] answers `/api/auth/info` as the server [serverId] (an old server without an id never does). */
    private fun probe(base: String, serverId: String): Boolean {
        if (base.isEmpty() || serverId.isEmpty()) return false
        return try {
            probeClient.newCall(Request.Builder().url("$base/api/auth/info").header("Accept", "application/json").build()).execute().use { res ->
                if (!res.isSuccessful) return false
                val o = EncoreJson.parseToJsonElement(res.body.string()) as? JsonObject
                o?.str("server_id") == serverId
            }
        } catch (_: Exception) {
            false
        }
    }

    /**
     * A call to the address in use [r] couldn't connect: back to the home address until [choose] finds a better one
     * (asked at once). True when the address changed, so the call is worth one more try.
     */
    fun fallBack(r: Route): Boolean {
        if (r.via == Via.HOME) return false
        // only when nothing else changed the route meanwhile
        if (!_route.compareAndSet(r, r.copy(via = Via.HOME))) return _route.value.via == Via.HOME
        CrashLog.log("route: ${r.target} can't be reached, back to ${r.home}")
        chooseSoon(SETTLE_MS)
        return true
    }

    /**
     * Whether [url] is on the address the route moved requests to: the session may go there (it answered as this
     * server). False while the home address is in use.
     */
    fun isRouteHost(url: HttpUrl): Boolean {
        val r = _route.value
        if (r.via == Via.HOME) return false
        val t = r.target.toHttpUrlOrNull() ?: return false
        return SessionRules.sameHost(url, t)
    }

    /** The addresses kept for [home] (via HOME until probed). */
    private fun load(a: Context, home: String): Route {
        val p = a.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (p.getString("home", "") != home) return Route(home = home)
        return Route(home = home, local = p.getString("local", "").orEmpty(), public = p.getString("public", "").orEmpty(), serverId = p.getString("server_id", "").orEmpty())
    }

    /** Keeps the addresses of [r] for the next start. */
    private fun save(a: Context, r: Route) {
        a.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString("home", r.home).putString("local", r.local).putString("public", r.public).putString("server_id", r.serverId)
            .apply()
    }
}

/**
 * Moves each request for the signed-in server to the address in use ([ServerRoute]). An application interceptor
 * after [AuthInterceptor], which has added the session for the home address already. When the address in use can't
 * be connected to, the request goes once more to the home address ([ServerRoute.fallBack]).
 */
class RouteInterceptor : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        val req = chain.request()
        val r = ServerRoute.current()
        val moved = RouteRules.rewrite(r, req.url) ?: return chain.proceed(req)
        return try {
            chain.proceed(req.newBuilder().url(moved).build())
        } catch (e: IOException) {
            if (chain.call().isCanceled() || !ServerRoute.fallBack(r)) throw e
            chain.proceed(req)
        }
    }
}
