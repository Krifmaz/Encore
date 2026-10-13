package app.encore.android.data.prefs

import android.content.Context
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.session.SessionStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** GET /api/playback/dsp: is server-side Cadenza processing on, may this account use it, and the choices on offer. */
data class DspInfo(
    val enabled: Boolean,
    val allowed: Boolean,
    val maxRate: Int,
    val filters: List<Pair<String, String>>,
    val dithers: List<Pair<String, String>>,
    val defaultFilter: String,
    val defaultDither: String,
    /** The server keeps finished renders (pre-render cache): seeking and the next song need no new job. */
    val cacheOn: Boolean,
) {
    companion object {
        fun from(o: JsonObject): DspInfo {
            fun pairs(key: String): List<Pair<String, String>> = (o[key] as? JsonArray)?.mapNotNull { e ->
                val a = e as? JsonArray ?: return@mapNotNull null
                val id = (a.getOrNull(0) as? JsonPrimitive)?.content ?: return@mapNotNull null
                id to ((a.getOrNull(1) as? JsonPrimitive)?.content ?: id)
            } ?: emptyList()
            fun bool(key: String) = (o[key] as? JsonPrimitive)?.content == "true"
            fun num(key: String) = (o[key] as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0
            val cacheMb = ((o["cache"] as? JsonObject)?.get("max_mb") as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0
            return DspInfo(
                enabled = bool("enabled"),
                allowed = bool("allowed"),
                maxRate = num("max_rate").toInt(),
                filters = pairs("filters"),
                dithers = pairs("dithers"),
                defaultFilter = (o["default_filter"] as? JsonPrimitive)?.content ?: "",
                defaultDither = (o["default_dither"] as? JsonPrimitive)?.content ?: "tpdf",
                cacheOn = cacheMb > 0,
            )
        }
    }
}

/**
 * Playback preferences, kept on this device and synced with the account (web/js/playback.js):
 * after sign-in the account's `prefs.playback` wins over the device's (`adoptAccount`), and every change here is
 * saved locally first and then `PUT /api/auth/prefs {playback}` so it follows the user to every device.
 */
class PrefsRepository(context: Context, private val api: EncoreApi, private val scope: CoroutineScope, private val store: SessionStore? = null) {
    private val prefs = context.applicationContext.getSharedPreferences("encore_playback_prefs", Context.MODE_PRIVATE)

    private val _playback = MutableStateFlow(load())
    val playback: StateFlow<PlaybackPrefs> = _playback.asStateFlow()

    init {
        // "Then shuffle library" also lives in SessionStore.settings (M1, read by the Up next sheet): keep both in step
        if (store != null) {
            scope.launch {
                // changes only (the first value is the default before the store has been read)
                store.settings.map { it.autoplay }.distinctUntilChanged().drop(1).collect { on ->
                    if (on != _playback.value.autoplay) update { it.copy(autoplay = on) }
                }
            }
        }
    }

    private val _dsp = MutableStateFlow<DspInfo?>(null)
    /** null until GET /api/playback/dsp answered (or while signed out). */
    val dsp: StateFlow<DspInfo?> = _dsp.asStateFlow()

    private fun load(): PlaybackPrefs {
        val text = prefs.getString(KEY, null) ?: return PlaybackPrefs()
        return try { PlaybackPrefs.clean(Json.parseToJsonElement(text) as? JsonObject) } catch (_: Exception) { PlaybackPrefs() }
    }

    private fun store(p: PlaybackPrefs) {
        _playback.value = p
        prefs.edit().putString(KEY, p.toJson().toString()).apply()
        if (store != null && store.settings.value.autoplay != p.autoplay) store.setAutoplay(p.autoplay)
    }

    /** Change one or more fields; saved on this device and on the account (kept on this device if the server can't). */
    fun update(change: (PlaybackPrefs) -> PlaybackPrefs) {
        val next = PlaybackPrefs.clean(change(_playback.value).toJson())
        if (next == _playback.value) return
        store(next)
        scope.launch {
            try { withContext(Dispatchers.IO) { api.putPlaybackPrefs(next.toJson()) } } catch (_: Exception) { /* kept on this device */ }
        }
    }

    /** After sign-in / on foreground: the account's playback settings win over this device's. */
    fun adoptAccount(playback: JsonObject?) {
        if (playback == null) return
        val p = PlaybackPrefs.clean(playback)
        if (p != _playback.value) store(p)
    }

    /** Fetches the account's prefs (GET /api/auth/me → prefs.playback) and what the server offers (GET /api/playback/dsp). */
    fun sync() {
        scope.launch {
            try {
                val me = withContext(Dispatchers.IO) { api.meRaw() }
                adoptAccount((me["prefs"] as? JsonObject)?.get("playback") as? JsonObject)
            } catch (_: Exception) { /* offline, or signed out */ }
            refreshDsp()
        }
    }

    fun refreshDsp() {
        scope.launch {
            _dsp.value = try { withContext(Dispatchers.IO) { api.dspInfo() } } catch (_: Exception) { null }
        }
    }

    /** Signing out: forget the account's choices (the next account starts from its own). */
    fun clear() {
        prefs.edit().remove(KEY).apply()
        _playback.value = PlaybackPrefs()
        _dsp.value = null
    }

    private companion object {
        const val KEY = "playback"
    }
}
