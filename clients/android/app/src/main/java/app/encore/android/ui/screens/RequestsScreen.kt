package app.encore.android.ui.screens

import android.content.Context
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.repo.LinkProbe
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.RequestRow
import app.encore.android.features.requests.RequestItem
import app.encore.android.features.requests.RequestRules
import app.encore.android.features.requests.RequestSheets
import app.encore.android.features.requests.StorageBanner
import app.encore.android.features.requests.rememberRequestActions
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.Tab
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MenuSeparator
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

private val FILTERS = listOf("all" to "All", "active" to "Active", "pending" to "Awaiting approval", "completed" to "Available", "failed" to "Failed")

/** "If a song is on an album:" is remembered on this phone, like the web's `encore.albumMode` (default: just the song). */
private object AlbumModePref {
    private const val FILE = "encore_requests"
    private const val KEY = "album_mode"

    fun read(context: Context): String =
        try { context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).getString(KEY, "song") ?: "song" } catch (_: Exception) { "song" }

    fun write(context: Context, mode: String) {
        try { context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putString(KEY, mode).apply() } catch (_: Exception) {}
    }
}

/**
 * Requests (web-rs/src/pages/requests.rs, with the web Downloader's link box and bulk actions, download.rs): the list
 * with status filters, every row's buttons (common.rs), a tap for the whole message and log, a link box with the
 * album choice, Preview, Import as playlist and Exactly this link, Retry failed, Clear… and Clear waiting beside Only
 * mine (as the web shows them), the admin's Download all now (servers with Download now), the storage banner while imports are
 * paused, and for admins Activity in ⋯.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun RequestsScreen(nav: Nav) {
    val features = nav.features
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var me by remember { mutableStateOf<Me?>(null) }
    var filter by remember { mutableStateOf(0) }
    var mine by remember { mutableStateOf(false) }
    var rows by remember { mutableStateOf<List<RequestRow>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var generation by remember { mutableStateOf(0) }
    var link by remember { mutableStateOf("") }
    var linkBusy by remember { mutableStateOf(false) }
    var albumMode by remember { mutableStateOf(AlbumModePref.read(context)) }
    var probe by remember { mutableStateOf<LinkProbe?>(null) }
    var probeError by remember { mutableStateOf<String?>(null) }
    var probing by remember { mutableStateOf(false) }
    var confirmClear by remember { mutableStateOf<Pair<List<String>, String>?>(null) }
    var clearMenu by remember { mutableStateOf(false) }
    // Retry failed on its way (its button waits)
    var retrying by remember { mutableStateOf(false) }
    // Download all now on its way (its button waits)
    var downloadingAll by remember { mutableStateOf(false) }
    // Clear waiting: counting (its button waits), then "Cancel N waiting requests?"
    var countingWaiting by remember { mutableStateOf(false) }
    var confirmWaiting by remember { mutableStateOf<Int?>(null) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    fun reload() { generation += 1 }
    val actions = rememberRequestActions(nav) { reload() }

    suspend fun load() {
        try {
            val m = me ?: withContext(Dispatchers.IO) { features.me() }.also { me = it; actions.me = it; if (!it.isAdmin && !mine) mine = true }
            rows = withContext(Dispatchers.IO) { features.requests(FILTERS[filter].first, mine && m.isAdmin) }
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (rows == null) error = e.message ?: "Couldn't load the requests"
        }
    }

    LaunchedEffect(filter, mine, generation) { load() }
    // the web refreshes every 4 s while the page is open
    LaunchedEffect(Unit) {
        while (true) {
            delay(4000)
            load()
        }
    }

    fun clear(statuses: List<String>, files: Boolean) {
        scope.launch {
            try {
                val (removed, deleted) = withContext(Dispatchers.IO) { features.clearRequests(statuses, files, mine) }
                Toasts.show(RequestRules.clearToast(removed, deleted, files), "ok")
                reload()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }
    // Clear waiting (Oct 21: hundreds of requests still waiting): the server counts first (a dry run), then asks
    fun clearWaiting() {
        if (countingWaiting) return
        countingWaiting = true
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { features.clearWaiting(mine, dryRun = true) }
                if (r.count > 0) confirmWaiting = r.count else Toasts.show(RequestRules.clearWaitingToast(r), "info")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
            countingWaiting = false
        }
    }
    fun clearWaitingNow() {
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { features.clearWaiting(mine, dryRun = false) }
                Toasts.show(RequestRules.clearWaitingToast(r), if (r.count > 0) "ok" else "info")
                reload()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }
    fun retryFailed() {
        if (retrying) return
        retrying = true
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { features.retryFailed(mine) }
                val (msg, kind) = RequestRules.retryFailedToast(r)
                Toasts.show(msg, kind)
                reload()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
            retrying = false
        }
    }
    // Download all now (admins): wake every waiting request and move requests up in their lines (with Only mine, the
    // admin's own first) – never more at once than each source allows; the server's words are the toast
    fun downloadAllNow() {
        if (downloadingAll) return
        downloadingAll = true
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { features.downloadAllNow(mine) }
                val (msg, kind) = RequestRules.downloadAllNowToast(r)
                Toasts.show(msg, kind)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
            downloadingAll = false
            reload()
        }
    }
    fun importAsPlaylist() {
        val url = link.trim()
        if (url.isEmpty()) return
        nav.open(Screen.PlaylistImport(null, url, requestMissing = true, albumMode = albumMode))
    }
    // Download (the web Downloader's): matched song by song, or downloaded directly
    fun requestLink() {
        val url = link.trim()
        if (url.isEmpty()) return
        linkBusy = true
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { features.requestLinkWithMode(url, albumMode) }
                Toasts.show(RequestRules.linkToast(r, albumMode), "ok")
                link = ""
                probe = null
                probeError = null
                reload()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't request that link", "err")
            }
            linkBusy = false
        }
    }
    fun preview() {
        val url = link.trim()
        if (url.isEmpty()) return
        if (RequestRules.previewImports(url)) { importAsPlaylist(); return }
        probing = true
        probe = null
        probeError = null
        scope.launch {
            try {
                probe = withContext(Dispatchers.IO) { features.probeLink(url) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                probeError = e.message ?: "That didn't work"
            }
            probing = false
        }
    }
    // Exactly this link: skip matching, download what the link points at
    fun exactly() {
        val url = link.trim()
        if (url.isEmpty()) return
        if (RequestRules.metadataOnly(url)) { importAsPlaylist(); return }
        linkBusy = true
        val title = probe?.title?.ifEmpty { null }
        scope.launch {
            try {
                val body = buildJsonObject {
                    put("kind", "url")
                    put("url", url)
                    if (title != null) put("title", title) else put("title", JsonNull)
                }
                val status = withContext(Dispatchers.IO) { features.createRequest(body) }
                Toasts.show(if (status == "pending") "Request sent – waiting for approval" else "Request queued", "ok")
                link = ""
                probe = null
                probeError = null
                reload()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
            linkBusy = false
        }
    }

    val m = me
    val admin = m?.isAdmin == true
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            EncoreTopBar("Requests", scrollBehavior = scrollBehavior, onBack = nav.back, actions = {
                if (admin) {
                    // the web's Activity (Soulseek transfers, torrents, the log), under its Downloader
                    MoreButton(entries = listOf(MenuEntry("Activity", R.drawable.ic_signal) { nav.open(Screen.Activity) }))
                }
            })
        },
    ) { padding ->
        val list = rows
        val err = error
        LazyColumn(Modifier.fillMaxSize().padding(padding)) {
            // quota, the admin's "Only mine", and the web's Retry failed and Clear buttons (members' Clear entries
            // only clear the list; the admin's also delete files, after a confirm)
            item {
                Column(Modifier.padding(horizontal = Gutter)) {
                    if (m != null && m.requestLimit > 0 && !m.isAdmin) {
                        Text("${m.requestsThisWeek} of ${m.requestLimit} requests used this week", style = MaterialTheme.typography.bodySmall, color = p.muted)
                    }
                    if (m != null) {
                        FlowRow(
                            Modifier.fillMaxWidth().padding(vertical = 4.dp),
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                            verticalArrangement = Arrangement.spacedBy(4.dp),
                        ) {
                            if (admin) {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Switch(checked = mine, onCheckedChange = { mine = it })
                                    Spacer(Modifier.width(8.dp))
                                    Text("Only mine", style = MaterialTheme.typography.bodyMedium)
                                    Spacer(Modifier.width(8.dp))
                                }
                            }
                            // only for those who may retry (Fix their own downloads), as the rows' Retry
                            if (RequestRules.mayFix(m)) {
                                SmallButton("Retry failed", iconRes = R.drawable.ic_refresh, enabled = !retrying, modifier = Modifier.align(Alignment.CenterVertically)) { retryFailed() }
                            }
                            // admins, on a server with Download now (its rows say whether it offers the button)
                            if (admin && RequestRules.hasDownloadNow(rows.orEmpty())) {
                                SmallButton("Download all now", iconRes = R.drawable.ic_download, enabled = !downloadingAll, modifier = Modifier.align(Alignment.CenterVertically)) { downloadAllNow() }
                            }
                            Box(Modifier.align(Alignment.CenterVertically)) {
                                SmallButton("Clear…", iconRes = R.drawable.ic_delete) { clearMenu = true }
                                PopMenu(
                                    clearMenu, { clearMenu = false },
                                    listOf(
                                        MenuEntry("Clear completed", R.drawable.ic_check) { clear(listOf("completed"), false) },
                                        MenuEntry("Clear failed & cancelled", R.drawable.ic_close) { clear(listOf("failed", "cancelled", "denied"), false) },
                                        if (admin) MenuSeparator else null,
                                        if (admin) MenuEntry("Clear failed & cancelled + delete their files", R.drawable.ic_delete, danger = true) {
                                            confirmClear = listOf("failed", "cancelled", "denied") to "Failed & cancelled"
                                        } else null,
                                        if (admin) MenuEntry("Clear completed + delete the songs", R.drawable.ic_delete, danger = true) {
                                            confirmClear = listOf("completed") to "Completed downloads"
                                        } else null,
                                    ),
                                )
                            }
                            // every request still waiting (admins: everyone's unless Only mine is on; members: their own)
                            SmallButton("Clear waiting", iconRes = R.drawable.ic_close, enabled = !countingWaiting, modifier = Modifier.align(Alignment.CenterVertically)) { clearWaiting() }
                        }
                    }
                }
            }
            // imports paused because a folder can't be written: the banner over the list (admins: Check again)
            item(key = "storage-banner") { StorageBanner(nav, admin) { reload() } }
            // a new request: paste a link (the web's Downloader) or search Discover
            item {
                Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    EncoreField(
                        value = link,
                        onValueChange = { link = it; probe = null; probeError = null },
                        placeholder = "Paste a link: Spotify, YouTube, Deezer, Tidal, Qobuz, Amazon Music, Bandcamp…",
                        leadingIcon = R.drawable.ic_link,
                        enabled = !linkBusy,
                        keyboardType = KeyboardType.Uri,
                        imeAction = ImeAction.Go,
                        onAction = { requestLink() },
                        modifier = Modifier.weight(1f),
                    )
                    Spacer(Modifier.width(8.dp))
                    SmallButton("Request", iconRes = R.drawable.ic_download, primary = true, enabled = !linkBusy && link.isNotBlank()) { requestLink() }
                }
            }
            if (link.isNotBlank()) {
                // the Downloader's choices for this link
                item {
                    Text("If a song is on an album:", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(horizontal = Gutter))
                    val modeIndex = RequestRules.ALBUM_MODES.indexOfFirst { it.first == albumMode }.coerceAtLeast(0)
                    ChoiceRow(RequestRules.ALBUM_MODES.map { it.second }, modeIndex, onSelect = { i ->
                        albumMode = RequestRules.ALBUM_MODES[i].first
                        AlbumModePref.write(context, albumMode)
                    })
                    val metadataOnly = RequestRules.metadataOnly(link)
                    FlowRow(Modifier.padding(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        SmallButton("Preview", enabled = !probing && !linkBusy) { preview() }
                        SmallButton("Import as playlist", iconRes = R.drawable.ic_queue_music, enabled = !linkBusy) { importAsPlaylist() }
                        SmallButton("Exactly this link", iconRes = R.drawable.ic_link, enabled = !linkBusy && !metadataOnly) { exactly() }
                    }
                    if (metadataOnly) {
                        Text(
                            "This service has no downloadable audio – use “Import as playlist”",
                            style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(horizontal = Gutter, vertical = 4.dp),
                        )
                    }
                    val pv = probe
                    val pe = probeError
                    when {
                        probing -> Row(Modifier.padding(horizontal = Gutter, vertical = 8.dp)) { Spinner() }
                        pe != null -> Text(pe, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err, modifier = Modifier.padding(horizontal = Gutter, vertical = 8.dp))
                        pv != null -> Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                            Cover(pv.thumbnail.ifEmpty { null }, nav.graph.imageLoader, size = 56.dp, corner = 8.dp, label = pv.title)
                            Spacer(Modifier.width(12.dp))
                            Column(Modifier.weight(1f)) {
                                Text(pv.title.ifEmpty { "Untitled" }, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                val sub = RequestRules.probeLine(pv)
                                if (sub.isNotEmpty()) Text(sub, style = MaterialTheme.typography.bodySmall, color = p.muted)
                            }
                        }
                    }
                }
            } else {
                item {
                    Row(Modifier.padding(horizontal = Gutter, vertical = 2.dp)) {
                        SmallButton("Search Discover to request music", iconRes = R.drawable.ic_search) { nav.showTab(Tab.DISCOVER) }
                    }
                }
            }
            item { ChoiceRow(FILTERS.map { it.second }, filter, onSelect = { filter = it }) }
            when {
                err != null -> item { Box(Modifier.fillMaxWidth().height(200.dp)) { ErrorBox(err) { error = null; reload() } } }
                list == null -> item { Box(Modifier.fillMaxWidth().height(200.dp)) { LoadingBox() } }
                else -> {
                    val top = list.filter { r -> r.parentId <= 0L || list.none { it.id == r.parentId } }
                    if (top.isEmpty()) {
                        item { EmptyState(R.drawable.ic_inbox, "Nothing here", "Head to Discover to request music.") }
                    }
                    for (r in top) {
                        val kids = list.filter { it.parentId == r.id }
                        item(key = "req-${r.id}") {
                            RequestItem(actions, r)
                            if (kids.isNotEmpty()) {
                                var open by remember { mutableStateOf(false) }
                                Text(
                                    "${kids.size} child request(s) · ${kids.count { it.status == "completed" }} available",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = p.muted,
                                    modifier = Modifier.padding(start = 78.dp, end = Gutter, bottom = 10.dp).clickable { open = !open },
                                )
                                if (open) {
                                    // keyed, so an open ⋯ stays on its row when the 4 s reload moves the children
                                    for (k in kids) key(k.id) { RequestItem(actions, k, indent = 24.dp) }
                                }
                            }
                        }
                    }
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }

    RequestSheets(actions, rows ?: emptyList())
    confirmWaiting?.let { n ->
        ConfirmDialog(
            title = "Clear waiting",
            text = RequestRules.clearWaitingQuestion(n),
            okLabel = "Clear waiting",
            danger = true,
            onDismiss = { confirmWaiting = null },
        ) { clearWaitingNow() }
    }
    confirmClear?.let { (statuses, label) ->
        ConfirmDialog(
            title = "Delete files",
            text = "$label: this permanently deletes the files from disk.",
            okLabel = "Delete",
            danger = true,
            onDismiss = { confirmClear = null },
        ) { clear(statuses, true) }
    }
}
