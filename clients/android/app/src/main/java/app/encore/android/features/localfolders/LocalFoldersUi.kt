package app.encore.android.features.localfolders

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.media3.common.MediaItem
import app.encore.android.R
import app.encore.android.data.api.Track
import app.encore.android.playback.MediaItems
import app.encore.android.ui.Nav
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Most songs the tab lists (Play all and Shuffle take them all). */
private const val SHOWN = 500

/**
 * Library → On this device: the folders picked on this phone (add with the system folder picker, remove, scan
 * again) and their songs that the library does not have. Songs the library has show in the library itself and
 * play from the phone's file there. Play all and Shuffle play every song of the folders.
 */
@Composable
fun LocalFoldersTab(nav: Nav) {
    val context = LocalContext.current
    val store = remember(context) { LocalFolders.of(context) }
    val changes by store.changes.collectAsState()
    val scanning by store.scanning.collectAsState()
    val scope = rememberCoroutineScope()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val api = nav.graph.api
    val server = api.base
    val p = MaterialTheme.palette

    fun rescan() {
        scope.launch {
            withContext(Dispatchers.IO) { store.scan() }
        }
    }

    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        if (store.addFolder(uri)) {
            Toasts.show("Folder added – looking for music…", "ok")
            rescan()
        } else {
            Toasts.show("Android didn't allow reading that folder", "err")
        }
    }

    // a scan when the tab first opens with folders but no songs yet; then the server's answers when they are older
    LaunchedEffect(Unit) {
        if (store.folders.isNotEmpty() && store.songs.isEmpty()) rescan()
    }
    LaunchedEffect(changes, scanning, server) {
        if (!scanning && store.needsMatch(server)) {
            try {
                withContext(Dispatchers.IO) { store.match(api) }
            } catch (e: Exception) {
                Toasts.show("Couldn't compare your folders with the library: ${e.message ?: "no answer"}", "err")
            }
        }
    }

    val matches = remember(changes, server) { store.matches(server) }
    val songs = remember(changes) { store.songs }
    val rest = remember(songs, matches) { songs.filter { it.uri !in matches } }
    val items: () -> List<MediaItem> = {
        songs.map { s -> matches[s.uri]?.let { Track.from(it) }?.let { MediaItems.track(api, it) } ?: MediaItems.local(s) }
    }

    LazyColumn(Modifier.fillMaxSize()) {
        item(key = "head") {
            Column(Modifier.padding(horizontal = Gutter, vertical = 8.dp)) {
                Text(
                    summary(rest.size, songs.size - rest.size, scanning),
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                )
                Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    SmallButton("Play all", iconRes = R.drawable.ic_play, primary = true, enabled = songs.isNotEmpty()) {
                        nav.player.playLocal(items())
                    }
                    SmallButton("Shuffle", iconRes = R.drawable.ic_shuffle, enabled = songs.isNotEmpty()) {
                        nav.player.playLocal(items(), shuffle = true)
                    }
                    SmallButton("Add folder", iconRes = R.drawable.ic_create_folder) { pick.launch(null) }
                    SmallButton("Scan", iconRes = R.drawable.ic_refresh, enabled = store.folders.isNotEmpty() && !scanning) { rescan() }
                }
            }
        }
        if (store.folders.isEmpty()) {
            item(key = "empty") {
                EmptyState(
                    R.drawable.ic_folder,
                    "No folders yet",
                    "Add a folder of music on this phone to play it here. Songs your library already has play from your files; nothing is uploaded.",
                )
            }
        }
        items(store.folders, key = { "folder:$it" }) { f ->
            Row(Modifier.fillMaxWidth().padding(start = Gutter, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(painterResource(R.drawable.ic_folder), contentDescription = null, tint = p.muted)
                Text(
                    store.label(f),
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f).padding(start = 12.dp),
                )
                IconButton(onClick = { store.removeFolder(f); rescan() }) {
                    Icon(painterResource(R.drawable.ic_close), contentDescription = "Remove this folder (its files stay)", tint = p.muted)
                }
            }
        }
        items(rest.take(SHOWN), key = { "song:" + it.uri }) { s ->
            val t = s.asTrack()
            TrackRow(
                track = t,
                imageLoader = null,
                coverUrl = null,
                liked = false,
                playing = nowPlaying?.id == t.id,
                onClick = { nav.player.playLocal(rest.map { MediaItems.local(it) }, rest.indexOf(s)) },
                onLike = null,
                number = t.trackNo,
                subtitle = listOf(t.artist, t.album).filter { it.isNotEmpty() }.joinToString(" · "),
                note = s.format.uppercase(),
            )
        }
        if (rest.size > SHOWN) {
            item(key = "more") {
                Text(
                    "and ${rest.size - SHOWN} more – Play all and Shuffle play them too",
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    modifier = Modifier.padding(horizontal = Gutter, vertical = 8.dp),
                )
            }
        }
        item(key = "foot") { Spacer(Modifier.height(24.dp)) }
    }
}

/** "12 songs · 30 already in your library, played from these files". */
fun summary(notInLibrary: Int, inLibrary: Int, scanning: Boolean): String {
    if (scanning) return "Looking for music…"
    val total = notInLibrary + inLibrary
    if (total == 0) return "No music found in these folders yet."
    val songs = if (total == 1) "1 song" else "$total songs"
    return if (inLibrary > 0) "$songs · $inLibrary already in your library, played from these files" else songs
}
