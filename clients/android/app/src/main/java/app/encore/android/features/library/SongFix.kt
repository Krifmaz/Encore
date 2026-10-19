package app.encore.android.features.library

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Track
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.SongMatch
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MenuSeparator
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.EncoreField
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.screens.features
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Correcting a library song (web-rs/src/pages/library.rs `track_menu` and `fix_song_modal`): "Edit song details…"
 * (admins), "This is the right song" / "Undo “confirmed”" and "Delete song from disk" (admins) / "Wrong song – remove
 * it" (members with the `fix` permission, on songs their own request brought in). [changes] counts the changes, so
 * every song list on screen loads again ([OnSongFixed]).
 */
object SongFixUi {
    /** Who is signed in (role and permissions), loaded once per session at the root; null until it answers. */
    private val _me = MutableStateFlow<Me?>(null)
    val me: StateFlow<Me?> = _me

    fun setMe(m: Me?) { _me.value = m }

    /** The song whose "Edit song details" sheet is open, and whether it was opened from its album page. */
    class Edit(val track: Track, val onAlbumPage: Boolean)

    private val _edit = MutableStateFlow<Edit?>(null)
    val edit: StateFlow<Edit?> = _edit

    private val _delete = MutableStateFlow<Track?>(null)
    val delete: StateFlow<Track?> = _delete

    private val _changes = MutableStateFlow(0)
    val changes: StateFlow<Int> = _changes

    fun openEdit(t: Track, onAlbumPage: Boolean) { _edit.value = Edit(t, onAlbumPage) }
    fun askDelete(t: Track) { _delete.value = t }
    fun closeEdit() { _edit.value = null }
    fun closeDelete() { _delete.value = null }
    fun changed() { _changes.value = _changes.value + 1 }

    /** The song menu's correcting entries, in the web's order (empty for someone who can't correct it). */
    fun entries(nav: Nav, t: Track, me: Me?, scope: CoroutineScope, onAlbumPage: Boolean): List<MenuEntry?> {
        val admin = me?.isAdmin == true
        val out = ArrayList<MenuEntry?>()
        if (admin) out.add(MenuEntry("Edit song details…", R.drawable.ic_edit) { openEdit(t, onAlbumPage) })
        if (!LibraryFixRules.canFix(t, me)) return out
        if (t.verifyStatus == "manual") {
            out.add(MenuEntry("Undo “confirmed”", R.drawable.ic_close) {
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) { nav.features.verifyManual(listOf(t.id), correct = false) }
                        Toasts.show("Will be checked again", "ok")
                        changed()
                    } catch (e: CancellationException) {
                        throw e
                    } catch (_: Exception) {
                    }
                }
            })
        } else {
            out.add(MenuEntry("This is the right song", R.drawable.ic_check) {
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) { nav.features.verifyManual(listOf(t.id)) }
                        Toasts.show("Marked as confirmed – the audio check won’t flag it again", "ok")
                        changed()
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "That didn't work", "err")
                    }
                }
            })
        }
        out.add(MenuSeparator)
        out.add(MenuEntry(if (admin) "Delete song from disk" else "Wrong song – remove it", R.drawable.ic_delete, danger = true) { askDelete(t) })
        return out
    }
}

/**
 * Calls [reload] each time a song is corrected or deleted from its ⋯ menu (or its album from the album page) while
 * this list is on screen, so a deleted song goes and a confirmed one offers "Undo “confirmed”" (the web passes a reload
 * to `track_menu`).
 */
@Composable
fun OnSongFixed(reload: () -> Unit) {
    val latest by rememberUpdatedState(reload)
    // a StateFlow gives its current value first: only the changes after that count
    LaunchedEffect(Unit) { SongFixUi.changes.drop(1).collect { latest() } }
}

/** The audio check's verdict beside a song (library.rs `verify_badge`); a tap shows what the check found. */
@Composable
fun VerifyBadge(t: Track) {
    val (label, kind) = LibraryFixRules.verifyBadge(t.verifyStatus) ?: return
    val detail = t.verifyDetail.ifEmpty { if (t.verifyStatus == "manual") "Confirmed by hand" else "" }
    Badge(label, kind = kind, onClick = if (detail.isNotEmpty()) ({ Toasts.show(detail, "info") }) else null)
}

/** Drawn once at the root: the "Edit song details" sheet and the "Delete song" confirm. */
@Composable
fun SongFixHost(nav: Nav) {
    val scope = rememberCoroutineScope()
    val edit by SongFixUi.edit.collectAsState()
    val del by SongFixUi.delete.collectAsState()
    edit?.let { e -> SongFixSheet(nav, e, onDismiss = { SongFixUi.closeEdit() }) }
    del?.let { t ->
        ConfirmDialog(
            title = "Delete song",
            text = "Permanently delete \"${t.title}\" from disk?",
            okLabel = "Delete",
            danger = true,
            onDismiss = { SongFixUi.closeDelete() },
        ) {
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { nav.features.deleteTrack(t.id) }
                    Toasts.show("Song deleted", "ok")
                    SongFixUi.changed()
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Toasts.show(e.message ?: "That didn't work", "err")
                }
            }
        }
    }
}

/** The web's fix_song_modal fields: (key sent to the server, label). */
private val FIELDS = listOf(
    "title" to "Title",
    "artist" to "Artist",
    "album" to "Album",
    "albumartist" to "Album artist",
    "tracknumber" to "Track #",
    "date" to "Year",
)

/**
 * "Edit song details" (admins): change what this file is – title, artist, album, album artist, track number, year –
 * optionally moving the file to match; "Identify by its audio" offers what the fingerprint says it is.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SongFixSheet(nav: Nav, e: SongFixUi.Edit, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val t = e.track
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val values = remember(t.id) {
        mutableStateListOf(
            t.title,
            t.artist,
            t.album,
            t.albumArtist.ifEmpty { t.artist },
            if (t.trackNo > 0) t.trackNo.toString() else "",
            t.year,
        )
    }
    var move by remember { mutableStateOf(true) }
    var saving by remember { mutableStateOf(false) }
    var listening by remember { mutableStateOf(false) }
    var idMessage by remember { mutableStateOf("") }
    var matches by remember { mutableStateOf<List<SongMatch>>(emptyList()) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 22.dp).padding(bottom = 24.dp)) {
            Text("Edit song details", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(vertical = 8.dp))
            Text(
                "Change what this file is. Give a different version its own title (e.g. “… (Remix)”) or album so it stops counting as a duplicate. The file is moved to match and is never merged with another song.",
                style = MaterialTheme.typography.bodySmall, color = p.muted,
            )
            FIELDS.forEachIndexed { i, (_, label) ->
                EncoreField(values[i], { values[i] = it }, label = label, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
            }
            Row(Modifier.padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Switch(checked = move, onCheckedChange = { move = it })
                Spacer(Modifier.width(8.dp))
                Text("Move the file to match (Artist/Album/NN Title)", style = MaterialTheme.typography.bodyMedium)
            }
            Row(Modifier.padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                EncoreButton("Identify by its audio", iconRes = R.drawable.ic_flag, enabled = !listening) {
                    listening = true
                    idMessage = "Listening…"
                    scope.launch {
                        try {
                            val res = withContext(Dispatchers.IO) { nav.features.identifyTrack(t.id) }
                            matches = res
                            idMessage = if (res.isNotEmpty()) "Pick what it really is:" else "No match for this audio."
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            idMessage = e.message ?: "No match for this audio."
                        }
                        listening = false
                    }
                }
                if (listening) Spinner()
            }
            if (idMessage.isNotEmpty()) Text(idMessage, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(top = 6.dp))
            for (m in matches) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clickable {
                            values[0] = m.title
                            values[1] = m.artist
                            if (m.album.isNotEmpty()) values[2] = m.album
                            Toasts.show("Filled in – check and save", "ok")
                        }
                        .padding(vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(m.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                        Text(m.artist + if (m.album.isNotEmpty()) " · ${m.album}" else "", style = MaterialTheme.typography.bodySmall, color = p.muted)
                    }
                    if (m.score.isNotEmpty()) Badge("${m.score}%", kind = "ok")
                }
            }
            Row(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                EncoreButton("Cancel") { onDismiss() }
                EncoreButton("Save", primary = true, enabled = !saving) {
                    saving = true
                    scope.launch {
                        try {
                            val body = FIELDS.mapIndexed { i, (k, _) -> k to values[i] }.toMap()
                            val albumId = withContext(Dispatchers.IO) { nav.features.fixTrack(t.id, body, move) }
                            Toasts.show("Song fixed", "ok")
                            onDismiss()
                            // the song moved to another album: from its old album page, go to the new one
                            if (e.onAlbumPage && albumId.isNotEmpty() && albumId != t.albumId) {
                                nav.back()
                                nav.open(Screen.Album(albumId))
                            }
                            SongFixUi.changed()
                        } catch (e: CancellationException) {
                            throw e
                        } catch (ex: Exception) {
                            Toasts.show(ex.message ?: "That didn't work", "err")
                            saving = false
                        }
                    }
                }
            }
        }
    }
}
