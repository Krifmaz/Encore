package app.encore.android.features.library

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.obj
import app.encore.android.core.objects
import app.encore.android.core.str
import app.encore.android.ui.Nav
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject

/**
 * Bring your own music's sheets (web-rs/src/pages/own_file.rs): ⋯ → *Replace with my own file* on a song, and Library →
 * *Upload your own files*. Opened from anywhere through [OwnFileUi]; drawn once at the root by [OwnFileHost].
 */
object OwnFileUi {
    /** The song whose file is replaced ([onDone] reloads the page). */
    class Song(val id: Long, val title: String, val onDone: (() -> Unit)?)

    private val _song = MutableStateFlow<Song?>(null)
    /** The song of the open Replace with my own file sheet. */
    val song: StateFlow<Song?> = _song

    private val _bulk = MutableStateFlow(false)
    /** The Upload your own files sheet is open. */
    val bulk: StateFlow<Boolean> = _bulk

    /** Opens Replace with my own file for song [id]. */
    fun replace(id: Long, title: String, onDone: (() -> Unit)? = null) { _song.value = Song(id, title, onDone) }

    /** Opens Upload your own files. */
    fun openBulk() { _bulk.value = true }

    /** Closes both sheets. */
    fun close() { _song.value = null; _bulk.value = false }

    /** The ⋯ entry for song [id] (null for a Surround song or an account without the permission). */
    fun entry(me: app.encore.android.data.repo.Me?, id: Long, title: String, surround: Boolean, onDone: (() -> Unit)? = null): MenuEntry? =
        if (!surround && id > 0L && OwnFileRules.allowed(me)) MenuEntry("Replace with my own file", R.drawable.ic_upload) { replace(id, title, onDone) } else null
}

/** Drawn once at the root: the sheet [OwnFileUi] has open. */
@Composable
fun OwnFileHost(nav: Nav) {
    val song by OwnFileUi.song.collectAsState()
    val bulk by OwnFileUi.bulk.collectAsState()
    song?.let { ReplaceSheet(nav, it) }
    if (bulk) BulkSheet(nav)
}

/** The error's words for a toast or a line. */
private fun words(e: Exception, fallback: String): String = e.message?.ifEmpty { null } ?: fallback

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ReplaceSheet(nav: Nav, song: OwnFileUi.Song) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var busy by remember(song) { mutableStateOf(false) }
    var line by remember(song) { mutableStateOf("") }
    var ok by remember(song) { mutableStateOf(false) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri == null) return@rememberLauncherForActivityResult
        busy = true
        line = "Uploading and checking…"
        scope.launch {
            try {
                val v = withContext(Dispatchers.IO) { nav.graph.ownFiles.replace(song.id, context.contentResolver, uri) }
                line = "Replaced: " + OwnFileRules.resultText(v)
                ok = true
                Toasts.show("Replaced with your file", "ok")
                song.onDone?.invoke()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                line = words(e, "The upload failed")
                ok = false
            }
            busy = false
        }
    }
    ModalBottomSheet(onDismissRequest = { OwnFileUi.close() }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 22.dp).padding(bottom = 24.dp)) {
            Text("Replace with my own file", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(vertical = 8.dp))
            Text("Your file takes the place of “${song.title}” for everyone. It is tagged as this song, and the song keeps its plays, likes, playlists and lyrics. The file it replaces is deleted, or kept in the Trash while Keep deleted files in Trash is on.",
                style = MaterialTheme.typography.bodySmall, color = p.muted)
            if (line.isNotEmpty()) Text(line, style = MaterialTheme.typography.bodySmall, color = if (busy) p.muted else if (ok) EncoreColors.Ok else EncoreColors.Err, modifier = Modifier.padding(top = 10.dp))
            Row(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                EncoreButton("Close") { OwnFileUi.close() }
                if (!ok) EncoreButton(if (busy) "Uploading…" else "Choose a file", iconRes = R.drawable.ic_upload, primary = true, enabled = !busy) { pick.launch(OwnFileRules.MIME_TYPES) }
            }
        }
    }
}

/** One file of the batch: its name, the server's answer once staged, the song chosen and how it went. */
private class BulkRow(val uri: Uri, val name: String) {
    var staged by mutableStateOf<JsonObject?>(null)
    var pick by mutableStateOf<Long?>(null)
    var note by mutableStateOf("")
    var failed by mutableStateOf(false)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun BulkSheet(nav: Nav) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val rows = remember { mutableStateListOf<BulkRow>() }
    var uploading by remember { mutableStateOf(false) }
    var applying by remember { mutableStateOf(false) }
    var applied by remember { mutableStateOf(false) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris: List<Uri> ->
        val fresh = uris.filter { u -> rows.none { it.uri == u } }.map { BulkRow(it, OwnFileApi.displayName(context.contentResolver, it)) }
        if (fresh.isEmpty()) return@rememberLauncherForActivityResult
        rows.addAll(fresh)
        // one at a time, each matched by the server as it arrives
        uploading = true
        scope.launch {
            for (r in fresh) {
                r.note = "Uploading and matching…"
                try {
                    val v = withContext(Dispatchers.IO) { nav.graph.ownFiles.stage(context.contentResolver, r.uri) }
                    val s = v["files"].objectsFirst()
                    if (s != null) {
                        r.staged = s
                        r.pick = OwnFileRules.defaultPick(s)
                        r.note = ""
                    } else {
                        r.note = v["failed"].objectsFirst()?.str("reason")?.ifEmpty { null } ?: "The server didn't take this file"
                        r.failed = true
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    r.note = words(e, "The upload failed")
                    r.failed = true
                }
            }
            uploading = false
        }
    }
    val chosen = rows.count { it.staged != null && it.pick != null }
    ModalBottomSheet(onDismissRequest = { OwnFileUi.close() }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 22.dp).padding(bottom = 24.dp)) {
            Text("Upload your own files", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(vertical = 8.dp))
            Text("Choose your own copies of songs that are in the library. Each file is matched to its song by its tags (artist, title, album, length) and replaces that song's file for everyone, keeping its plays, likes, playlists and lyrics. Replaced files are deleted, or kept in the Trash while Keep deleted files in Trash is on.",
                style = MaterialTheme.typography.bodySmall, color = p.muted)
            if (!applied) EncoreButton("Choose files", iconRes = R.drawable.ic_upload, enabled = !applying, modifier = Modifier.padding(top = 12.dp)) { pick.launch(OwnFileRules.MIME_TYPES) }
            for (r in rows) {
                Column(Modifier.fillMaxWidth().padding(top = 12.dp)) {
                    Text(r.name, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                    val s = r.staged
                    if (s != null) {
                        val tags = s.obj("tags")
                        Text("${tags?.str("artist").orEmpty()} – ${tags?.str("title").orEmpty()} · ${s.str("format").uppercase()}", style = MaterialTheme.typography.bodySmall, color = p.muted)
                        if (!applied) {
                            ChoicePicker(OwnFileRules.choices(s), r.pick) { r.pick = it }
                            if (OwnFileRules.defaultPick(s) == null) Text("Not sure – choose the song or leave it out", style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn)
                        }
                    }
                    if (r.note.isNotEmpty()) Text(r.note, style = MaterialTheme.typography.bodySmall, color = if (r.failed) EncoreColors.Err else if (applied) EncoreColors.Ok else p.muted)
                }
            }
            Row(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                EncoreButton("Close") { OwnFileUi.close() }
                if (!applied) EncoreButton(OwnFileRules.applyLabel(chosen), primary = true, enabled = chosen > 0 && !uploading && !applying) {
                    applying = true
                    val items = rows.mapNotNull { r -> r.staged?.str("token")?.let { t -> r.pick?.let { t to it } } }
                    scope.launch {
                        try {
                            val v = withContext(Dispatchers.IO) { nav.graph.ownFiles.apply(items) }
                            for (r in rows) {
                                val token = r.staged?.str("token") ?: continue
                                OwnFileRules.outcome(v, token)?.let { (line, good) -> r.note = line; r.failed = !good }
                            }
                            applied = true
                            val n = v["replaced"].objectsAll().size
                            Toasts.show(if (n == 1) "Replaced 1 song" else "Replaced $n songs", "ok")
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            Toasts.show(words(e, "Couldn't replace the songs"), "err")
                        }
                        applying = false
                    }
                }
            }
        }
    }
}

/** A staged upload's song: the chosen one on a button that opens the list of its choices. */
@Composable
private fun ChoicePicker(choices: List<OwnFileRules.Choice>, picked: Long?, onPick: (Long?) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val label = choices.firstOrNull { it.id == picked }?.label ?: "Leave out"
    Box {
        TextButton(onClick = { open = true }) { Text(label, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.palette.accent) }
        PopMenu(expanded = open, onDismiss = { open = false }, entries = choices.map { c -> MenuEntry(c.label) { onPick(c.id); open = false } })
    }
}

/** The first object of a JSON array (null when there is none). */
private fun kotlinx.serialization.json.JsonElement?.objectsFirst(): JsonObject? = objectsAll().firstOrNull()

/** Every object of a JSON array. */
private fun kotlinx.serialization.json.JsonElement?.objectsAll(): List<JsonObject> = (this as? kotlinx.serialization.json.JsonArray)?.mapNotNull { it as? JsonObject } ?: objects()
