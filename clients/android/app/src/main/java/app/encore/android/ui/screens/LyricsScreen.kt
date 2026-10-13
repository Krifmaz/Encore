package app.encore.android.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.R
import app.encore.android.ui.Nav
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

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
            fun s(k: String): String { val v = o[k] as? JsonPrimitive ?: return ""; return if (v.isString) v.content else v.content.takeIf { it != "null" } ?: "" }
            val synced = (o["synced"] as? JsonPrimitive)?.content == "true"
            val text = s("lyrics")
            return Lyrics(
                lines = if (synced) parseLrc(text) else emptyList(),
                plain = if (synced) "" else text,
                instrumental = (o["instrumental"] as? JsonPrimitive)?.content == "true",
                synced = synced,
                source = s("source"),
            )
        }
    }
}

/**
 * Full-screen lyrics for the song that plays (web/js/player.js lyrics pane + pages/lyrics.js): time-synced lines
 * follow the position and a tap on one seeks there; plain lyrics scroll. Opened from the player's ⋯ menu.
 */
@Composable
fun LyricsScreen(nav: Nav, onClose: () -> Unit) {
    val p = MaterialTheme.palette
    val track by nav.player.nowPlaying.collectAsState()
    val position by nav.player.positionMs.collectAsState()
    val t = track
    BackHandler(onBack = onClose)

    var lyrics by remember { mutableStateOf<Lyrics?>(null) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(t?.id) {
        val id = t?.id ?: return@LaunchedEffect
        loading = true
        error = null
        lyrics = null
        try {
            lyrics = withContext(Dispatchers.IO) { Lyrics.from(nav.graph.api.lyrics(id)) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Lyrics lookup failed"
        }
        loading = false
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(p.bg)
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = {})
            .safeDrawingPadding(),
    ) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onClose) { Icon(painterResource(R.drawable.ic_expand_more), contentDescription = "Close", tint = p.text) }
            Column(Modifier.weight(1f), horizontalAlignment = Alignment.CenterHorizontally) {
                Text("Lyrics", style = MaterialTheme.typography.titleSmall, color = p.text)
                if (t != null) Text(
                    "${t.title} · ${t.artist}",
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            IconButton(onClick = onClose, enabled = false) { }
        }
        val l = lyrics
        when {
            t == null -> Empty("Nothing playing.", p.muted)
            loading -> Row(Modifier.fillMaxWidth().padding(24.dp), horizontalArrangement = androidx.compose.foundation.layout.Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.padding(end = 12.dp), color = p.accent, strokeWidth = 2.dp)
                Text("Looking for lyrics…", color = p.muted)
            }
            error != null -> Empty(error ?: "", p.muted)
            l == null -> Empty("No lyrics found for this song.", p.muted)
            l.instrumental -> Empty("Instrumental", p.muted)
            l.lines.isNotEmpty() -> SyncedLines(l, position / 1000.0, onSeek = { sec -> nav.player.seekTo((sec * 1000).toLong()) })
            l.plain.isNotBlank() -> Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 16.dp)) {
                Text(
                    "Plain lyrics · " + if (l.source == "file") "saved next to the song" else "from LRCLIB, saved next to the song",
                    style = MaterialTheme.typography.bodySmall,
                    color = p.faint,
                    modifier = Modifier.padding(bottom = 10.dp),
                )
                Text(l.plain, color = p.text, lineHeight = 26.sp, style = MaterialTheme.typography.bodyLarge)
            }
            else -> Empty("No lyrics found for this song.", p.muted)
        }
    }
}

@Composable
private fun Empty(text: String, color: androidx.compose.ui.graphics.Color) {
    Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
        Text(text, color = color, textAlign = TextAlign.Center)
    }
}

/** Time-synced lines: the current one (0.3 s ahead, like the web) is bright and kept in the middle; a tap seeks. */
@Composable
private fun SyncedLines(l: Lyrics, positionSec: Double, onSeek: (Double) -> Unit) {
    val p = MaterialTheme.palette
    val listState = rememberLazyListState()
    val at = positionSec + 0.3
    var current = -1
    for (i in l.lines.indices) if (l.lines[i].timeSec <= at) current = i else break
    LaunchedEffect(current) {
        if (current >= 0) {
            val viewport = listState.layoutInfo.viewportEndOffset - listState.layoutInfo.viewportStartOffset
            listState.animateScrollToItem(current, -(viewport / 2 - 40))
        }
    }
    LazyColumn(
        Modifier.fillMaxSize(),
        state = listState,
        contentPadding = PaddingValues(horizontal = 24.dp, vertical = 120.dp),
    ) {
        itemsIndexed(l.lines) { i, line ->
            val on = i == current
            Text(
                line.text.ifEmpty { "♪" },
                color = if (on) p.text else p.faint,
                fontWeight = if (on) FontWeight.Bold else FontWeight.Medium,
                fontSize = if (on) 24.sp else 20.sp,
                lineHeight = 30.sp,
                modifier = Modifier.fillMaxWidth().clickable { onSeek(line.timeSec) }.padding(vertical = 8.dp),
            )
        }
    }
}
