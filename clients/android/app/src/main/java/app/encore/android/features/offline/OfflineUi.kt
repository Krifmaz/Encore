package app.encore.android.features.offline

import android.Manifest
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.rememberModalBottomSheetState
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.Tile
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.draw.clip
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

/**
 * "Offline · Downloaded songs play · tap for details" with Retry, above the app while offline.
 *
 * It is the Scaffold's top bar, which in an edge-to-edge window (MainActivity's `enableEdgeToEdge`) owns the status
 * bar inset: the warn tint runs under the status bar, and the text and Retry sit below it (they used to draw under
 * the clock and icons, where the system also takes every touch). Two short lines instead of one long wrapped
 * sentence, a 48 dp Retry, and the whole banner opens [OfflineSheet] with the full message, why the server can't be
 * reached, and Retry.
 */
@Composable
fun OfflineBanner() {
    val active by OfflineMode.active.collectAsState()
    if (!active) return
    val trying by OfflineMode.trying.collectAsState()
    val context = LocalContext.current
    val p = MaterialTheme.palette
    // the sheet with the full message, opened by a tap anywhere on the banner but Retry
    var details by remember { mutableStateOf(false) }
    Row(
        Modifier
            .fillMaxWidth()
            .background(EncoreColors.Warn.copy(alpha = 0.16f))
            // after the background: the tint fills the status bar, the content stays below it
            .windowInsetsPadding(WindowInsets.statusBars)
            .clickable(onClickLabel = "Offline details") { details = true }
            .heightIn(min = 48.dp)
            .padding(start = Gutter, end = 8.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(R.drawable.ic_cloud_off), contentDescription = null, tint = p.text, modifier = Modifier.size(20.dp))
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(OfflineMode.BANNER_TITLE, style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold, color = p.text, maxLines = 1)
            Text(OfflineMode.BANNER_LINE, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Spacer(Modifier.width(8.dp))
        RetryButton(trying) { OfflineMode.retry(context) }
    }
    if (details) OfflineSheet(onDismiss = { details = false })
}

/**
 * The banner's and the sheet's Retry: a 48 dp touch target around the small button (Material's minimum), reading
 * "Trying…" and not tappable while a check is on its way.
 */
@Composable
private fun RetryButton(trying: Boolean, onRetry: () -> Unit) {
    Box(Modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp), contentAlignment = Alignment.Center) {
        SmallButton(if (trying) "Trying…" else "Retry", iconRes = R.drawable.ic_refresh, enabled = !trying, onClick = onRetry)
    }
}

/**
 * The sheet the offline banner opens: the full offline message, why the last check failed ([OfflineMode.reason]),
 * how many songs are saved here and how many plays and likes wait, Retry, and Close. Closes by itself once the server
 * answers again.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OfflineSheet(onDismiss: () -> Unit) {
    val context = LocalContext.current
    val active by OfflineMode.active.collectAsState()
    val trying by OfflineMode.trying.collectAsState()
    val reason by OfflineMode.reason.collectAsState()
    val store = remember(context) { OfflineStore.of(context) }
    val all by store.all.collectAsState()
    // the plays and likes still waiting, read off the main thread when the sheet opens or the downloads change
    var waiting by remember { mutableStateOf("") }
    LaunchedEffect(all) {
        val (plays, likes) = withContext(Dispatchers.IO) { OfflineMode.pending() }
        waiting = Outbox.countsText(plays, likes)
    }
    // back online: nothing left to say
    LaunchedEffect(active) { if (!active) onDismiss() }
    val p = MaterialTheme.palette
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, bottom = 24.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(painterResource(R.drawable.ic_cloud_off), contentDescription = null, tint = EncoreColors.Warn, modifier = Modifier.size(22.dp))
                Spacer(Modifier.width(10.dp))
                Text("You're offline", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            }
            Text(OfflineMode.BANNER, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(top = 12.dp))
            Text("Why: $reason", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(top = 8.dp))
            Text(
                listOf(
                    "${OfflineIndex.songs(store.songs(all).size)} saved on this phone",
                    if (waiting.isNotEmpty()) "$waiting will be sent when the server is back" else "",
                ).filter { it.isNotEmpty() }.joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
                modifier = Modifier.padding(top = 4.dp),
            )
            Text("The app asks the server again every 30 seconds.", style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(top = 4.dp))
            Row(Modifier.padding(top = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                EncoreButton(if (trying) "Trying…" else "Retry", iconRes = R.drawable.ic_refresh, primary = true, enabled = !trying) {
                    OfflineMode.retry(context)
                }
                EncoreButton("Close", onClick = onDismiss)
            }
        }
    }
}

// ---- Home's "Saved on this phone" row --------------------------------------------------------------------------

/**
 * Home's row of the albums saved on this phone, newest download first, shown while offline (the server's sections
 * are then only what it last answered, or nothing): a tap plays the album from its files, "Downloaded" opens the
 * Library tab's Downloaded list. Nothing when no song is saved here.
 *
 * @param onOpenAll opens the full Downloaded list (the Library tab, which opens on Downloaded while offline).
 */
@Composable
fun SavedOnPhoneRow(nav: Nav, onOpenAll: () -> Unit) {
    val context = LocalContext.current
    val store = remember(context) { OfflineStore.of(context) }
    val all by store.all.collectAsState()
    // the newest downloads first, at most a screen's worth of cards (the full list is a tap away)
    val albums = remember(all) { OfflineIndex.group(store.songs(all)).sortedByDescending { it.latest }.take(MAX_SAVED_ON_HOME) }
    if (albums.isEmpty()) return
    SectionHead("Saved on this phone", subtitle = "Plays without the server", action = "Downloaded", onAction = onOpenAll)
    LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        items(albums, key = { it.key }) { a ->
            val tracks = remember(a) { a.songs.map { OfflineStore.trackOf(it) } }
            val cover = remember(a.albumId, a.songs.size) { coverOf(nav, store, a.albumId, 400) }
            Tile(
                title = a.album.ifEmpty { "Unknown album" },
                subtitle = listOf(a.artist, OfflineIndex.songs(a.songs.size)).filter { it.isNotEmpty() }.joinToString(" · "),
                modifier = Modifier.width(150.dp),
                onClick = { nav.player.play(tracks) },
            ) { m ->
                Cover(cover, nav.graph.imageLoader, m, corner = 12.dp, label = a.album.ifEmpty { "?" })
            }
        }
    }
}

/** How many saved albums Home's row shows at most. */
private const val MAX_SAVED_ON_HOME = 20

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

/**
 * Home's "Shuffle library" and the Library's Shuffle: 50 songs from the whole library, or in offline mode the songs
 * saved on this phone (the server can't pick any), shuffled. Says so when there is nothing to play.
 */
suspend fun shuffleLibrary(nav: Nav) {
    if (OfflineMode.active.value) {
        val store = OfflineStore.of(nav.graph.app)
        val saved = withContext(Dispatchers.IO) { store.ensureLoaded(); store.songs().map { OfflineStore.trackOf(it) } }
        if (saved.isEmpty()) Toasts.show(EMPTY_TITLE, "info") else nav.player.play(saved, shuffle = true)
        return
    }
    val songs = try { withContext(Dispatchers.IO) { nav.graph.api.shuffle(50, emptyList()) } } catch (_: Exception) { emptyList() }
    if (songs.isEmpty()) Toasts.show("Your library is empty", "err") else nav.player.play(songs)
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

/**
 * The head of one saved album in the downloaded list: cover, album, artist · songs · size, Play and ⋯. A tap on the
 * cover plays the album; a tap on the rest opens the album page (plays it when offline).
 */
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
        // the cover plays the saved album; the rest of the row opens the album page (or plays, when offline)
        Cover(
            cover, nav.graph.imageLoader,
            modifier = Modifier.clip(RoundedCornerShape(8.dp)).clickable(onClickLabel = "Play") { nav.player.play(tracks) },
            size = 56.dp, corner = 8.dp, label = a.album.ifEmpty { "?" },
        )
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
