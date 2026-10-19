package app.encore.android.features.offline

import android.Manifest
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Track
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.formatSize
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.PlayButtons
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

const val EMPTY_TITLE = "Nothing downloaded yet"
const val EMPTY_TEXT = "Use ⋯ → Download for offline on a song, an album or a playlist."
const val KEPT_NOTE = "Downloads are kept in Music/Encore and stay when the app is removed"
const val PREFER_LOCAL = "Play downloaded songs from this device"
const val PREFER_LOCAL_HELP = "Saves data when a song is already downloaded here."

// ---- the offline banner ----------------------------------------------------------------------------------------

/** "Offline – the server can't be reached. Downloaded songs play." with Retry, above the app while offline. */
@Composable
fun OfflineBanner() {
    val active by OfflineMode.active.collectAsState()
    if (!active) return
    val trying by OfflineMode.trying.collectAsState()
    val context = LocalContext.current
    val p = MaterialTheme.palette
    Row(
        Modifier.fillMaxWidth().background(EncoreColors.Warn.copy(alpha = 0.16f)).padding(horizontal = Gutter, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(R.drawable.ic_cloud_off), contentDescription = null, tint = p.text, modifier = Modifier.size(18.dp))
        Spacer(Modifier.width(10.dp))
        Text(OfflineMode.BANNER, style = MaterialTheme.typography.bodySmall, color = p.text, modifier = Modifier.weight(1f))
        Spacer(Modifier.width(8.dp))
        SmallButton(if (trying) "Trying…" else "Retry", enabled = !trying) { OfflineMode.retry(context) }
    }
}

// ---- "Download for offline" in the menus -------------------------------------------------------------------

/**
 * The download entries of a song, album or playlist menu. On Android 8–9 the first download asks for the storage
 * permission (Android 10 and later need none for the app's own files in Music/).
 */
class OfflineMenu internal constructor(
    private val nav: Nav,
    private val store: OfflineStore,
    private val offline: Boolean,
    private val withStorage: (() -> Unit) -> Unit,
) {
    /** The signed-in server's saved songs by id (a menu is made again whenever the index changes). */
    private val byId: Map<Long, OfflineRecord> by lazy { store.songs().associateBy { it.id } }

    /** "Download for offline" (some of these songs are not saved yet; not offline) and "Remove download" (some are). */
    fun entries(tracks: List<Track>): List<MenuEntry> {
        val songs = tracks.filter { it.id > 0L }.distinctBy { it.id }
        if (songs.isEmpty()) return emptyList()
        val saved = songs.mapNotNull { byId[it.id] }
        val out = ArrayList<MenuEntry>(2)
        if (saved.size < songs.size && !offline) out.add(MenuEntry("Download for offline", R.drawable.ic_download) { download(songs) })
        if (saved.isNotEmpty()) out.add(MenuEntry("Remove download", R.drawable.ic_delete) { remove(saved) })
        return out
    }

    fun download(tracks: List<Track>) = withStorage { store.worker.enqueue(tracks) }

    /** "Download again in original quality" for converted copies. */
    fun downloadAgain(records: List<OfflineRecord>) = withStorage { store.worker.enqueue(records.map { OfflineStore.trackOf(it) }, again = true) }

    fun remove(records: List<OfflineRecord>) {
        nav.graph.scope.launch {
            val n = withContext(Dispatchers.IO) { store.remove(records) }
            if (n > 0) Toasts.show(if (n == 1) "Download removed" else "$n downloads removed", "ok")
        }
    }
}

@Composable
fun rememberOfflineMenu(nav: Nav): OfflineMenu {
    val context = LocalContext.current
    val store = remember(context) { OfflineStore.of(context) }
    val all by store.all.collectAsState()
    val offline by OfflineMode.active.collectAsState()
    val waiting = remember { mutableStateOf<(() -> Unit)?>(null) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        val go = waiting.value
        waiting.value = null
        if (granted) go?.invoke() else Toasts.show(OfflineWorker.NO_PERMISSION, "err")
    }
    return remember(all, offline, nav) {
        OfflineMenu(nav, store, offline) { action ->
            if (store.canWrite()) {
                action()
            } else {
                waiting.value = action
                launcher.launch(Manifest.permission.WRITE_EXTERNAL_STORAGE)
            }
        }
    }
}

// ---- the Downloaded list ---------------------------------------------------------------------------------------

/** The Library tab while offline: its title, Shuffle over the saved songs, and the Downloaded list. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OfflineLibraryScreen(nav: Nav) {
    val context = LocalContext.current
    val store = remember(context) { OfflineStore.of(context) }
    val all by store.all.collectAsState()
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = MaterialTheme.palette.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            EncoreTopBar("Library", scrollBehavior = scrollBehavior, actions = {
                val songs = store.songs(all)
                SmallButton("Shuffle", iconRes = R.drawable.ic_shuffle, primary = true, enabled = songs.isNotEmpty(), modifier = Modifier.padding(end = Gutter)) {
                    nav.player.play(songs.map { OfflineStore.trackOf(it) }, shuffle = true)
                }
            })
        },
    ) { padding ->
        DownloadedList(nav, Modifier.padding(padding))
    }
}

/**
 * The songs saved on this phone (the signed-in server's), by album, newest download first: Play and Shuffle,
 * a search box, and per album and song Play, Play next, Add to queue, Download again in original quality (a
 * converted copy) and Remove download.
 */
@Composable
fun DownloadedList(nav: Nav, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val store = remember(context) { OfflineStore.of(context) }
    val all by store.all.collectAsState()
    val offline by OfflineMode.active.collectAsState()
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val menu = rememberOfflineMenu(nav)
    var query by remember { mutableStateOf("") }
    val songs = remember(all) { store.songs(all) }
    val albums = remember(songs, query) { OfflineIndex.group(songs, query) }
    // the plays and likes still waiting for the server (the web's outbox note), read again when the outbox changes
    val outboxChanges by store.outbox.changes.collectAsState()
    var waiting by remember { mutableStateOf("") }
    LaunchedEffect(outboxChanges, all) {
        val (plays, likes) = withContext(Dispatchers.IO) { OfflineMode.pending() }
        waiting = Outbox.countsText(plays, likes)
    }
    val p = MaterialTheme.palette
    if (songs.isEmpty()) {
        EmptyState(R.drawable.ic_download, EMPTY_TITLE, EMPTY_TEXT, modifier)
        return
    }
    LazyColumn(modifier.fillMaxSize()) {
        item(key = "head") {
            Column {
                Text(
                    "${OfflineIndex.songs(songs.size)} · ${formatSize(store.usedBytes(songs))}",
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    modifier = Modifier.padding(start = Gutter, end = Gutter, top = 8.dp),
                )
                if (waiting.isNotEmpty()) {
                    Row(Modifier.padding(start = Gutter, end = Gutter, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(painterResource(R.drawable.ic_refresh), contentDescription = null, tint = p.muted, modifier = Modifier.size(14.dp))
                        Spacer(Modifier.width(6.dp))
                        Text("$waiting will be sent when the server is back", style = MaterialTheme.typography.bodySmall, color = p.muted)
                    }
                }
                PlayButtons(nav, albums.flatMap { a -> a.songs.map { OfflineStore.trackOf(it) } })
                // as the web: the search shows from two saved songs
                if (songs.size >= 2) {
                    OutlinedTextField(
                        value = query,
                        onValueChange = { query = it },
                        placeholder = { Text("Search downloads") },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 4.dp),
                    )
                }
            }
        }
        if (albums.isEmpty() && query.isNotBlank()) {
            item(key = "nomatch") {
                Text(
                    "No downloads match “${query.trim()}”.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = p.muted,
                    modifier = Modifier.padding(horizontal = Gutter, vertical = 12.dp),
                )
            }
        }
        for (a in albums) {
            item(key = "album:" + a.key) { AlbumHead(nav, store, menu, a, offline) }
            items(a.songs, key = { "song:" + it.sourceServer + ":" + it.id }) { r ->
                SongRow(nav, menu, a, r, liked = r.id in liked, playing = nowPlaying?.id == r.id, offline = offline)
            }
        }
        item(key = "foot") { Spacer(Modifier.height(24.dp)) }
    }
}

/** The cover of a saved album: the copy saved with it, else the server's. */
private fun coverOf(nav: Nav, store: OfflineStore, albumId: String, size: Int): String? {
    store.coverFile(albumId)?.let { return Uri.fromFile(it).toString() }
    return if (albumId.isNotEmpty()) nav.graph.api.coverUrl(albumId, size) else null
}

@Composable
private fun AlbumHead(nav: Nav, store: OfflineStore, menu: OfflineMenu, a: OfflineAlbum, offline: Boolean) {
    val p = MaterialTheme.palette
    val tracks = remember(a) { a.songs.map { OfflineStore.trackOf(it) } }
    val cover = remember(a.albumId, a.songs.size) { coverOf(nav, store, a.albumId, 200) }
    Row(
        Modifier
            .fillMaxWidth()
            .clickable { if (!offline && a.albumId.isNotEmpty()) nav.open(Screen.Album(a.albumId)) else nav.player.play(tracks) }
            .padding(start = Gutter, end = 4.dp, top = 16.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Cover(cover, nav.graph.imageLoader, size = 56.dp, corner = 8.dp, label = a.album.ifEmpty { "?" })
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(a.album.ifEmpty { "Unknown album" }, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                listOf(a.artist, OfflineIndex.songs(a.songs.size), formatSize(a.bytes)).filter { it.isNotEmpty() }.joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        IconButton(onClick = { nav.player.play(tracks) }) {
            Icon(painterResource(R.drawable.ic_play), contentDescription = "Play", tint = p.text)
        }
        val converted = a.songs.filter { it.converted }
        MoreButton(
            entries = listOf(
                MenuEntry("Play", R.drawable.ic_play) { nav.player.play(tracks) },
                MenuEntry("Play next", R.drawable.ic_skip_next) { tracks.asReversed().forEach { nav.player.playNext(it) }; Toasts.show("Plays next", "ok") },
                MenuEntry("Add to queue", R.drawable.ic_queue_music) { tracks.forEach { nav.player.addToQueue(it) }; Toasts.show("Added to queue", "ok") },
                if (converted.isNotEmpty() && !offline) MenuEntry("Download again in original quality", R.drawable.ic_download) { menu.downloadAgain(converted) } else null,
                MenuEntry("Remove download", R.drawable.ic_delete, danger = true) { menu.remove(a.songs) },
            ),
        )
    }
}

@Composable
private fun SongRow(nav: Nav, menu: OfflineMenu, a: OfflineAlbum, r: OfflineRecord, liked: Boolean, playing: Boolean, offline: Boolean) {
    val t = remember(r) { OfflineStore.trackOf(r) }
    var open by remember { mutableStateOf(false) }
    // the web's quality badge: "Audio" when the record has none, in the warn colour for a converted copy
    val subtitle = remember(r, a.artist) {
        buildAnnotatedString {
            if (r.artist.isNotEmpty() && r.artist != a.artist) append("${r.artist} · ")
            if (r.converted) withStyle(SpanStyle(color = EncoreColors.Warn)) { append(r.quality.ifBlank { "Audio" }) }
            else append(r.quality.ifBlank { "Audio" })
        }
    }
    Box {
        TrackRow(
            track = t,
            imageLoader = null,
            coverUrl = null,
            liked = liked,
            playing = playing,
            onClick = {
                val list = a.songs.map { OfflineStore.trackOf(it) }
                nav.player.play(list, a.songs.indexOf(r).coerceAtLeast(0))
            },
            onLike = { nav.graph.likes.toggle(r.id) },
            number = r.track,
            subtitleLinks = subtitle,
            onMore = { open = true },
        )
        PopMenu(
            open,
            { open = false },
            listOf(
                MenuEntry("Play", R.drawable.ic_play) { nav.player.play(a.songs.map { OfflineStore.trackOf(it) }, a.songs.indexOf(r).coerceAtLeast(0)) },
                MenuEntry("Play next", R.drawable.ic_skip_next) { nav.player.playNext(t); Toasts.show("Plays next", "ok") },
                MenuEntry("Add to queue", R.drawable.ic_queue_music) { nav.player.addToQueue(t); Toasts.show("Added to queue", "ok") },
                if (r.converted && !offline) MenuEntry("Download again in original quality", R.drawable.ic_download) { menu.downloadAgain(listOf(r)) } else null,
                MenuEntry("Remove download", R.drawable.ic_delete, danger = true) { menu.remove(listOf(r)) },
            ),
        )
    }
}

// ---- Settings → This device ---------------------------------------------------------------------------------

/**
 * Settings → This device → Downloads: the room they take, what is downloading, each album with its size and a
 * remove button, Remove all, and where the files are.
 */
@Composable
fun OfflineStorageSection(nav: Nav) {
    val context = LocalContext.current
    val store = remember(context) { OfflineStore.of(context) }
    val all by store.all.collectAsState()
    val progress by store.worker.progress.collectAsState()
    val menu = rememberOfflineMenu(nav)
    var free by remember { mutableStateOf(-1L) }
    var confirmAll by remember { mutableStateOf(false) }
    LaunchedEffect(all.size) { free = withContext(Dispatchers.IO) { store.freeBytes() } }
    val p = MaterialTheme.palette
    val albums = remember(all) { OfflineIndex.group(all) }
    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp)) {
        Text("Downloads", style = MaterialTheme.typography.bodyLarge)
        Text(
            listOf(
                formatSize(store.usedBytes(all)),
                OfflineIndex.songs(all.size),
                if (free >= 0) "${formatSize(free)} free" else "",
            ).filter { it.isNotEmpty() }.joinToString(" · "),
            style = MaterialTheme.typography.bodySmall,
            color = p.muted,
        )
        Text(KEPT_NOTE, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(top = 2.dp))
        // the web's Settings → Playback → Downloads switch; off, downloaded songs stream while the server answers
        val preferLocal by store.preferLocal.collectAsState()
        Row(
            Modifier.fillMaxWidth().padding(top = 6.dp).clickable { store.setPreferLocal(!preferLocal) },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Checkbox(checked = preferLocal, onCheckedChange = { store.setPreferLocal(it) })
            Text(PREFER_LOCAL, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
        }
        Text(PREFER_LOCAL_HELP, style = MaterialTheme.typography.bodySmall, color = p.muted)
        progress?.let { pr ->
            Text(OfflineIndex.progressText(pr.done, pr.total, pr.current), style = MaterialTheme.typography.bodySmall, color = p.accent, modifier = Modifier.padding(top = 6.dp))
        }
        if (albums.isEmpty()) {
            Text(EMPTY_TEXT, style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(top = 6.dp))
        }
        for (a in albums) {
            Row(Modifier.fillMaxWidth().padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(a.album.ifEmpty { "Unknown album" }, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        listOf(a.artist, OfflineIndex.songs(a.songs.size), formatSize(a.bytes)).filter { it.isNotEmpty() }.joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                        color = p.muted,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                IconButton(onClick = { menu.remove(a.songs) }) {
                    Icon(painterResource(R.drawable.ic_delete), contentDescription = "Remove download", tint = p.muted)
                }
            }
        }
        if (all.isNotEmpty()) {
            Row(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                EncoreButton("Remove all", iconRes = R.drawable.ic_delete) { confirmAll = true }
            }
        }
    }
    if (confirmAll) {
        ConfirmDialog(
            "Remove all downloads",
            "Remove the ${OfflineIndex.songs(all.size)} saved on this device? They stay in the library on the server.",
            "Remove all",
            danger = true,
            onDismiss = { confirmAll = false },
        ) {
            nav.graph.scope.launch {
                withContext(Dispatchers.IO) { store.removeAll() }
                Toasts.show("All downloads removed", "ok")
            }
        }
    }
}
