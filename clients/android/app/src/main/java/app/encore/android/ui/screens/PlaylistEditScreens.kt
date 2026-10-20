package app.encore.android.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.repo.ImportPreview
import app.encore.android.data.repo.PLAYLIST_SOURCES
import app.encore.android.data.repo.PlaylistItem
import app.encore.android.data.repo.PlaylistPage
import app.encore.android.features.library.PlaylistShareRules
import app.encore.android.features.settings.copyToClipboard
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MenuSeparator
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.RowDivider
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// Playlist editing (web/js/pages/playlists.js): create, rename, delete, reorder, remove songs, import from a link
// or a pasted list, request the missing songs. The read-only Playlists / Playlist pages (design part) open these
// through [PlaylistsActions] and [PlaylistMenu].

/** Runs a call on IO, toasts the result (or the error) and then `after`. */
private fun act(scope: CoroutineScope, call: () -> String, after: () -> Unit = {}) {
    scope.launch {
        try {
            val msg = withContext(Dispatchers.IO) { call() }
            if (msg.isNotEmpty()) Toasts.show(msg, "ok")
            after()
        } catch (e: Exception) {
            Toasts.show(e.message ?: "That didn't work", "err")
        }
    }
}

/** A one-field dialog (new playlist name, rename). */
@Composable
fun NameDialog(title: String, initial: String, okLabel: String, placeholder: String = "", onDismiss: () -> Unit, onOk: (String) -> Unit) {
    val p = MaterialTheme.palette
    var name by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        title = { Text(title) },
        text = { EncoreField(name, { name = it }, placeholder = placeholder, imeAction = ImeAction.Done, onAction = { onDismiss(); onOk(name) }, modifier = Modifier.fillMaxWidth()) },
        confirmButton = { TextButton(onClick = { onDismiss(); onOk(name) }) { Text(okLabel, color = p.text, fontWeight = FontWeight.SemiBold) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/** playlists.js `albumChoice`: when a song is part of an album – just the song, the whole album, or the admin default. */
@Composable
fun AlbumChoiceDialog(title: String, onDismiss: () -> Unit, onPick: (albumMode: String?) -> Unit) {
    val p = MaterialTheme.palette
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("When a song is part of an album:", style = MaterialTheme.typography.bodyMedium)
                EncoreButton("Just the song", iconRes = R.drawable.ic_note, modifier = Modifier.fillMaxWidth()) { onDismiss(); onPick("song") }
                EncoreButton("Download the whole album (best quality available)", iconRes = R.drawable.ic_disc, modifier = Modifier.fillMaxWidth()) { onDismiss(); onPick("album") }
                EncoreButton("Use the admin default", modifier = Modifier.fillMaxWidth()) { onDismiss(); onPick(null) }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/** The Playlists page's New / Import buttons (the web's header buttons), for the design part's top bar. */
@Composable
fun PlaylistsActions(nav: Nav) {
    val scope = rememberCoroutineScope()
    var newName by remember { mutableStateOf(false) }
    IconButton(onClick = { newName = true }) { Icon(painterResource(R.drawable.ic_add), contentDescription = "New") }
    IconButton(onClick = { nav.open(Screen.PlaylistImport(null)) }) { Icon(painterResource(R.drawable.ic_download), contentDescription = "Import") }
    if (newName) {
        NameDialog("New playlist", "New playlist", "Create", onDismiss = { newName = false }) { name ->
            scope.launch {
                try {
                    val id = withContext(Dispatchers.IO) { nav.features.createPlaylist(name.ifBlank { "New playlist" }) }
                    if (id > 0L) nav.open(Screen.Playlist(id, name.ifBlank { "New playlist" }))
                } catch (e: Exception) {
                    Toasts.show(e.message ?: "Couldn't create the playlist", "err")
                }
            }
        }
    }
}

/**
 * The "⋯" of one playlist (the web's header buttons): Edit (reorder / remove), Add from link, Download missing,
 * Remove duplicates, Re-check library, Pull from source, Rename, Delete. For the design part's Playlist top bar.
 * [shareUrl] is the public link while the playlist is shared: Copy link and Stop sharing join the menu.
 */
@Composable
fun PlaylistMenu(nav: Nav, id: Long, name: String, isLiked: Boolean, onChanged: () -> Unit = {}, shareUrl: String? = null) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val features = nav.features
    var rename by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var confirmStop by remember { mutableStateOf(false) }
    var missingChoice by remember { mutableStateOf(false) }
    MoreButton(
        entries = listOf(
            MenuEntry("Edit songs", R.drawable.ic_edit) { nav.open(Screen.PlaylistEdit(id)) },
            MenuEntry("Add from link", R.drawable.ic_link) { nav.open(Screen.PlaylistImport(id)) },
            MenuEntry("Download missing", R.drawable.ic_download) { missingChoice = true },
            MenuEntry("Remove duplicates", R.drawable.ic_check) {
                act(scope, { val n = features.dedupePlaylist(id); if (n > 0) "Removed $n duplicate(s)" else "No duplicates" }, onChanged)
            },
            MenuEntry("Re-check library", R.drawable.ic_refresh) { act(scope, { "${features.rematchPlaylist(id)} newly matched" }, onChanged) },
            *PlaylistShareRules.shareEntries(shareUrl != null).map { e ->
                when (e) {
                    PlaylistShareRules.ShareEntry.COPY_LINK -> MenuEntry(e.label, R.drawable.ic_content_copy) { copyToClipboard(context, shareUrl.orEmpty()) }
                    PlaylistShareRules.ShareEntry.STOP -> MenuEntry(e.label, R.drawable.ic_close, danger = true) { confirmStop = true }
                }
            }.toTypedArray(),
            if (!isLiked) MenuSeparator else null,
            if (!isLiked) MenuEntry("Rename", R.drawable.ic_edit) { rename = true } else null,
            if (!isLiked) MenuEntry("Delete playlist", R.drawable.ic_delete, danger = true) { confirmDelete = true } else null,
        ),
    )
    if (rename) {
        NameDialog("Rename playlist", name, "Save", onDismiss = { rename = false }) { n ->
            if (n.isNotBlank() && n.trim() != name) act(scope, { features.renamePlaylist(id, n.trim()); "Renamed" }, onChanged)
        }
    }
    if (confirmDelete) {
        ConfirmDialog("Delete playlist", "Delete “$name”? Songs stay in the library.", "Delete", danger = true, onDismiss = { confirmDelete = false }) {
            act(scope, { features.deletePlaylist(id); "" }) { nav.back() }
        }
    }
    if (confirmStop) {
        ConfirmDialog(PlaylistShareRules.STOP_TITLE, PlaylistShareRules.STOP_TEXT, PlaylistShareRules.STOP_TITLE, danger = true, onDismiss = { confirmStop = false }) {
            act(scope, { features.stopSharingPlaylist(id); PlaylistShareRules.STOPPED }, onChanged)
        }
    }
    if (missingChoice) {
        AlbumChoiceDialog("Download missing songs", onDismiss = { missingChoice = false }) { mode ->
            act(scope, { features.requestMissing(id, mode).toastText() }, onChanged)
        }
    }
}

/** Reorder and remove songs (the web's table: `data-rm`, drag → here up/down arrows; POST /api/playlists/{id}/move). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlaylistEditScreen(nav: Nav, id: Long) {
    val features = nav.features
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var version by remember { mutableStateOf(0) }
    val loader = rememberLoad("playlist-edit", id, version) { features.playlistPage(id) }
    var items by remember { mutableStateOf<List<PlaylistItem>?>(null) }
    var busy by remember { mutableStateOf(false) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()

    fun move(from: Int, to: Int) {
        val list = items ?: return
        if (to < 0 || to >= list.size || busy) return
        val row = list[from]
        val next = list.toMutableList().apply { removeAt(from); add(to, row) }
        items = next
        busy = true
        scope.launch {
            try {
                withContext(Dispatchers.IO) { features.movePlaylistItem(id, row.id, to) }
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't move the song", "err")
                items = list
            }
            busy = false
        }
    }
    fun remove(item: PlaylistItem) {
        val list = items ?: return
        items = list.filter { it.id != item.id }
        scope.launch {
            try {
                withContext(Dispatchers.IO) { features.removePlaylistItem(id, item.id) }
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't remove the song", "err")
                items = list
            }
        }
    }

    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("Edit playlist", scrollBehavior = scrollBehavior, onBack = nav.back) },
    ) { padding ->
        LoadableContent(loader) { pl: PlaylistPage ->
            val list = items ?: pl.items.also { items = it }
            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                item {
                    Column(Modifier.padding(horizontal = Gutter)) {
                        Text(pl.name, style = MaterialTheme.typography.headlineMedium)
                        Text(
                            "${list.size} tracks · ${list.count { it.trackId > 0 }} in library · ${list.count { it.trackId <= 0 }} missing",
                            style = MaterialTheme.typography.bodyMedium, color = p.muted, modifier = Modifier.padding(top = 2.dp),
                        )
                        Text("Use the arrows to reorder; ✕ removes a song.", style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(top = 6.dp, bottom = 8.dp))
                        if (pl.sourceUrl.isNotEmpty()) {
                            var follow by remember { mutableStateOf(pl.follow) }
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Switch(checked = follow, onCheckedChange = { on -> follow = on; act(scope, { features.setPlaylistFollow(id, on); "" }) })
                                Spacer(Modifier.width(8.dp))
                                Text("Follow source", style = MaterialTheme.typography.bodyMedium)
                                Spacer(Modifier.width(12.dp))
                                SmallButton("Pull from ${PLAYLIST_SOURCES[pl.source] ?: "source"}", iconRes = R.drawable.ic_refresh) {
                                    act(scope, { "${features.refreshPlaylist(id)} new track(s)" }) { items = null; version += 1 }
                                }
                            }
                        }
                    }
                }
                if (list.isEmpty()) {
                    item { EmptyState(R.drawable.ic_note, "Empty playlist", "Add tracks from an album page or from a link.") }
                }
                items(list.size, key = { i -> list[i].id }) { i ->
                    val row = list[i]
                    Column {
                        Row(Modifier.fillMaxWidth().padding(start = Gutter, end = 4.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text((i + 1).toString(), style = MaterialTheme.typography.bodyMedium, color = p.faint, textAlign = TextAlign.End, modifier = Modifier.width(28.dp))
                            Spacer(Modifier.width(12.dp))
                            Column(Modifier.weight(1f)) {
                                Text(row.title, fontWeight = FontWeight.SemiBold, color = if (row.trackId > 0) p.text else p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text(listOf(row.artist, row.album).filter { s -> s.isNotEmpty() }.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Row(Modifier.padding(top = 3.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                    when {
                                        row.trackId > 0 -> {
                                            Badge("✓ in library", kind = "ok")
                                            if (row.verifyStatus == "verified") Badge("verified", kind = "ok")
                                            if (row.verifyStatus == "mismatch") Badge("wrong audio", kind = "err")
                                        }
                                        row.requestStatus.isNotEmpty() -> StatusBadge(row.requestStatus)
                                        else -> Badge("missing")
                                    }
                                }
                            }
                            IconButton(onClick = { move(i, i - 1) }, enabled = i > 0 && !busy) { Icon(painterResource(R.drawable.ic_arrow_upward), contentDescription = "Move up", tint = if (i > 0) p.muted else p.border) }
                            IconButton(onClick = { move(i, i + 1) }, enabled = i < list.size - 1 && !busy) { Icon(painterResource(R.drawable.ic_arrow_downward), contentDescription = "Move down", tint = if (i < list.size - 1) p.muted else p.border) }
                            IconButton(onClick = { remove(row) }) { Icon(painterResource(R.drawable.ic_close), contentDescription = "Remove", tint = EncoreColors.Err.copy(alpha = 0.8f)) }
                        }
                        RowDivider()
                    }
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
}

/**
 * playlists.js `importModal`: a playlist from a link or a pasted list, with a preview; `into` adds the songs to an
 * existing playlist ("Add tracks from a link") instead of making a new one; [startUrl] fills the link in.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun PlaylistImportScreen(nav: Nav, into: Long?, startUrl: String = "", startRequestMissing: Boolean = false, startAlbumMode: String? = null) {
    val features = nav.features
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var mode by remember { mutableStateOf(0) }
    // Settings → Connections → Import Liked Songs opens this with the service's liked-songs link filled in
    var url by remember { mutableStateOf(startUrl) }
    var text by remember { mutableStateOf("") }
    var name by remember { mutableStateOf("") }
    var follow by remember { mutableStateOf(false) }
    val albumModes = listOf("" to "Albums: admin default", "song" to "Just the songs", "album" to "Whole albums")
    // the Requests link box's "Import as playlist" (the web Downloader's): missing songs requested, its album choice
    var requestMissing by remember { mutableStateOf(startRequestMissing) }
    var albumMode by remember { mutableStateOf(albumModes.indexOfFirst { it.first == startAlbumMode }.coerceAtLeast(0)) }
    var preview by remember { mutableStateOf<ImportPreview?>(null) }
    var previewError by remember { mutableStateOf<String?>(null) }
    var previewing by remember { mutableStateOf(false) }
    var importing by remember { mutableStateOf(false) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()

    fun bodyUrl(): String? = if (mode == 0) url.trim() else null
    fun bodyText(): String? = if (mode == 1) text else null

    fun doPreview() {
        previewing = true
        previewError = null
        scope.launch {
            try {
                val pv = withContext(Dispatchers.IO) { features.importPreview(bodyUrl(), bodyText()) }
                preview = pv
                if (into == null && name.isEmpty()) name = pv.name
            } catch (e: Exception) {
                preview = null
                previewError = e.message ?: "Couldn't read that"
            }
            previewing = false
        }
    }
    // from the Requests link box the link is previewed at once, as the web Downloader does
    LaunchedEffect(Unit) { if (startRequestMissing && startUrl.isNotBlank()) doPreview() }
    fun doImport() {
        importing = true
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) {
                    features.importPlaylist(bodyUrl(), bodyText(), if (into == null) name else null, follow && mode == 0 && into == null, into, requestMissing, albumModes[albumMode].first.ifEmpty { null })
                }
                Toasts.show(
                    "Imported ${r.added} track(s)" + (if (r.duplicatesSkipped > 0) ", skipped ${r.duplicatesSkipped} duplicate(s)" else "") + (if (r.requested > 0) ", requested ${r.requested}" else ""),
                    "ok",
                )
                nav.back()
                if (into == null && r.id > 0L) nav.open(Screen.Playlist(r.id, name.ifEmpty { preview?.name ?: "Imported playlist" }))
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Import failed", "err")
                importing = false
            }
        }
    }

    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar(if (into != null) "Add tracks from a link" else "Import playlist", scrollBehavior = scrollBehavior, onBack = nav.back) },
    ) { padding ->
        LazyColumn(Modifier.fillMaxSize().padding(padding)) {
            item { ChoiceRow(listOf("From a link", "Paste a list"), mode, onSelect = { mode = it }) }
            item {
                Column(Modifier.padding(horizontal = Gutter)) {
                    if (mode == 0) {
                        EncoreField(url, { url = it }, placeholder = "YouTube Music, Spotify, Deezer, Tidal, Apple Music, Amazon Music, Qobuz, YouTube (Music), SoundCloud, Bandcamp, ListenBrainz…", leadingIcon = R.drawable.ic_link, keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go, onAction = { doPreview() }, modifier = Modifier.fillMaxWidth())
                        Text(
                            "Connect Spotify/Tidal on the Connections page to import private playlists and Liked Songs (e.g. https://music.youtube.com/playlist?list=LM or https://open.spotify.com/collection/tracks). Amazon Music playlists and albums import from their public share links.",
                            style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(top = 6.dp),
                        )
                    } else {
                        EncoreField(text, { text = it }, placeholder = "One per line:  Artist - Title", singleLine = false, minLines = 6, imeAction = ImeAction.Default, modifier = Modifier.fillMaxWidth())
                    }
                }
            }
            item {
                Column(Modifier.padding(horizontal = Gutter, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (into == null) {
                        EncoreField(name, { name = it }, placeholder = "Playlist name (optional)", modifier = Modifier.fillMaxWidth())
                        if (mode == 0) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Switch(checked = follow, onCheckedChange = { follow = it })
                                Spacer(Modifier.width(8.dp)); Text("Keep following (pull new tracks)")
                            }
                        }
                    }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Switch(checked = requestMissing, onCheckedChange = { requestMissing = it })
                        Spacer(Modifier.width(8.dp)); Text("Download missing tracks")
                    }
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        albumModes.forEachIndexed { i, (_, label) -> Badge(label, kind = if (i == albumMode) "accent" else "", onClick = { albumMode = i }) }
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        EncoreButton("Preview", iconRes = R.drawable.ic_search, enabled = !previewing && !importing) { doPreview() }
                        EncoreButton(if (importing) "Importing…" else "Import", iconRes = R.drawable.ic_download, primary = true, enabled = !importing && (if (mode == 0) url.isNotBlank() else text.isNotBlank())) { doImport() }
                    }
                }
            }
            val pv = preview
            val pe = previewError
            when {
                previewing -> item { Row(Modifier.padding(Gutter)) { Spinner(20.dp) } }
                pe != null -> item { Box(Modifier.padding(horizontal = Gutter)) { CardBox { Text(pe, color = EncoreColors.Err, style = MaterialTheme.typography.bodySmall) } } }
                pv != null -> {
                    item {
                        FlowRow(Modifier.padding(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text(pv.name, fontWeight = FontWeight.Bold, modifier = Modifier.align(Alignment.CenterVertically))
                            Badge(PLAYLIST_SOURCES[pv.source] ?: pv.source)
                            Badge("${pv.count} tracks")
                            Badge("${pv.inLibrary} already in library", kind = "ok")
                            if (pv.duplicates > 0) Badge("${pv.duplicates} duplicate(s) will be skipped", kind = "warn")
                        }
                        if (pv.warning.isNotEmpty()) Text(pv.warning, color = EncoreColors.Err, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(horizontal = Gutter, vertical = 6.dp))
                    }
                    items(pv.tracks.size, key = { i -> "pv-$i" }) { i ->
                        val t = pv.tracks[i]
                        Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 5.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                "${t.artist} – ${t.title}" + if (t.album.isNotEmpty()) " · ${t.album}" else "",
                                style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
                            )
                            if (t.hasIsrc) Badge("ISRC", modifier = Modifier.padding(start = 6.dp))
                            Badge(if (t.inLibrary) "in library" else "missing", kind = if (t.inLibrary) "ok" else "", modifier = Modifier.padding(start = 6.dp))
                        }
                    }
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }
}
