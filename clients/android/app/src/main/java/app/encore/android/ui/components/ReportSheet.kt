package app.encore.android.ui.components

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.repo.features
import app.encore.android.ui.Nav
import app.encore.android.ui.screens.EncoreField
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * ⋯ → Report a problem (web/js/pages/report.js `reportModal`): what is wrong with a song or an album. The server
 * blocks the source the file came from, moves it to the trash and gets the music again from somewhere else – except
 * for the lyrics reasons (Wrong lyrics, Lyrics out of sync), which keep the song and only replace its lyrics.
 */
object ReportUi {
    /** kind "track" (id = track id) or "album" (id = album id); [reason] is ticked to start with ("" for none). */
    class Request(val kind: String, val id: String, val title: String, val onDone: (() -> Unit)? = null, val reason: String = "")

    private val _open = MutableStateFlow<Request?>(null)
    val open: StateFlow<Request?> = _open

    fun track(id: Long, title: String, onDone: (() -> Unit)? = null) { _open.value = Request("track", id.toString(), title, onDone) }

    /** Settings → Blocked → Replace: the reason the quality check suggests is ticked to start with. */
    fun track(id: Long, title: String, reason: String, onDone: (() -> Unit)?) { _open.value = Request("track", id.toString(), title, onDone, reason) }
    fun album(id: String, title: String, onDone: (() -> Unit)? = null) { _open.value = Request("album", id, title, onDone) }

    /**
     * The lyrics view's "Report lyrics" (Wrong lyrics ticked) or "Other timing" ([reason] = [ReportRules.LYRICS_SYNC]):
     * the sheet for song [id]; [onDone] reloads the lyrics.
     */
    fun lyrics(id: Long, title: String, onDone: (() -> Unit)?, reason: String = ReportRules.WRONG_LYRICS) {
        _open.value = Request("track", id.toString(), title, onDone, reason)
    }
    fun close() { _open.value = null }
}

/** The reasons, in the web's order and words (server: encore_requests::report::REASONS). */
val REPORT_REASONS = listOf(
    "wrong_song" to "Wrong song",
    "wrong_version" to "Wrong version (live, remix, edit, missing parts)",
    "bad_audio" to "Bad audio quality",
    "loudness" to "Too quiet or too loud",
    "fake_lossless" to "Fake lossless",
    "glitches" to "Skips or glitches",
    ReportRules.WRONG_LYRICS to "Wrong lyrics",
    ReportRules.LYRICS_SYNC to "Lyrics out of sync",
    "other" to "Other",
)

/** What the sheet shows and says for each kind and reason (no Compose: unit-tested). */
object ReportRules {
    /** The words are wrong (another song's, another version's). */
    const val WRONG_LYRICS = "wrong_lyrics"

    /** The words are right but the timing is off. */
    const val LYRICS_SYNC = "lyrics_sync"

    /** A reason about the lyrics: the song stays, only its lyrics are replaced. */
    fun isLyrics(reason: String): Boolean = reason == WRONG_LYRICS || reason == LYRICS_SYNC

    /** The reasons offered for a "track" or an "album" (lyrics are reported one song at a time). */
    fun reasonsFor(kind: String): List<Pair<String, String>> = if (kind == "album") REPORT_REASONS.filterNot { isLyrics(it.first) } else REPORT_REASONS

    /** The line under the title: what Encore will do, for the reason ticked now ("" for none yet). */
    fun explanation(kind: String, title: String, reason: String): String {
        val what = if (kind == "album") "album" else "song"
        return when (reason) {
            WRONG_LYRICS -> "The song itself stays as it is – nothing is deleted or downloaded again. Encore drops these lyrics for “$title”, never picks them again for it and looks for other lyrics right away (time-synced first). If there are none, the song shows no lyrics rather than wrong ones."
            LYRICS_SYNC -> "The song itself stays as it is – nothing is deleted or downloaded again. Encore drops this timing for “$title” and looks for another time-synced version right away; if there is none, the same words are shown without timing."
            else -> "What is wrong with “$title”? Encore stops using the source this $what came from, moves the file${if (kind == "album") "s" else ""} to the trash for 7 days and gets the $what again from somewhere else. Plays, likes and playlists stay with the $what."
        }
    }

    /** The toast once the server answered (status replacing | lyrics_found | lyrics_none | …, and its message). */
    fun doneToast(status: String, message: String): String = when (status) {
        "replacing" -> "Reported – looking for a better copy"
        "lyrics_found" -> "Reported – found other lyrics"
        "lyrics_none" -> "Reported – no other lyrics found, so none are shown"
        else -> message.ifEmpty { "Reported" }
    }
}

/** Drawn once at the root; shows the sheet while [ReportUi.open] holds a request. */
@Composable
fun ReportHost(nav: Nav) {
    val req by ReportUi.open.collectAsState()
    req?.let { ReportSheet(nav, it, onDismiss = { ReportUi.close() }) }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ReportSheet(nav: Nav, req: ReportUi.Request, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var reason by remember(req) { mutableStateOf(req.reason) }
    var note by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 22.dp).padding(bottom = 24.dp)) {
            Text("Report a problem", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(vertical = 8.dp))
            // the line follows the reason ticked: a lyrics reason keeps the song
            Text(ReportRules.explanation(req.kind, req.title, reason), style = MaterialTheme.typography.bodySmall, color = p.muted)
            Column(Modifier.padding(top = 10.dp)) {
                for ((key, label) in ReportRules.reasonsFor(req.kind)) {
                    Row(Modifier.fillMaxWidth().clickable { reason = key }, verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = reason == key, onClick = { reason = key })
                        Text(label, style = MaterialTheme.typography.bodyMedium)
                    }
                }
            }
            EncoreField(note, { note = it }, placeholder = if (ReportRules.isLyrics(reason)) "Optional – e.g. the lyrics are for the remix" else "Optional – e.g. the intro is missing", label = "Note", singleLine = false, minLines = 2, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
            Row(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                EncoreButton("Cancel") { onDismiss() }
                EncoreButton("Report", iconRes = R.drawable.ic_flag, primary = true, enabled = !busy) {
                    when {
                        reason.isEmpty() -> Toasts.show("Pick what is wrong", "err")
                        reason == "other" && note.isBlank() -> Toasts.show("Say what is wrong", "err")
                        else -> {
                            busy = true
                            scope.launch {
                                try {
                                    val body = buildJsonObject {
                                        put("kind", req.kind)
                                        put("reason", reason)
                                        put("note", note.trim())
                                        if (req.kind == "album") put("album_id", req.id) else put("track_id", req.id.toLongOrNull() ?: 0L)
                                    }
                                    val res = withContext(Dispatchers.IO) { nav.graph.features.report(body) }
                                    Toasts.show(ReportRules.doneToast(res.status, res.message), "ok")
                                    onDismiss()
                                    req.onDone?.invoke()
                                } catch (e: Exception) {
                                    Toasts.show(e.message ?: "Couldn't send the report", "err")
                                    busy = false
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
