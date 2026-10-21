package app.encore.android.features.group

import android.content.Context
import android.content.SharedPreferences
import android.os.SystemClock
import app.encore.android.AppGraph
import app.encore.android.core.str
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.Track
import app.encore.android.graph
import app.encore.android.remote.RemoteDevice
import app.encore.android.ui.components.Toasts
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonObjectBuilder
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import okhttp3.Call
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.OkHttpClient
import okhttp3.Request
import java.util.concurrent.TimeUnit

/**
 * This phone in a group listen (0.31): the group it is in ([state], from the server's answers and its event stream),
 * the server's clock ([serverNow]), the live group listens on the server ([live], for Home and the devices sheet), and
 * every action – start, join by code or from the list, leave, end, the host's settings and the playback commands.
 *
 * The playback service's [GroupSync] makes the player follow [state]; the UI only reads the flows and calls the
 * actions. One per process (the UI and the service share it); any thread.
 *
 * Accounts only: guests join through the web page. Older servers answer 404 on the routes: [available] goes false
 * and every entry hides, with no error shown.
 */
object GroupListen {
    @Volatile private var graph: AppGraph? = null
    private lateinit var prefs: SharedPreferences
    private lateinit var api: GroupApi
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    private val _state = MutableStateFlow<GroupState?>(null)
    /** The group this phone is in, or null. */
    val state: StateFlow<GroupState?> = _state.asStateFlow()

    private val _available = MutableStateFlow<Boolean?>(null)
    /** The server has group listens (null: not asked yet; false: turned off, or a server before 0.31). */
    val available: StateFlow<Boolean?> = _available.asStateFlow()

    private val _live = MutableStateFlow<List<GroupSummary>>(emptyList())
    /** The live group listens this account may see ("Listening together now"). */
    val live: StateFlow<List<GroupSummary>> = _live.asStateFlow()

    private val _localPaused = MutableStateFlow(false)
    /** This member paused only here ("Paused here · Catch up"); the group plays on. */
    val localPaused: StateFlow<Boolean> = _localPaused.asStateFlow()

    private val _nudge = MutableStateFlow(0)
    /** Bumped when the player should follow the group again (a refused command, Catch up). */
    val nudge: StateFlow<Int> = _nudge.asStateFlow()

    // dialogs the root draws (EncoreRoot): the group sheet, Start, Join with a code, and "Join <host>'s group listen?"
    val sheetOpen = MutableStateFlow(false)
    val startOpen = MutableStateFlow(false)
    val joinOpen = MutableStateFlow(false)
    val invite = MutableStateFlow<GroupInvite?>(null)
    /** a group link shared to the app (ShareActivity → MainActivity), opened once signed in (EncoreRoot) */
    val pendingLink = MutableStateFlow<String?>(null)
    /** a start or join is on its way (the dialogs' buttons wait) */
    val busy = MutableStateFlow(false)

    /**
     * The server's clock minus this phone's time since boot (SystemClock.elapsedRealtime), ms, from GET /api/time
     * ([GroupClock]: 8 round trips at a join, 3 every minute after, blended in). Not the wall clock: Android steps that
     * when it sets the time from the network, and every expected position would jump with it. Until measured (or on a
     * server without /api/time) the phone's wall clock stands in.
     */
    private val clock = GroupClock()
    val offsetMs: Long get() = clock.offsetMs
    /** the server process the group was last seen on: another one after a restart */
    @Volatile private var boot: String = ""
    @Volatile private var restored = false
    private var eventsJob: Job? = null
    /** the clock measured again every minute while in a group (GroupClock) */
    private var clockJob: Job? = null
    @Volatile private var eventsCall: Call? = null

    /** In a group now (any thread). */
    val inGroup: Boolean get() = _state.value != null

    /** The server's clock now (Unix ms). */
    fun serverNow(): Long = if (clock.measured) SystemClock.elapsedRealtime() + clock.offsetMs else System.currentTimeMillis()

    /** Ready to use; called by the UI and the playback service (either may come first). */
    fun init(context: Context) {
        if (graph != null) return
        synchronized(this) {
            if (graph != null) return
            val g = context.graph
            val remote = RemoteDevice.get(context)
            prefs = context.applicationContext.getSharedPreferences("encore_group", Context.MODE_PRIVATE)
            api = GroupApi(g.api, g.http, { g.store.deviceId }, { remote.name() })
            graph = g
        }
    }

    private fun g(): AppGraph? = graph

    // ---- this device's group after a restart of the app ------------------------------------------------------

    /**
     * The group this phone was in when the app last ran: still there (GET /api/group/current) → back in it, the event
     * stream opens and the player follows. Once per process.
     */
    fun restore() {
        val graph = g() ?: return
        if (restored) return
        restored = true
        val saved = try { prefs.getString(K_ID, null) } catch (_: Exception) { null } ?: return
        scope.launch {
            try { graph.store.awaitLoaded() } catch (_: Exception) {}
            if (graph.store.current == null) return@launch
            val s = try {
                api.current()
            } catch (e: ApiException) {
                if (e.code in GONE) null else return@launch
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // offline: the next start tries again
                restored = false
                return@launch
            }
            if (s != null && s.id == saved) {
                measureClock()
                // back in, but quiet: this also runs when Android or the car starts the service, maybe hours later, so
                // the listener chooses to hear the group ("Paused here · Catch up"); only start and join play at once
                _localPaused.value = true
                adopt(s)
            } else {
                forget()
            }
        }
    }

    // ---- the server's list -------------------------------------------------------------------------------------

    /**
     * GET /api/group now: the list and whether the server has group listens at all. Quiet on any error. A server
     * without them (before 0.31, or turned off) is asked again only every 10 minutes, not every 20 s Home is shown.
     */
    suspend fun refreshList() {
        val graph = g() ?: return
        if (graph.store.current == null) return
        // signed in to another server since: nothing known about this one yet
        val base = graph.api.base
        if (base != listFor) { listFor = base; _available.value = null; _live.value = emptyList(); noGroupsAt = 0L }
        if (_available.value == false && noGroupsAt > 0L && SystemClock.elapsedRealtime() - noGroupsAt < NO_GROUPS_ASK_MS) return
        try {
            val list = withContext(Dispatchers.IO) { api.list() }
            _live.value = list
            _available.value = true
            noGroupsAt = 0L
        } catch (e: CancellationException) {
            throw e
        } catch (e: ApiException) {
            if (e.code in GONE || e.code == 405) {
                _available.value = false
                _live.value = emptyList()
                noGroupsAt = SystemClock.elapsedRealtime()
            }
        } catch (_: Exception) {
        }
    }

    /** when the server last said it has no group listens (elapsed ms; 0: it has them, or not asked) */
    @Volatile private var noGroupsAt = 0L

    // ---- start and join --------------------------------------------------------------------------------------

    /** Start a group listen: what plays here (library songs only) becomes its queue. */
    fun start(visibility: String, everyoneControls: Boolean, queue: List<Track>, index: Int, positionSec: Double, playing: Boolean, onDone: () -> Unit = {}) {
        if (g() == null || busy.value) return
        val lib = queue.filter { it.id > 0L }
        val idx = queue.take(index.coerceAtLeast(0)).count { it.id > 0L }.coerceIn(0, maxOf(0, lib.size - 1))
        val (win, at) = RemoteDevice.window(lib, idx, 1000)
        busy.value = true
        scope.launch {
            try {
                measureClock()
                val s = api.start(visibility, everyoneControls, win.map { it.id }, at, if (win.isEmpty()) 0.0 else positionSec, playing && win.isNotEmpty())
                unlinkFirst()
                adopt(s)
                sheetOpen.value = true
                withContext(Dispatchers.Main) { onDone() }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't start the group listen", "err")
            } finally {
                busy.value = false
            }
        }
    }

    /**
     * A code or invite link to join (typed, or shared to the app): who hosts it is looked up first and the root asks
     * "Join <host>'s group listen?". A link to another server is refused.
     */
    fun openInvite(text: String) {
        val graph = g() ?: return
        val code = GroupRules.codeFrom(text)
        if (code == null) {
            Toasts.show("That isn't a group listen code or link", "err")
            return
        }
        val server = GroupRules.serverOf(text)
        if (server != null && !sameServer(server, graph.api.base)) {
            Toasts.show("That group listen is on another server (${server.substringAfter("://")})", "err")
            return
        }
        if (_state.value?.code == code) { sheetOpen.value = true; return }
        scope.launch {
            val summary = try {
                api.lookup(code)
            } catch (e: CancellationException) {
                throw e
            } catch (e: ApiException) {
                if (e.code == 404) {
                    Toasts.show("No group listen with that code", "err")
                    return@launch
                }
                if (e.code == 403) {
                    Toasts.show(e.message?.takeIf { !it.startsWith("HTTP ") } ?: GroupRules.endedText("off", ""), "err")
                    return@launch
                }
                // too many wrong codes (client-shapes §3.7): the join would be refused too
                if (e.code == 429) {
                    Toasts.show(e.message?.takeIf { !it.startsWith("HTTP ") } ?: "Too many tries. Wait a moment and try again.", "err")
                    return@launch
                }
                // the lookup failed otherwise: ask without the host's name, the join says the rest
                null
            } catch (_: Exception) {
                null
            }
            invite.value = GroupInvite(code, summary)
        }
    }

    /** Join by code (the answer to "Join <host>'s group listen?" or the code box). */
    fun joinCode(code: String, onDone: () -> Unit = {}) = join({ api.joinCode(code) }, onDone)

    /** Join an "anyone" group from the server's list. */
    fun joinId(id: String, onDone: () -> Unit = {}) {
        if (_state.value?.id == id) { sheetOpen.value = true; onDone(); return }
        join({ api.joinId(id) }, onDone)
    }

    private fun join(call: () -> GroupState, onDone: () -> Unit) {
        if (g() == null || busy.value) return
        busy.value = true
        scope.launch {
            try {
                measureClock()
                // in another group: out of it first
                _state.value?.let { old -> try { api.leave(old.id) } catch (_: Exception) {} ; stop(null) }
                val s = call()
                unlinkFirst()
                _localPaused.value = false
                adopt(s)
                Toasts.show(if (s.youHost) "You joined your group listen" else "You joined ${s.hostName}'s group listen", "ok")
                withContext(Dispatchers.Main) { onDone() }
            } catch (e: CancellationException) {
                throw e
            } catch (e: ApiException) {
                Toasts.show(if (e.code == 404) e.message?.takeIf { it != "HTTP 404" } ?: "No group listen with that code" else e.message ?: "Couldn't join", "err")
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't join", "err")
            } finally {
                busy.value = false
            }
        }
    }

    /** Linking to another device is off while in a group: this phone plays the group's music itself. */
    private fun unlinkFirst() {
        val graph = g() ?: return
        val remote = RemoteDevice.get(graph.app)
        if (remote.link.value != null) remote.unlink()
    }

    // ---- leave, end, settings ----------------------------------------------------------------------------------

    /** Leave (the host leaving hands the group to the one who has been in it longest). */
    fun leave(quiet: Boolean = false) {
        val s = _state.value ?: return
        stop(if (quiet) null else "You left the group listen")
        scope.launch { try { api.leave(s.id) } catch (_: Exception) {} }
    }

    /** "End for everyone" (the host). */
    fun end() {
        val s = _state.value ?: return
        scope.launch {
            try {
                api.end(s.id)
                // the event stream's `ended` may have come first and said it already
                if (_state.value?.id == s.id) stop("You ended the group listen")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't end the group listen", "err")
            }
        }
    }

    /** The host's settings: any of everyone_controls, visibility, guests_allowed, max_members. */
    fun patch(build: JsonObjectBuilder.() -> Unit) {
        val s = _state.value ?: return
        scope.launch {
            try {
                apply(api.patch(s.id, build))
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't change the group listen", "err")
                _nudge.value = _nudge.value + 1
            }
        }
    }

    /** "Remove from the group" (guests only; the host or an admin). */
    fun removeMember(member: GroupMember) {
        val s = _state.value ?: return
        scope.launch {
            try {
                api.removeMember(s.id, member.key)?.let { apply(it) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't remove ${member.name}", "err")
            }
        }
    }

    // ---- playback -------------------------------------------------------------------------------------------

    /** This device may play, pause, seek and change the queue (the host, or "Everyone can control"). */
    fun canControl(): Boolean = _state.value?.youCanControl == true

    /** "Only Alex controls playback" as a toast; true when this device may not control. */
    fun deniedHere(): Boolean {
        val s = _state.value ?: return false
        if (s.youCanControl) return false
        Toasts.show(GroupRules.onlyHost(s), "info")
        return true
    }

    /**
     * POST /api/group/{id}/command {op, …}; a refusal shows the server's reason and the player follows the group again.
     * [onDone] runs once the group took it.
     */
    fun command(op: String, onDone: () -> Unit = {}, build: JsonObjectBuilder.() -> Unit = {}) {
        val s = _state.value ?: return
        scope.launch {
            try {
                api.command(s.id, op, build)?.let { apply(it) }
                onDone()
            } catch (e: CancellationException) {
                throw e
            } catch (e: ApiException) {
                Toasts.show(e.message ?: GroupRules.onlyHost(s), "err")
                _nudge.value = _nudge.value + 1
            } catch (_: Exception) {
                Toasts.show("Can't reach the server", "err")
                _nudge.value = _nudge.value + 1
            }
        }
    }

    private fun JsonObjectBuilder.ids(tracks: List<Track>) {
        putJsonArray("tracks") { for (t in tracks) if (t.id > 0L) add(JsonPrimitive(t.id)) }
    }

    /** "Play" on a list while in a group: it becomes the group's queue (those who control). True when the group took it. */
    fun playList(tracks: List<Track>, index: Int, positionSec: Double = 0.0): Boolean {
        if (!inGroup) return false
        if (deniedHere()) return true
        val lib = tracks.filter { it.id > 0L }
        if (lib.isEmpty()) { Toasts.show("Only songs from your library can be played in a group listen", "err"); return true }
        val idx = tracks.take(index.coerceAtLeast(0)).count { it.id > 0L }.coerceIn(0, lib.size - 1)
        val (win, at) = RemoteDevice.window(lib, idx, 1000)
        command("set_queue") { ids(win); put("index", at); put("position", positionSec) }
        return true
    }

    /**
     * A list's Shuffle (a playlist, an album, an artist) in a group: it becomes the group's queue, shuffled by the
     * server, from its first song (`set_queue` with `shuffle`) for those who control; the others add it to the group's
     * Up next, shuffled (`enqueue` with `shuffle`). The list is shuffled here too (an older server plays it as sent),
     * and a list of more than 1000 songs sends 1000 of them, picked at random. True when the group took it.
     */
    fun shuffleList(tracks: List<Track>): Boolean {
        if (!inGroup) return false
        val lib = tracks.filter { it.id > 0L }
        if (lib.isEmpty()) { Toasts.show("Only songs from your library can be played in a group listen", "err"); return true }
        val mixed = lib.shuffled().take(1000)
        if (canControl()) {
            command("set_queue") { ids(mixed); put("index", 0); put("position", 0.0); put("shuffle", true) }
        } else {
            command("enqueue", onDone = { Toasts.show("Added to the group's Up next, shuffled", "ok") }) { ids(mixed); put("shuffle", true) }
        }
        return true
    }

    /** Up next's Shuffle in a group (those who control): the songs after the playing one, in a new order for everyone. */
    fun shuffleUpNext() {
        if (!inGroup || deniedHere()) return
        command("shuffle")
    }

    /** Up next's Clear in a group (those who control, after "Clear the group's Up next?"): every song after the playing one goes. */
    fun clearUpNext() {
        if (!inGroup || deniedHere()) return
        command("clear")
    }

    /** "Add to queue": every member may. True when the group took it. */
    fun enqueue(tracks: List<Track>): Boolean {
        if (!inGroup) return false
        command("enqueue") { ids(tracks) }
        return true
    }

    /** "Play next": every member may. True when the group took it. */
    fun playNext(tracks: List<Track>): Boolean {
        if (!inGroup) return false
        command("play_next") { ids(tracks) }
        return true
    }

    /** Up next: play this entry now. */
    fun jump(entry: GroupEntry, index: Int) {
        if (deniedHere()) return
        command("jump") { put("index", index); put("qid", entry.qid) }
    }

    fun remove(entry: GroupEntry, index: Int) {
        if (deniedHere()) return
        command("remove") { put("index", index); put("qid", entry.qid) }
    }

    fun move(entry: GroupEntry, from: Int, to: Int) {
        if (deniedHere()) return
        command("move") { put("qid", entry.qid); put("from", from); put("to", to) }
    }

    /** This member paused here only (or started again). The group plays on. */
    fun setLocalPaused(paused: Boolean) {
        if (_localPaused.value == paused) return
        _localPaused.value = paused
    }

    /** "Catch up": play again where the group is now. */
    fun catchUp() {
        _localPaused.value = false
        _nudge.value = _nudge.value + 1
    }

    /** Follow the group again (a change here the group did not take). */
    fun resync() { _nudge.value = _nudge.value + 1 }

    // ---- state -------------------------------------------------------------------------------------------------

    /** A state of the group this phone is in, from the server (an answer or the event stream), when not older. */
    private fun apply(s: GroupState) {
        val old = _state.value
        // an answer that comes after leaving (or about another group) changes nothing: only start and join adopt a group
        if (old == null || old.id != s.id || s.rev < old.rev) return
        for (t in GroupRules.comings(old, s)) Toasts.show(t, "info")
        if (s.boot.isNotEmpty()) boot = s.boot
        _state.value = s
    }

    /** Now in [s]: remembered for a restart of the app, and its event stream opens. */
    private fun adopt(s: GroupState) {
        if (s.boot.isNotEmpty()) boot = s.boot
        _state.value = s
        try { prefs.edit().putString(K_ID, s.id).apply() } catch (_: Exception) {}
        openEvents(s.id)
        keepClock(s.id)
    }

    /** Out of the group (left, ended, removed); [message] as a toast. */
    private fun stop(message: String?) {
        eventsJob?.cancel()
        eventsJob = null
        clockJob?.cancel()
        clockJob = null
        try { eventsCall?.cancel() } catch (_: Exception) {}
        _state.value = null
        _localPaused.value = false
        sheetOpen.value = false
        forget()
        if (message != null) Toasts.show(message, "info")
        scope.launch { refreshList() }
    }

    private fun forget() {
        try { prefs.edit().remove(K_ID).apply() } catch (_: Exception) {}
    }

    // ---- the server's clock ----------------------------------------------------------------------------------

    /** [n] round trips to GET /api/time, one after the other; stops at a server without it. Blocking. */
    private fun samples(n: Int): List<ClockSample> {
        val out = ArrayList<ClockSample>(n)
        for (i in 0 until n) {
            try {
                // the phone's side in time since boot, which no clock setting moves
                val t0 = SystemClock.elapsedRealtime()
                val (now, b) = api.time()
                val t1 = SystemClock.elapsedRealtime()
                out.add(ClockSample(t0, now, t1))
                if (b.isNotEmpty() && boot.isEmpty()) boot = b
            } catch (e: ApiException) {
                // a server without /api/time: its clock is taken as this phone's
                break
            } catch (_: Exception) {
            }
        }
        return out
    }

    /** At a start or join (and back in the group after a restart of the app): 8 round trips, the offset set anew. Blocking. */
    private fun measureClock() {
        if (clock.join(samples(GroupClock.JOIN_SAMPLES))) clockAt = SystemClock.elapsedRealtime()
    }

    /** Every minute in a group: 3 round trips, blended into the offset (never more than 50 ms at once). Blocking. */
    private fun refreshClock() {
        clock.refresh(samples(GroupClock.REFRESH_SAMPLES))
        clockAt = SystemClock.elapsedRealtime()
    }

    @Volatile private var clockAt: Long = 0L

    // ---- the event stream ------------------------------------------------------------------------------------

    /**
     * The server sends a keep-alive every 15 s: three missed in a row and the connection is taken as dead (a NAT or
     * proxy that dropped it without a word), so [eventsLoop] reconnects instead of waiting forever.
     */
    private val sseClient: OkHttpClient by lazy {
        graph!!.http.newBuilder().cache(null).readTimeout(45, TimeUnit.SECONDS).build()
    }

    private fun openEvents(id: String) {
        if (eventsJob?.isActive == true && openFor == id) return
        eventsJob?.cancel()
        try { eventsCall?.cancel() } catch (_: Exception) {}
        openFor = id
        eventsJob = scope.launch { eventsLoop(id) }
    }

    @Volatile private var openFor: String = ""
    @Volatile private var listFor: String = ""

    private suspend fun eventsLoop(id: String) {
        var backoff = 1000L
        var rejoinedAt = Long.MIN_VALUE / 2
        while (currentCoroutineContext().isActive && _state.value?.id == id) {
            val opened = SystemClock.elapsedRealtime()
            try {
                stream(id)
            } catch (e: CancellationException) {
                throw e
            } catch (e: ApiException) {
                if (e.code in GONE) {
                    when (gone(id, e)) {
                        Back.IN -> {
                            // in again: its stream opens at once (after the usual wait when that was a moment ago too)
                            val again = opened - rejoinedAt < 30_000L
                            rejoinedAt = opened
                            if (!again) { backoff = 1000L; continue }
                        }
                        Back.RETRY -> {}
                        Back.OUT -> return
                    }
                }
            } catch (_: Exception) {
            }
            if (_state.value?.id != id) return
            if (SystemClock.elapsedRealtime() - opened > 60_000L) backoff = 1000L
            delay(backoff)
            backoff = (backoff * 2).coerceAtMost(15_000L)
        }
    }

    /**
     * The offset is measured again every minute while in [id] (start and join measure it first), whether or not the
     * stream dropped: 3 round trips, slow ones left out, blended in ([GroupClock]).
     */
    @Synchronized
    private fun keepClock(id: String) {
        if (clockJob?.isActive == true && clockFor == id) return
        clockJob?.cancel()
        clockFor = id
        clockJob = scope.launch {
            while (currentCoroutineContext().isActive && _state.value?.id == id) {
                delay(GroupClock.REFRESH_EVERY_MS)
                if (_state.value?.id != id) return@launch
                if (SystemClock.elapsedRealtime() - clockAt >= GroupClock.REFRESH_EVERY_MS - 1000L) refreshClock()
            }
        }
    }

    @Volatile private var clockFor: String = ""

    /** Blocking: reads events until the stream ends, fails or is cancelled. */
    private suspend fun stream(id: String) {
        val req = Request.Builder()
            .url(api.eventsUrl(id))
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .build()
        val call = sseClient.newCall(req)
        eventsCall = call
        val ctx = currentCoroutineContext()
        try {
            if (!ctx.isActive) return
            call.execute().use { res ->
                if (!res.isSuccessful) {
                    val text = try { res.body.string() } catch (_: Exception) { "" }
                    val o = try { graph!!.api.json.parseToJsonElement(text) as? JsonObject } catch (_: Exception) { null }
                    throw ApiException(res.code, o?.str("detail")?.ifEmpty { null } ?: "HTTP ${res.code}", o)
                }
                val src = res.body.source()
                var event = ""
                val data = StringBuilder()
                // the server sends a keep-alive comment every 15 s
                while (ctx.isActive) {
                    val line = src.readUtf8Line() ?: break
                    when {
                        line.isEmpty() -> {
                            if (data.isNotEmpty() && dispatch(id, event, data.toString())) return
                            event = ""
                            data.setLength(0)
                        }
                        line.startsWith(":") -> {}
                        line.startsWith("event:") -> event = line.substring(6).trim()
                        line.startsWith("data:") -> {
                            if (data.isNotEmpty()) data.append('\n')
                            data.append(line.substring(5).removePrefix(" "))
                        }
                    }
                }
            }
        } finally {
            if (eventsCall === call) eventsCall = null
        }
    }

    /** One event; true when the group is over (the stream is done). */
    private fun dispatch(id: String, event: String, data: String): Boolean {
        val el = try { graph!!.api.json.parseToJsonElement(data) } catch (_: Exception) { null } ?: return false
        if (_state.value?.id != id) return true
        when (event) {
            "", "state" -> GroupState.from(el)?.let { if (it.id == id) apply(it) }
            "ended" -> {
                val reason = (el as? JsonObject)?.str("reason") ?: ""
                val host = _state.value?.hostName ?: ""
                stop(if (reason == "ended" && _state.value?.youHost == true) "You ended the group listen" else GroupRules.endedText(reason, host))
                return true
            }
        }
        return false
    }

    /** What [gone] found: back in the group, ask again later (the server could not be asked), or out. */
    private enum class Back { IN, RETRY, OUT }

    /**
     * The event stream answered 404 / 403 / 410: this device was dropped (offline over 2 minutes) or the group is gone.
     * Back in with the code when it is still there (client-shapes §3.17); else out: ended (after a server restart,
     * said so), or the join's own reason when the group is there but this device can't get back in (full, too many
     * tries, turned off).
     */
    private fun gone(id: String, e: ApiException): Back {
        val s = _state.value ?: return Back.OUT
        if (s.id != id) return Back.OUT
        if (e.code == 403) {
            stop(e.message?.takeIf { !it.startsWith("HTTP ") } ?: GroupRules.endedText("off", ""))
            return Back.OUT
        }
        val back = try {
            api.joinCode(s.code)
        } catch (j: ApiException) {
            when {
                j.code == 404 -> null
                // the server is in trouble for a moment: asked again with the stream
                j.code >= 500 -> return Back.RETRY
                else -> {
                    stop(GroupRules.rejoinRefused(j.code, j.message))
                    return Back.OUT
                }
            }
        } catch (_: Exception) {
            // the server can't be asked now: the stream is tried again, and with it the join
            return Back.RETRY
        }
        if (back != null) {
            if (back.id == id) {
                apply(back)
                return Back.IN
            }
            // the code belongs to another group now (this one ended and the code was given out again): not that one
            try { api.leave(back.id) } catch (_: Exception) {}
        }
        val nowBoot = try { api.time().second } catch (_: Exception) { "" }
        val restarted = nowBoot.isNotEmpty() && boot.isNotEmpty() && nowBoot != boot
        stop(GroupRules.endedText(if (restarted) "restart" else "", s.hostName))
        return Back.OUT
    }

    // ---- helpers -------------------------------------------------------------------------------------------

    /** Same scheme, host and port (and path prefix) as the signed-in server. */
    private fun sameServer(link: String, base: String): Boolean {
        val a = link.toHttpUrlOrNull() ?: return false
        val b = base.toHttpUrlOrNull() ?: return false
        return a.host == b.host && a.port == b.port
    }

    private const val K_ID = "group_id"
    /** a server without group listens is asked again after this long */
    private const val NO_GROUPS_ASK_MS = 600_000L
    /** answers that mean "no such group here" (or turned off) */
    private val GONE = setOf(403, 404, 410)
}
