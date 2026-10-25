package app.encore.android.features.offline

import app.encore.android.data.api.ServerRoute
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import app.encore.android.AppGraph
import app.encore.android.CrashLog
import app.encore.android.data.api.ApiException
import app.encore.android.graph
import app.encore.android.ui.components.Toasts
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import okhttp3.Request
import java.io.IOException
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Offline mode (the web's offline/mode.rs, for the phone): the signed-in server can't be reached. The app then shows
 * the offline banner and stays usable: every page shows what the server last answered for it (the API answers this
 * phone kept, data/api/ApiCache.kt), so Home, Library, Playlists, the pages opened before and Settings still open, the
 * Library tab opens on Downloaded, and the songs saved on this phone play from their files. The server is asked again
 * every 30 s, when the network changes, on Retry, and when a call fails to reach it; once it answers, the plays and
 * likes made meanwhile are sent ("Back online – 3 plays sent") and the open page loads again from the server.
 *
 * Also the outbox's front door for the rest of the app: [queuePlay] and [queueLike] keep what the server couldn't
 * take, [flush] sends it under the account that made it, and [syncSoon] sends it after the next play or like the
 * server took (a short gap while online, as the web's `sync_outbox`).
 */
object OfflineMode {
    /** The full message, shown in the offline sheet (OfflineUi.kt `OfflineSheet`) the banner opens. */
    const val BANNER = "Offline – showing what this phone saved. Downloaded songs play."
    /** The banner's first line: short enough for one line on the narrowest phone. */
    const val BANNER_TITLE = "Offline"
    /** The banner's second line (one line, ellipsized): what still works, and that the banner opens the details. */
    const val BANNER_LINE = "Downloaded songs play · tap for details"
    /** The toast after a Retry the server still didn't answer. */
    const val STILL_OFFLINE = "The server still can't be reached"

    /** How often offline mode asks the server again. */
    private const val RETRY_MS = 30_000L
    /** How long the server gets to answer a check. */
    private const val WAIT_SECONDS = 8L
    /** A failed call asks for a check at most this often ([noticeUnreachable]). */
    private const val NOTICE_GAP_MS = 10_000L

    private val _active = MutableStateFlow(false)
    /** The app is in offline mode. */
    val active: StateFlow<Boolean> = _active.asStateFlow()

    private val _trying = MutableStateFlow(false)
    /** A check is on its way (the banner's Retry says "Trying…"). */
    val trying: StateFlow<Boolean> = _trying.asStateFlow()

    private val _reason = MutableStateFlow(OfflineReason.UNKNOWN)
    /** Why the last check failed ([OfflineReason]), for the offline sheet; kept while offline mode is on. */
    val reason: StateFlow<String> = _reason.asStateFlow()

    @Volatile private var app: Context? = null
    private val checking = Mutex()
    private val flushLock = Any()
    private var retryJob: Job? = null
    /** the phone has a network (the browser's `navigator.onLine`), kept by the default-network callback of [start] */
    @Volatile private var online = true
    /** when a failed call last asked for a check ([noticeUnreachable]), in epoch ms */
    @Volatile private var lastNotice = 0L
    /** the outbox is being sent ([syncSoon]) */
    private val syncing = AtomicBoolean(false)
    /** something was sent meanwhile: send again once that is done */
    @Volatile private var again = false

    /**
     * From the main screen (signed in) and the playback service (Android Auto without the app): watch the network
     * and ask the server now. The watch starts once per process; later calls only ask again.
     */
    @Synchronized
    fun start(context: Context) {
        val a = context.applicationContext
        if (app != null) { recheck(a, 0L); return }
        app = a
        OfflineStore.of(a)
        try {
            val cm = a.getSystemService(ConnectivityManager::class.java)
            if (cm != null) {
                online = cm.activeNetwork != null
                cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
                    override fun onAvailable(network: Network) { online = true; recheck(a, 1500L) }
                    override fun onLost(network: Network) { online = false; recheck(a, 1500L) }
                })
            }
        } catch (e: Exception) {
            CrashLog.log("offline: no network callback: ${e.message}")
        }
        recheck(a, 0L)
    }

    /** There is no server to ask: offline mode, or the phone has no network at all (the web's `no_server`). */
    fun noServer(): Boolean = _active.value || !online

    /** The banner's Retry: says so when the server still can't be reached. */
    fun retry(context: Context) {
        val a = context.applicationContext
        a.graph.scope.launch { if (!check(a)) Toasts.show(STILL_OFFLINE, "warn") }
    }

    /**
     * A call to the server just failed because it couldn't be reached (data/api/EncoreApi.kt): check the server now,
     * so offline mode (and its banner) starts without waiting for the network to change. At most once per
     * [NOTICE_GAP_MS]; nothing while offline mode is already on (its own retry loop runs) or before [start].
     */
    fun noticeUnreachable() {
        val a = app ?: return
        if (_active.value) return
        val now = System.currentTimeMillis()
        if (now - lastNotice < NOTICE_GAP_MS) return
        lastNotice = now
        recheck(a, 0L)
    }

    private fun recheck(a: Context, afterMs: Long) {
        a.graph.scope.launch {
            if (afterMs > 0) delay(afterMs)
            check(a)
        }
    }

    /** Asks the server once; true when it answered. Enters or leaves offline mode accordingly. */
    suspend fun check(a: Context): Boolean = checking.withLock {
        val graph = a.graph
        // the playback service can start before anything read the session from disk
        graph.store.awaitLoaded()
        val server = graph.store.current?.server
        if (server.isNullOrEmpty()) {
            _active.value = false
            return@withLock false
        }
        _trying.value = true
        // a phone that left home (or came back) finds the server at its other address first
        if (_active.value) ServerRoute.choose(a)
        val failure = try { withContext(Dispatchers.IO) { probe(graph, server) } } finally { _trying.value = false }
        val up = failure == null
        if (failure != null) _reason.value = failure
        if (up) {
            val was = _active.value
            // (a retry loop running this check ends by itself: offline mode is off now)
            _active.value = false
            if (was) CrashLog.log("offline mode: the server answers again")
            val (plays, likes) = withContext(Dispatchers.IO) { flush(a) }
            if (was) Toasts.show(Outbox.backOnlineText(plays, likes), "ok")
            if (likes > 0) graph.likes.refresh(force = true)
        } else {
            // offline mode starts whether or not songs are saved here: the pages the server answered before still
            // show (ApiCache), and Settings stays reachable
            val store = OfflineStore.of(a)
            val saved = withContext(Dispatchers.IO) { store.ensureLoaded(); store.songs().size }
            if (!_active.value) {
                CrashLog.log("offline mode: the server can't be reached, $saved songs saved here")
                _active.value = true
            }
            keepTrying(a)
        }
        up
    }

    private fun keepTrying(a: Context) {
        if (retryJob?.isActive == true) return
        retryJob = a.graph.scope.launch {
            while (isActive && _active.value) {
                delay(RETRY_MS)
                if (_active.value) check(a)
            }
        }
    }

    /**
     * Asks `/api/health` once. Null when it answered (any status but 502–504: a proxy saying the server behind it is
     * down counts as no answer); otherwise why not, in words ([OfflineReason]).
     */
    private fun probe(graph: AppGraph, server: String): String? {
        val client = graph.http.newBuilder()
            .connectTimeout(WAIT_SECONDS, TimeUnit.SECONDS)
            .callTimeout(WAIT_SECONDS, TimeUnit.SECONDS)
            .cache(null)
            .build()
        return try {
            client.newCall(Request.Builder().url("$server/api/health").build()).execute().use {
                if (it.code in 502..504) OfflineReason.of(phoneOnline = true, code = it.code, error = null, seconds = WAIT_SECONDS) else null
            }
        } catch (e: Exception) {
            // still asked without a network (a VPN or a LAN can answer anyway); without one, that is the reason
            OfflineReason.of(phoneOnline = online, code = null, error = e, seconds = WAIT_SECONDS)
        }
    }

    /** A failure that means the server could not be reached (no answer, or a proxy saying it is down). */
    fun unreachable(e: Throwable): Boolean = when (e) {
        is ApiException -> e.code in 502..504
        is IOException -> true
        else -> false
    }

    // ---- the outbox ----------------------------------------------------------------------------------------

    private fun store(): OfflineStore? = OfflineStore.instance

    /** A play the server didn't get: it waits, stamped with now. */
    fun queuePlay(id: Long, seconds: Double) {
        val s = store() ?: return
        val owner = s.owner()
        if (owner.isEmpty() || id <= 0L) return
        val sec = if (seconds.isFinite()) seconds.coerceAtLeast(0.0) else 0.0
        s.outbox.update { it.own(owner).addPlay(OutPlay(id, sec, System.currentTimeMillis() / 1000L)) }
    }

    /** A like (`liked`) or an unlike the server couldn't take: it waits; the hearts this phone keeps follow it. */
    fun queueLike(id: Long, liked: Boolean, likedNow: Set<Long>) {
        val s = store() ?: return
        val owner = s.owner()
        if (owner.isEmpty() || id <= 0L) return
        s.outbox.update { it.own(owner).addLike(OutLike(id, liked, System.currentTimeMillis() / 1000L)) }
        s.keepLikes(likedNow)
    }

    /** A like or an unlike of this song waits: a new tap goes after it, through the outbox, so the server sees them in order. */
    fun hasLike(id: Long): Boolean {
        val s = store() ?: return false
        val o = s.outbox.read()
        return o.belongsTo(s.owner()) && o.likes.any { it.trackId == id }
    }

    /** The liked songs the server listed, with the likes and unlikes still waiting applied. */
    fun withPending(ids: Set<Long>): Set<Long> {
        val s = store() ?: return ids
        val o = s.outbox.read()
        if (!o.belongsTo(s.owner()) || o.likes.isEmpty()) return ids
        val out = HashSet(ids)
        for (l in o.likes) if (l.liked) out.add(l.trackId) else out.remove(l.trackId)
        return out
    }

    /** Keep the server's list of liked songs for the hearts offline. */
    fun keepLikes(ids: Set<Long>) { store()?.keepLikes(ids) }

    /** The liked songs this phone last knew (for the hearts while the server can't be reached). */
    fun keptLikes(): Set<Long> = store()?.keptLikes() ?: emptySet()

    /**
     * After a play or a like the server took: what still waits from an earlier gap goes now (the web's
     * `sync_outbox`). Not in offline mode (that sends it itself when the server is back, and says how many went),
     * nothing when nothing waits, one send at a time. Any thread.
     */
    fun syncSoon() {
        val a = app ?: return
        if (_active.value) return
        if (!syncing.compareAndSet(false, true)) { again = true; return }
        a.graph.scope.launch {
            try {
                do {
                    again = false
                    if (_active.value || withContext(Dispatchers.IO) { pending() } == (0 to 0)) break
                    val (plays, likes) = withContext(Dispatchers.IO) { flush(a) }
                    if (likes > 0) a.graph.likes.refresh(force = true)
                } while (again && plays + likes > 0)
            } catch (e: Exception) {
                CrashLog.log("offline: outbox not sent: ${e.message}")
            } finally {
                syncing.set(false)
            }
        }
    }

    /** Plays and likes waiting: (plays, likes). */
    fun pending(): Pair<Int, Int> {
        val s = store() ?: return 0 to 0
        val o = s.outbox.read()
        return if (o.belongsTo(s.owner())) o.plays.size to o.likes.size else 0 to 0
    }

    /**
     * Sends what waits, when it belongs to the account whose session the app holds (`GET /api/auth/me`); another
     * account's outbox is dropped. The plays in the list form of `POST /api/library/plays`, 500 per call, then the
     * likes one by one (a 404 counts as sent: the song is gone). What failed stays. Returns (plays, likes) sent.
     */
    fun flush(context: Context): Pair<Int, Int> = synchronized(flushLock) {
        val s = OfflineStore.of(context)
        val api = context.graph.api
        val owner = s.owner()
        val doc = s.outbox.read()
        if (owner.isEmpty() || doc.isEmpty) return 0 to 0
        if (!doc.belongsTo(owner)) {
            s.outbox.update { if (it.belongsTo(owner)) it else it.own(owner) }
            return 0 to 0
        }
        // the session decides: never one account's plays and likes under another's
        val me = try { api.me() } catch (_: Exception) { return 0 to 0 }
        if (me.username.isNotEmpty() && OfflineStore.norm(s.server()) + "|" + me.username != owner) {
            CrashLog.log("offline: the outbox belongs to another account; dropped")
            s.outbox.update { Outbox(owner = OfflineStore.norm(s.server()) + "|" + me.username) }
            return 0 to 0
        }
        val sentPlays = ArrayList<OutPlay>()
        val sentLikes = ArrayList<OutLike>()
        var failed = false
        for (chunk in doc.plays.chunked(Outbox.PLAYS_PER_CALL)) {
            try {
                api.post("/api/library/plays", Outbox.playsBody(chunk))
                sentPlays.addAll(chunk)
            } catch (e: Exception) {
                CrashLog.log("offline: plays not sent: ${e.message}")
                failed = true
                break
            }
        }
        if (!failed) {
            for (l in doc.likes) {
                try {
                    if (l.liked) api.like(l.trackId) else api.unlike(l.trackId)
                    sentLikes.add(l)
                } catch (e: ApiException) {
                    if (e.code == 404) sentLikes.add(l) else break
                } catch (_: Exception) {
                    break
                }
            }
        }
        if (sentPlays.isNotEmpty() || sentLikes.isNotEmpty()) s.outbox.update { it.forget(sentPlays, sentLikes) }
        sentPlays.size to sentLikes.size
    }
}
