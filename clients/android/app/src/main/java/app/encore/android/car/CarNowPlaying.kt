package app.encore.android.car

import android.net.Uri
import android.os.SystemClock
import app.encore.android.data.api.EncoreApi
import app.encore.android.features.offline.OfflineStore
import app.encore.android.playback.ArtistNode
import app.encore.android.ui.screens.Lyrics
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** One synced lyric line: when it starts (ms into the song) and its text. */
data class CarLine(val timeMs: Long, val text: String)

/**
 * What plays now, as the playback service sees it (CarControls publishes it on the main thread). The Encore
 * Lyrics car app reads it from any thread; between publishes the position runs on from [atElapsedMs].
 * [positionMs] counts from the start of the song, also for server-made streams that started mid-song.
 */
data class NowPlaying(
    val mediaId: String,
    val trackId: Long?,
    val title: String,
    val artist: String,
    val album: String,
    val artworkUri: Uri?,
    val playing: Boolean,
    val positionMs: Long,
    val atElapsedMs: Long,
    val speed: Float,
    /** the song's artist as a browse node (the car app's artist row opens it); null when the song names none */
    val artistNode: ArtistNode? = null,
) {
    fun positionNow(): Long =
        if (!playing) positionMs else positionMs + ((SystemClock.elapsedRealtime() - atElapsedMs) * speed).toLong()
}

/** The playback service → car app link inside the app's process. */
object CarNowPlaying {
    private val flow = MutableStateFlow<NowPlaying?>(null)
    private val watching = MutableStateFlow(0)

    val state: StateFlow<NowPlaying?> = flow.asStateFlow()

    /** How many car screens show the live lyrics now: while above 0 the service re-publishes the position every second. */
    val watchers: StateFlow<Int> = watching.asStateFlow()

    fun publish(np: NowPlaying?) { flow.value = np }

    fun watch(on: Boolean) {
        synchronized(watching) { watching.value = (watching.value + if (on) 1 else -1).coerceAtLeast(0) }
    }
}

/**
 * Synced lyrics per song, shared by the car card (CarControls) and the Encore Lyrics car app: fetched once and
 * kept for the last few songs. Blocking; call it off the main thread.
 */
object LyricsCache {
    private const val KEEP = 12
    private val cache = object : LinkedHashMap<String, List<CarLine>>(16, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, List<CarLine>>?): Boolean = size > KEEP
    }

    /** The song's synced lines; empty when it has none (or plain lyrics only). Errors are not cached. */
    fun synced(api: EncoreApi, trackId: Long): List<CarLine> {
        val key = api.base + "#" + trackId
        synchronized(cache) { cache[key]?.let { return it } }
        val lines = try {
            // offline: the lyrics saved with a downloaded song
            val o = try { api.lyrics(trackId) } catch (e: Exception) { OfflineStore.instance?.savedLyrics(trackId) ?: throw e }
            val l = Lyrics.from(o)
            if (l.synced) l.lines.map { CarLine((it.timeSec * 1000).toLong(), it.text) } else emptyList()
        } catch (_: Exception) {
            return emptyList()
        }
        synchronized(cache) { cache[key] = lines }
        return lines
    }

    /** The index of the line playing at [positionMs] (-1 before the first). A small lead so the line shows as it is sung. */
    fun lineAt(lines: List<CarLine>, positionMs: Long): Int {
        val t = positionMs + 150
        var lo = 0
        var hi = lines.size - 1
        var at = -1
        while (lo <= hi) {
            val mid = (lo + hi) ushr 1
            if (lines[mid].timeMs <= t) { at = mid; lo = mid + 1 } else hi = mid - 1
        }
        return at
    }
}
