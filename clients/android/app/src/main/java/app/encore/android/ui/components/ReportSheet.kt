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
 * blocks the source the file came from, moves it to the trash and gets the music again from somewhere else.
 */
object ReportUi {
    /** kind "track" (id = track id) or "album" (id = album id). */
    class Request(val kind: String, val id: String, val title: String, val onDone: (() -> Unit)? = null)

    private val _open = MutableStateFlow<Request?>(null)
    val open: StateFlow<Request?> = _open

    fun track(id: Long, title: String, onDone: (() -> Unit)? = null) { _open.value = Request("track", id.toString(), title, onDone) }
    fun album(id: String, title: String, onDone: (() -> Unit)? = null) { _open.value = Request("album", id, title, onDone) }
    fun close() { _open.value = null }
}

/** The reasons, in the web's order and words. */
val REPORT_REASONS = listOf(
    "wrong_song" to "Wrong song",
    "wrong_version" to "Wrong version (live, remix, edit, missing parts)",
    "bad_audio" to "Bad audio quality",
    "loudness" to "Too quiet or too loud",
    "fake_lossless" to "Fake lossless",
    "glitches" to "Skips or glitches",
    "other" to "Other",
)

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
    var reason by remember { mutableStateOf("") }
    var note by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    val what = if (req.kind == "album") "album" else "song"
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 22.dp).padding(bottom = 24.dp)) {
            Text("Report a problem", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(vertical = 8.dp))
            Text(
                "What is wrong with “${req.title}”? Encore stops using the source this $what came from, moves the file${if (req.kind == "album") "s" else ""} to the trash for 7 days and gets the $what again from somewhere else. Plays, likes and playlists stay with the $what.",
                style = MaterialTheme.typography.bodySmall, color = p.muted,
            )
            Column(Modifier.padding(top = 10.dp)) {
                for ((key, label) in REPORT_REASONS) {
                    Row(Modifier.fillMaxWidth().clickable { reason = key }, verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = reason == key, onClick = { reason = key })
                        Text(label, style = MaterialTheme.typography.bodyMedium)
                    }
                }
            }
            EncoreField(note, { note = it }, placeholder = "Optional – e.g. the intro is missing", label = "Note", singleLine = false, minLines = 2, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
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
                                    Toasts.show(if (res.status == "replacing") "Reported – looking for a better copy" else res.message.ifEmpty { "Reported" }, "ok")
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
