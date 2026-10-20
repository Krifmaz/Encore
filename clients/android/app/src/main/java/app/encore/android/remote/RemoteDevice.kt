package app.encore.android.remote

import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import android.os.Handler
import android.os.Looper
import app.encore.android.AppGraph
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.str
import app.encore.android.data.api.ArtistCredit
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.graph
import app.encore.android.ui.components.Toasts
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.Call
import okhttp3.OkHttpClient
import okhttp3.Request
import java.util.concurrent.TimeUnit

/**
 * What runs the commands other devices send this phone, and says what it plays: the playback service
 * (remote/ServiceHost.kt). Both calls are made on the main thread.
 */
interface PlayerHost {
    /** web/js/player.js `snapshot()`: `{playing:false}` when nothing is loaded. */
    fun snapshot(): JsonObject
    /** web/js/player.js `command(c)`. */
    fun run(c: JsonObject)
    /** Pause here (this phone became a remote, or its music was sent to another device). */
    fun pauseLocal()
}

/** Another signed-in device, as GET /api/remote/devices and the event stream list it. */
data class DeviceInfo(val id: String, val name: String, val state: JsonObject) {
    val remoteOf: String get() = state.str("remote_of")
    val title: String get() = state.str("title")
    val playing: Boolean get() = state.bool("playing")
}

/** This phone is locked as a remote for [id] until you unlink. `state` is that device's latest check-in. */
data class Link(
    val id: String,
    val name: String,
    val state: JsonObject = JsonObject(emptyMap()),
    val online: Boolean = true,
    /** when `state` arrived (elapsed ms): the position moves on from there while it plays */
    val at: Long = System.currentTimeMillis(),
) {
    /** Playing there (and still checking in). */
    val playing: Boolean get() = online && state.bool("playing")
    /** Its song, or null when nothing plays there. */
    val track: Track? get() = RemoteDevice.trackOf(state)
    /** Its queue (at most 300 songs around the current one) and where its song is in it. */
    val queue: List<Track> get() = RemoteDevice.tracksOf(state["queue"])
    val index: Int get() = state.dbl("index").toInt()
    val durationSec: Double get() = state.dbl("duration")
    val volume: Double get() = if (state.containsKey("volume")) state.dbl("volume").coerceIn(0.0, 1.0) else 0.9
    /** player.js `quality()` of what plays there ("FLAC 1411k" …). */
    val signalLabel: String get() = (state["signal"] as? JsonObject)?.str("label") ?: ""
    /** Its shuffle mode and repeat ("off" | "all" | "one"), as its check-ins say. */
    val shuffle: Boolean get() = state.bool("shuffle")
    val repeat: String get() = state.str("repeat").takeIf { it == "all" || it == "one" } ?: "off"

    /** The linked device's position now, in seconds. */
    fun positionSec(now: Long = System.currentTimeMillis()): Double {
        val p = state.dbl("position")
        val d = state.dbl("duration")
        val moved = if (state.bool("playing") && online) (now - at).coerceAtLeast(0L) / 1000.0 else 0.0
        val out = p + moved
        return if (d > 0) out.coerceAtMost(d) else out
    }
}

/**
 * This phone as an Encore remote device (web/js/remote.js): it checks in every couple of seconds with what it
 * plays (POST /api/remote/heartbeat), keeps the server's event stream open (GET /api/remote/events) for commands
 * and the other devices' state, runs the commands it is sent through the playback service, and can be linked as a
 * remote for another device (remembered in prefs like the web's `encore.link`).
 *
 * Who keeps it running: the phone UI while it is in the foreground ("ui") and the playback service for its whole
 * life ("service", through [attach]). The event stream stays open while the UI is in front or music plays here;
 * otherwise the check-ins carry the commands.
 */
class RemoteDevice private constructor(context: Context, private val graph: AppGraph) {
    private val prefs: SharedPreferences = context.applicationContext.getSharedPreferences("encore_remote", Context.MODE_PRIVATE)
    private val api: EncoreApi get() = graph.api
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val main = Handler(Looper.getMainLooper())

    private val _devices = MutableStateFlow<List<DeviceInfo>>(emptyList())
    /** The other devices, as the server last told us. */
    val devices: StateFlow<List<DeviceInfo>> = _devices.asStateFlow()

    private val _link = MutableStateFlow<Link?>(savedLink())
    /** The device this phone controls, or null. */
    val link: StateFlow<Link?> = _link.asStateFlow()

    private val _name = MutableStateFlow(storedName())
    /** The name the user gave this phone ("" = the default, [defaultName]). */
    val customName: StateFlow<String> = _name.asStateFlow()

    private val _streaming = MutableStateFlow(false)
    /** The server's event stream is open: commands and the others' state arrive pushed. */
    val streaming: StateFlow<Boolean> = _streaming.asStateFlow()

    /** Main thread only. */
    private var host: PlayerHost? = null
    private val holders = HashSet<String>()
    private var beatJob: Job? = null
    private var eventsJob: Job? = null
    @Volatile private var eventsCall: Call? = null
    private val poke = Channel<Unit>(Channel.CONFLATED)
    private val seen = ArrayDeque<Long>()
    private var missed = 0
    /** Commands that came before the service was there to run them (main thread). */
    private val pending = ArrayList<Pair<Long, JsonObject>>()
    @Volatile private var hostPlaying = false

    /** The id this phone checks in under (made once, kept in the session store). */
    val me: String get() = graph.store.deviceId

    // ---- name -------------------------------------------------------------------------------------------

    /** "Android app (Pixel 8)" unless you named it. */
    fun name(): String = _name.value.ifEmpty { defaultName() }

    fun setName(n: String) {
        val v = n.trim().take(60)
        try { prefs.edit().putString(K_NAME, v).apply() } catch (_: Exception) {}
        _name.value = v
        beatSoon()
    }

    private fun storedName(): String = try { prefs.getString(K_NAME, "") ?: "" } catch (_: Exception) { "" }

    // ---- who keeps it running --------------------------------------------------------------------------

    /** Main thread. */
    fun retain(who: String) {
        if (!holders.add(who)) return
        update()
    }

    /** Main thread. */
    fun release(who: String) {
        if (!holders.remove(who)) return
        update()
    }

    /** The playback service is up: it runs the commands and says what plays. Main thread. */
    fun attach(h: PlayerHost) {
        host = h
        retain("service")
        // commands that arrived while the service was not there yet (at most a minute old)
        val now = System.currentTimeMillis()
        val queued = pending.filter { now - it.first < 60_000L }.map { it.second }
        pending.clear()
        for (c in queued) runNow(c)
        beatSoon()
    }

    /** Main thread. */
    fun detach(h: PlayerHost) {
        if (host !== h) return
        host = null
        hostPlaying = false
        release("service")
    }

    /** The service says whether music plays here (keeps the event stream open in the background). Main thread. */
    fun setPlaying(playing: Boolean) {
        if (hostPlaying == playing) return
        hostPlaying = playing
        update()
    }

    private fun update() {
        val run = holders.isNotEmpty()
        if (run && beatJob?.isActive != true) beatJob = scope.launch { beatLoop() }
        if (!run) { beatJob?.cancel(); beatJob = null }
        val stream = run && ("ui" in holders || hostPlaying)
        if (stream && eventsJob?.isActive != true) eventsJob = scope.launch { eventsLoop() }
        if (!stream && eventsJob != null) {
            eventsJob?.cancel()
            eventsJob = null
            try { eventsCall?.cancel() } catch (_: Exception) {}
            _streaming.value = false
        }
        if (run) beatSoon()
    }

    // ---- check-ins ---------------------------------------------------------------------------------------

    private val beatSoonRunnable = Runnable { poke.trySend(Unit) }

    /** Check in right away (debounced 150 ms): something about this phone's playback just changed. Any thread. */
    fun beatSoon() {
        main.removeCallbacks(beatSoonRunnable)
        main.postDelayed(beatSoonRunnable, 150L)
    }

    private suspend fun beatLoop() {
        while (currentCoroutineContext().isActive) {
            beat()
            // every 2 s; slower when only a paused service in the background keeps it going (web: hidden and idle)
            val idle = withContext(Dispatchers.Main) { "ui" !in holders && !hostPlaying && _link.value == null }
            withTimeoutOrNull(if (idle) 6000L else 2000L) { poke.receive() }
        }
    }

    private suspend fun beat() {
        if (graph.store.current == null || me.isEmpty()) return
        val l = _link.value
        val state: JsonObject = if (l != null) {
            buildJsonObject { put("playing", false); put("remote_of", l.id) }
        } else {
            withContext(Dispatchers.Main) { try { host?.snapshot() } catch (_: Exception) { null } } ?: IDLE
        }
        try {
            val body = buildJsonObject {
                put("id", me)
                put("name", name())
                put("state", state)
            }
            val r = api.post("/api/remote/heartbeat", body) as? JsonObject
            (r?.get("commands") as? JsonArray)?.forEach { c -> (c as? JsonObject)?.let { runCommand(it) } }
            // with the event stream open the server pushes the others' state; linked, we still ask so a device
            // that went quiet shows as offline
            if (l != null || !_streaming.value) gotDevices(api.get("/api/remote/devices"))
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            // offline: try again next time
        }
    }

    /** GET /api/remote/devices now (the Devices sheet when the stream is down). */
    fun refresh() {
        scope.launch {
            try { gotDevices(api.get("/api/remote/devices")) } catch (e: CancellationException) { throw e } catch (_: Exception) {}
        }
    }

    // ---- the server's event stream ---------------------------------------------------------------------

    private val sseClient: OkHttpClient by lazy {
        graph.http.newBuilder().cache(null).readTimeout(0, TimeUnit.MILLISECONDS).build()
    }

    private suspend fun eventsLoop() {
        var backoff = 2000L
        while (currentCoroutineContext().isActive) {
            if (graph.store.current == null || me.isEmpty()) { delay(5000L); continue }
            val opened = System.currentTimeMillis()
            try {
                stream()
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
            }
            _streaming.value = false
            // a stream that stayed up a while starts the backoff over
            if (System.currentTimeMillis() - opened > 60_000L) backoff = 2000L
            delay(backoff)
            backoff = (backoff * 2).coerceAtMost(30_000L)
        }
    }

    /** Blocking: reads events until the stream ends, fails or is cancelled (the call, or this coroutine). */
    private suspend fun stream() {
        val req = Request.Builder()
            .url(api.url("/api/remote/events?device=" + EncoreApi.enc(me)))
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .build()
        val call = sseClient.newCall(req)
        eventsCall = call
        val ctx = currentCoroutineContext()
        try {
            if (!ctx.isActive) return
            call.execute().use { res ->
                if (!res.isSuccessful) throw java.io.IOException("HTTP ${res.code}")
                _streaming.value = true
                val src = res.body.source()
                var event = ""
                val data = StringBuilder()
                // the server sends a keep-alive every 15 s, so a cancelled job notices within that
                while (ctx.isActive) {
                    val line = src.readUtf8Line() ?: break
                    when {
                        line.isEmpty() -> {
                            if (data.isNotEmpty()) dispatch(event, data.toString())
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

    private fun dispatch(event: String, data: String) {
        val o = try { api.json.parseToJsonElement(data) as? JsonObject } catch (_: Exception) { null } ?: return
        when (event) {
            "command" -> (o["command"] as? JsonObject)?.let { runCommand(it) }
            "devices" -> gotDevices(o["devices"])
        }
    }

    // ---- commands sent to this phone -------------------------------------------------------------------

    /** Run a command another device sent us – once, however many ways it arrived. Any thread. */
    private fun runCommand(c: JsonObject) {
        val seq = (c["seq"] as? JsonPrimitive)?.content?.toDoubleOrNull()?.toLong()
        if (seq != null) {
            synchronized(seen) {
                if (seen.contains(seq)) return
                seen.addLast(seq)
                while (seen.size > 200) seen.removeFirst()
            }
        }
        main.post { runNow(c) }
    }

    private fun runNow(c: JsonObject) {
        val h = host
        if (h == null) {
            // the service starts with the UI; until then keep it (a few, briefly)
            if (pending.size < 20) pending.add(System.currentTimeMillis() to c)
            return
        }
        val from = c.str("from")
        if (c.str("action") == "play_queue" && from.isNotEmpty()) Toasts.show("Music sent here from $from", "ok")
        try { h.run(c) } catch (_: Exception) {}
        beatSoon()
    }

    // ---- the other devices ----------------------------------------------------------------------------

    /** The other devices changed (pushed by the server, or fetched). Any thread. */
    private fun gotDevices(list: JsonElement?) {
        val arr = list as? JsonArray ?: return
        val mine = me
        val out = ArrayList<DeviceInfo>()
        for (e in arr) {
            val o = e as? JsonObject ?: continue
            val id = o.str("id")
            if (id.isEmpty() || id == mine) continue
            out.add(DeviceInfo(id, o.str("name"), o["state"] as? JsonObject ?: JsonObject(emptyMap())))
        }
        _devices.value = out
        val l = _link.value ?: return
        val d = out.firstOrNull { it.id == l.id }
        synchronized(this) {
            missed = if (d != null) 0 else missed + 1
            // two missed checks: offline (the bar says so)
            if (d != null) _link.value = l.copy(state = d.state, online = true, at = System.currentTimeMillis())
            else if (missed > 1 && l.online) _link.value = l.copy(online = false)
        }
    }

    // ---- linking ---------------------------------------------------------------------------------------

    /** Lock this phone as a remote for `d`: everything you play here plays there. */
    fun linkTo(d: DeviceInfo, silent: Boolean = false) {
        val name = d.name.ifEmpty { "another device" }
        try {
            prefs.edit().putString(K_LINK, buildJsonObject { put("id", d.id); put("name", name) }.toString()).apply()
        } catch (_: Exception) {}
        missed = 0
        _link.value = Link(d.id, name, d.state, online = true)
        main.post {
            try { host?.pauseLocal() } catch (_: Exception) {}
            update()
        }
        if (!silent) Toasts.show("Linked – this device now controls $name", "ok")
        beatSoon()
    }

    fun unlink() {
        try { prefs.edit().remove(K_LINK).apply() } catch (_: Exception) {}
        _link.value = null
        main.post { update() }
        Toasts.show("Unlinked – music plays on this device again", "ok")
        beatSoon()
    }

    private fun savedLink(): Link? {
        val raw = try { prefs.getString(K_LINK, null) } catch (_: Exception) { null } ?: return null
        val o = try { graph.api.json.parseToJsonElement(raw) as? JsonObject } catch (_: Exception) { null } ?: return null
        val id = o.str("id")
        if (id.isEmpty()) return null
        // until its first check-in arrives
        return Link(id, o.str("name").ifEmpty { "another device" }, JsonObject(emptyMap()), online = true)
    }

    // ---- sending commands ------------------------------------------------------------------------------

    /** POST /api/remote/{id}/command with `from` = this phone's name; errors become a toast. */
    fun send(id: String, body: JsonObject) {
        val withFrom = JsonObject(body + ("from" to JsonPrimitive(name())))
        scope.launch {
            try {
                api.post("/api/remote/${EncoreApi.enc(id)}/command", withFrom)
                if (!_streaming.value) { delay(400L); gotDevices(api.get("/api/remote/devices")) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't reach that device", "err")
            }
        }
    }

    /**
     * Send to the linked device, and show the change right away (the next check-in confirms it).
     * False when this phone isn't linked.
     */
    fun sendToLink(body: JsonObject): Boolean {
        val l = _link.value ?: return false
        send(l.id, body)
        val action = body.str("action")
        val s = l.state
        val pos = l.positionSec()
        val patched: Map<String, JsonElement>? = when (action) {
            "toggle" -> if (s.str("title").isNotEmpty()) mapOf("playing" to JsonPrimitive(!s.bool("playing")), "position" to JsonPrimitive(pos)) else null
            "pause" -> mapOf("playing" to JsonPrimitive(false), "position" to JsonPrimitive(pos))
            "play" -> mapOf("playing" to JsonPrimitive(true), "position" to JsonPrimitive(pos))
            "seek" -> mapOf("position" to JsonPrimitive(body.dbl("value")))
            "volume" -> mapOf("volume" to JsonPrimitive(body.dbl("value")))
            "shuffle" -> mapOf("shuffle" to JsonPrimitive(body.bool("value")))
            "repeat" -> mapOf("repeat" to JsonPrimitive(body.str("value")))
            else -> null
        }
        if (patched != null) {
            synchronized(this) {
                val cur = _link.value
                if (cur != null && cur.id == l.id) _link.value = cur.copy(state = JsonObject(cur.state + patched), at = System.currentTimeMillis())
            }
        }
        return true
    }

    companion object {
        private const val K_NAME = "device_name"
        private const val K_LINK = "link"
        /** What a device reports while nothing plays (this phone's service, or no service). */
        val IDLE: JsonObject = buildJsonObject { put("playing", false) }

        @Volatile private var instance: RemoteDevice? = null

        fun get(context: Context): RemoteDevice =
            instance ?: synchronized(this) {
                instance ?: RemoteDevice(context.applicationContext, context.graph).also { instance = it }
            }

        /** web/js/remote.js names other apps "Desktop app (Mac)"; this one is "Android app (<model>)". */
        fun defaultName(): String {
            val model = (Build.MODEL ?: "").trim()
            return if (model.isEmpty()) "Android app" else "Android app ($model)"
        }

        /** The web's `slim(t)`: what a queue entry carries between devices (client-shapes §5.1). */
        fun slim(t: Track): JsonObject = buildJsonObject {
            put("id", t.id)
            put("title", t.title)
            put("artist", t.artist)
            put("album", t.album)
            put("album_id", t.albumId)
            if (t.format.isNotEmpty()) put("format", t.format)
            put("duration", t.durationSec)
            // the loudness data and the file's facts travel with the song, so it plays normalized on the next device too
            t.gain?.let { put("gain", it.toJson()) }
            if (t.sampleRate > 0) put("sample_rate", t.sampleRate)
            if (t.bitDepth > 0) put("bit_depth", t.bitDepth)
        }

        fun slimList(tracks: List<Track>): JsonArray = buildJsonArray { for (t in tracks) add(slim(t)) }

        /**
         * player.js `libWindow()`: at most 300 songs around the current one (from 50 before it), and where the current
         * song is in them.
         */
        fun <T> window(queue: List<T>, index: Int, max: Int = 300): Pair<List<T>, Int> {
            if (queue.size <= max) return queue to index
            val from = maxOf(0, minOf(index - 50, queue.size - max))
            return queue.subList(from, from + max) to (index - from)
        }

        /** Songs in a command or a device's state (`tracks` / `queue`) as library tracks. */
        fun tracksOf(e: JsonElement?): List<Track> {
            val arr = e as? JsonArray ?: return emptyList()
            val out = ArrayList<Track>(arr.size)
            for (x in arr) (x as? JsonObject)?.let { o -> Track.from(o)?.let { out.add(it) } }
            return out
        }

        /** The song a device's state describes (id, title, artist, album, album_id, format, duration), or null. */
        fun trackOf(s: JsonObject): Track? {
            if (s.str("title").isEmpty()) return null
            val id = s.dbl("id").toLong()
            return Track(
                id = id,
                title = s.str("title"),
                artist = s.str("artist"),
                album = s.str("album"),
                albumArtist = s.str("artist"),
                albumId = s.str("album_id"),
                trackNo = 0,
                disc = 0,
                year = "",
                durationSec = s.dbl("duration"),
                format = s.str("format").lowercase(),
                bitrate = 0,
                artists = ArtistCredit.fromJson(s["artists"]),
            )
        }
    }
}
