package app.encore.android.ui.screens

import android.content.Context
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.animateScrollBy
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.interaction.collectIsDraggedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.R
import app.encore.android.features.offline.OfflineStore
import app.encore.android.core.bool
import app.encore.android.core.str
import app.encore.android.ui.Nav
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject

/** One line of a time-synced (.lrc) lyric. */
data class LyricLine(val timeSec: Double, val text: String)

/** The lyrics as the server returns them (`GET /api/library/tracks/{id}/lyrics`), parsed like web/js/player.js. */
class Lyrics(val lines: List<LyricLine>, val plain: String, val instrumental: Boolean, val synced: Boolean, val source: String) {
    companion object {
        private val STAMP = Regex("""\[(\d+):(\d+(?:\.\d+)?)]""")
        private val ANY_TAG = Regex("""\[[^]]*]""")

        /** `[mm:ss.xx]words` lines, sorted by time; a line with several stamps appears once per stamp. */
        fun parseLrc(text: String): List<LyricLine> {
            val out = ArrayList<LyricLine>()
            for (line in text.split(Regex("\r?\n"))) {
                val stamps = STAMP.findAll(line).toList()
                val words = line.replace(ANY_TAG, "").trim()
                for (m in stamps) {
                    val t = (m.groupValues[1].toDoubleOrNull() ?: 0.0) * 60 + (m.groupValues[2].toDoubleOrNull() ?: 0.0)
                    out.add(LyricLine(t, words))
                }
            }
            return out.sortedBy { it.timeSec }
        }

        fun from(o: JsonObject): Lyrics {
            val synced = o.bool("synced")
            val text = o.str("lyrics")
            return Lyrics(
                lines = if (synced) parseLrc(text) else emptyList(),
                plain = if (synced) "" else text,
                instrumental = o.bool("instrumental"),
                synced = synced,
                source = o.str("source"),
            )
        }
    }
}

/**
 * Lyrics mode of the full player (the web's `encore.lyricsMode`): on, the cover shrinks to a thumbnail beside the
 * title and the lyrics take the screen. Remembered on this device, so the player opens the way you left it.
 */
object LyricsMode {
    private const val FILE = "encore_ui"
    private const val KEY = "encore.lyricsMode"
    @Volatile private var flow: MutableStateFlow<Boolean>? = null

    fun state(context: Context): StateFlow<Boolean> = flowOf(context).asStateFlow()

    fun set(context: Context, on: Boolean) {
        val f = flowOf(context)
        if (f.value == on) return
        f.value = on
        try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putBoolean(KEY, on).apply()
        } catch (_: Exception) {}
    }

    @Synchronized
    private fun flowOf(context: Context): MutableStateFlow<Boolean> {
        flow?.let { return it }
        val on = try {
            context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getBoolean(KEY, false)
        } catch (_: Exception) { false }
        return MutableStateFlow(on).also { flow = it }
    }
}

/**
 * "Lyrics" from a song's ⋯ menu or the player's menu (PlayerConnection.lyrics): the full player opens in lyrics mode.
 * Drawn once at the root; [onOpenPlayer] shows the full player.
 */
@Composable
fun LyricsRequests(nav: Nav, onOpenPlayer: () -> Unit) {
    val asked by nav.player.lyricsOpen.collectAsState()
    LaunchedEffect(asked) {
        if (!asked) return@LaunchedEffect
        LyricsMode.set(nav.graph.app, true)
        onOpenPlayer()
        nav.player.closeLyrics()
    }
}

/** The lyrics of one song as they load (kept by the full player while it is open, so the mode can flip freely). */
@Stable
class LyricsLoad {
    var lyrics by mutableStateOf<Lyrics?>(null)
    var loading by mutableStateOf(false)
    var error by mutableStateOf<String?>(null)
    /** the song these are for (-1: none yet) */
    var forId: Long = -1L
    /** bumped by [reload]: [rememberLyrics] asks the server again */
    var version by mutableStateOf(0)
    /** the next answer replaces the copy saved with a download (a lyrics report changed them) */
    var refreshSaved: Boolean = false

    /** Ask the server again (after a lyrics report: it has already looked for other lyrics). */
    fun reload() {
        forId = -1L
        refreshSaved = true
        version += 1
    }
}

/** Loads the lyrics of [trackId] once [active] (lyrics mode is on): from the server, or saved with a download. */
@Composable
fun rememberLyrics(nav: Nav, trackId: Long?, active: Boolean): LyricsLoad {
    val load = remember { LyricsLoad() }
    LaunchedEffect(trackId, active, load.version) {
        val id = trackId ?: return@LaunchedEffect
        if (!active || load.forId == id) return@LaunchedEffect
        load.forId = id
        load.loading = true
        load.error = null
        load.lyrics = null
        try {
            load.lyrics = withContext(Dispatchers.IO) {
                // offline: the lyrics saved with a downloaded song
                val o = try {
                    nav.graph.api.lyrics(id).also {
                        // after a lyrics report the copy saved with a download follows, so offline shows the new ones too
                        if (load.refreshSaved) OfflineStore.of(nav.graph.app).replaceLyrics(id, it)
                        load.refreshSaved = false
                    }
                } catch (e: Exception) { OfflineStore.of(nav.graph.app).savedLyrics(id) ?: throw e }
                Lyrics.from(o)
            }
        } catch (e: CancellationException) {
            // another song (or the player closed) before they came: asked again next time
            load.forId = -1L
            throw e
        } catch (e: Exception) {
            load.error = e.message ?: "Lyrics lookup failed"
        }
        load.loading = false
    }
    return load
}

/**
 * The lyrics pane of the full player (the web's lyrics mode): time-synced lines follow the position, the current one
 * large (28 sp, the others 22 sp) and kept in the middle with a smooth scroll, the lines already sung dimmed; a tap on
 * a line seeks there. Plain lyrics scroll.
 *
 * @param onReport the "Report lyrics" button in the bottom corner while lyrics are shown (wrong words or timing); null
 *   leaves it out (the TV, a Surround song, a file outside the library).
 */
@Composable
fun LyricsPane(nav: Nav, load: LyricsLoad, positionMs: Long, modifier: Modifier = Modifier, onReport: (() -> Unit)? = null) {
    val p = MaterialTheme.palette
    val l = load.lyrics
    Box(modifier) {
        when {
            load.loading -> Row(Modifier.fillMaxWidth().padding(24.dp), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.padding(end = 12.dp), color = p.accent, strokeWidth = 2.dp)
                Text("Looking for lyrics…", color = p.muted)
            }
            load.error != null -> Empty(load.error ?: "", p.muted)
            l == null -> Empty("No lyrics found for this song.", p.muted)
            l.instrumental -> Empty("Instrumental", p.muted)
            l.lines.isNotEmpty() -> SyncedLines(l, positionMs / 1000.0, onSeek = { sec -> nav.player.seekTo((sec * 1000).toLong()) })
            l.plain.isNotBlank() -> Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 16.dp)) {
                Text(
                    "Plain lyrics · " + if (l.source == "file") "saved next to the song" else "from LRCLIB, saved next to the song",
                    style = MaterialTheme.typography.bodySmall,
                    color = p.faint,
                    modifier = Modifier.padding(bottom = 10.dp),
                )
                Text(l.plain, color = p.text, fontSize = 22.sp, lineHeight = 30.sp, fontWeight = FontWeight.Medium)
            }
            else -> Empty("No lyrics found for this song.", p.muted)
        }
        // something is shown (words, or LRCLIB's "Instrumental"): it can be reported as wrong or out of sync
        val shown = !load.loading && load.error == null && l != null && (l.instrumental || l.lines.isNotEmpty() || l.plain.isNotBlank())
        if (onReport != null && shown) {
            SmallButton("Report lyrics", iconRes = R.drawable.ic_flag, modifier = Modifier.align(Alignment.BottomEnd).padding(12.dp)) { onReport() }
        }
    }
}

@Composable
private fun Empty(text: String, color: androidx.compose.ui.graphics.Color) {
    Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Text(text, color = color, textAlign = TextAlign.Center)
    }
}

/**
 * Time-synced lines: the current one (0.3 s ahead, like the web) bright, large and scrolled to the middle; the ones
 * already sung dimmed. While you drag the list it stays where you put it; the next line brings it back.
 */
@Composable
private fun SyncedLines(l: Lyrics, positionSec: Double, onSeek: (Double) -> Unit) {
    val p = MaterialTheme.palette
    val listState = rememberLazyListState()
    val dragged by listState.interactionSource.collectIsDraggedAsState()
    val at = positionSec + 0.3
    var current = -1
    for (i in l.lines.indices) if (l.lines[i].timeSec <= at) current = i else break
    LaunchedEffect(current) {
        if (current < 0 || dragged) return@LaunchedEffect
        val info = listState.layoutInfo
        val middle = (info.viewportStartOffset + info.viewportEndOffset) / 2
        val item = info.visibleItemsInfo.firstOrNull { it.index == current }
        if (item != null) {
            listState.animateScrollBy((item.offset + item.size / 2 - middle).toFloat(), tween(450))
        } else {
            // far away (a seek, or the first line): jump there, then centre it
            listState.scrollToItem(current)
            val again = listState.layoutInfo
            val it2 = again.visibleItemsInfo.firstOrNull { it.index == current } ?: return@LaunchedEffect
            listState.scrollBy((it2.offset + it2.size / 2 - (again.viewportStartOffset + again.viewportEndOffset) / 2).toFloat())
        }
    }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        // room above the first and below the last line, so both can sit in the middle
        val pad = (maxHeight / 2 - 24.dp).coerceAtLeast(0.dp)
        LazyColumn(
            Modifier.fillMaxSize(),
            state = listState,
            contentPadding = PaddingValues(start = 24.dp, end = 24.dp, top = pad, bottom = pad),
        ) {
            itemsIndexed(l.lines) { i, line ->
                val on = i == current
                val color by animateColorAsState(
                    when {
                        on -> p.text
                        i < current -> p.faint
                        else -> p.muted
                    },
                    tween(300),
                    label = "lyric",
                )
                Text(
                    line.text.ifEmpty { "♪" },
                    color = color,
                    fontWeight = if (on) FontWeight.Bold else FontWeight.SemiBold,
                    fontSize = if (on) 28.sp else 22.sp,
                    lineHeight = if (on) 34.sp else 28.sp,
                    modifier = Modifier.fillMaxWidth().clickable { onSeek(line.timeSec) }.padding(vertical = 8.dp),
                )
            }
        }
    }
}
