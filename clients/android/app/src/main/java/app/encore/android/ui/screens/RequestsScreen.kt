package app.encore.android.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.repo.CANCELLABLE_STATUSES
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.PickerOpts
import app.encore.android.data.repo.RequestRow
import app.encore.android.data.repo.formatAgo
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.Tab
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MenuSeparator
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private val FILTERS = listOf("all" to "All", "active" to "Active", "pending" to "Awaiting approval", "completed" to "Available", "failed" to "Failed")

/** Requests (web/js/pages/requests.js): the list with status badges, Pick / Retry / Cancel, a new request from a link. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RequestsScreen(nav: Nav) {
    val features = nav.features
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var me by remember { mutableStateOf<Me?>(null) }
    var filter by remember { mutableStateOf(0) }
    var mine by remember { mutableStateOf(false) }
    var rows by remember { mutableStateOf<List<RequestRow>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var generation by remember { mutableStateOf(0) }
    var link by remember { mutableStateOf("") }
    var linkBusy by remember { mutableStateOf(false) }
    var confirmFiles by remember { mutableStateOf<Pair<RequestRow, String>?>(null) }
    var confirmClear by remember { mutableStateOf<Pair<List<String>, String>?>(null) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()

    suspend fun load() {
        try {
            val m = me ?: withContext(Dispatchers.IO) { features.me() }.also { me = it; if (!it.isAdmin && !mine) mine = true }
            rows = withContext(Dispatchers.IO) { features.requests(FILTERS[filter].first, mine && m.isAdmin) }
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (rows == null) error = e.message ?: "Couldn't load the requests"
        }
    }
    fun reload() { generation += 1 }

    LaunchedEffect(filter, mine, generation) { load() }
    // the web refreshes every 4 s while the page is open
    LaunchedEffect(Unit) {
        while (true) {
            delay(4000)
            load()
        }
    }

    fun act(r: RequestRow, action: String) {
        scope.launch {
            try {
                withContext(Dispatchers.IO) { features.requestAction(r.id, action) }
                reload()
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }
    fun remove(r: RequestRow, files: Boolean) {
        scope.launch {
            try {
                val n = withContext(Dispatchers.IO) { features.deleteRequest(r.id, files) }
                if (files) Toasts.show("$n file(s) deleted", "ok")
                reload()
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }
    fun clear(statuses: List<String>, files: Boolean) {
        scope.launch {
            try {
                val (removed, deleted) = withContext(Dispatchers.IO) { features.clearRequests(statuses, files, mine) }
                Toasts.show("$removed cleared" + if (files) " · $deleted file(s) deleted" else "", "ok")
                reload()
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }
    fun requestLink() {
        val url = link.trim()
        if (url.isEmpty()) return
        linkBusy = true
        scope.launch {
            try {
                val msg = withContext(Dispatchers.IO) { features.requestLink(url) }
                Toasts.show(msg, "ok")
                link = ""
                reload()
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't request that link", "err")
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
                    MoreButton(
                        entries = listOf(
                            MenuEntry("Clear completed", R.drawable.ic_check) { clear(listOf("completed"), false) },
                            MenuEntry("Clear failed & cancelled", R.drawable.ic_close) { clear(listOf("failed", "cancelled", "denied"), false) },
                            MenuSeparator,
                            MenuEntry("Clear failed & cancelled + delete their files", R.drawable.ic_delete, danger = true) {
                                confirmClear = listOf("failed", "cancelled", "denied") to "Failed & cancelled"
                            },
                            MenuEntry("Clear completed + delete the songs", R.drawable.ic_delete, danger = true) {
                                confirmClear = listOf("completed") to "Completed downloads"
                            },
                        ),
                    )
                }
            })
        },
    ) { padding ->
        val list = rows
        val err = error
        LazyColumn(Modifier.fillMaxSize().padding(padding)) {
            // quota and the admin's "Only mine"
            item {
                Column(Modifier.padding(horizontal = Gutter)) {
                    if (m != null && m.requestLimit > 0 && !m.isAdmin) {
                        Text("${m.requestsThisWeek} of ${m.requestLimit} requests used this week", style = MaterialTheme.typography.bodySmall, color = p.muted)
                    }
                    if (admin) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Switch(checked = mine, onCheckedChange = { mine = it })
                            Spacer(Modifier.width(8.dp))
                            Text("Only mine", style = MaterialTheme.typography.bodyMedium)
                        }
                    }
                }
            }
            // a new request: paste a link (the "Share to Encore" endpoint) or search Discover
            item {
                Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    EncoreField(
                        value = link,
                        onValueChange = { link = it },
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
            item {
                Row(Modifier.padding(horizontal = Gutter, vertical = 2.dp)) {
                    SmallButton("Search Discover to request music", iconRes = R.drawable.ic_search) { nav.showTab(Tab.DISCOVER) }
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
                            RequestItem(nav, r, m, admin, onAct = { act(r, it) }, onRemove = { remove(r, false) }, onDeleteFiles = { confirmFiles = r to it })
                            if (kids.isNotEmpty()) {
                                var open by remember { mutableStateOf(false) }
                                Text(
                                    "${kids.size} child request(s) · ${kids.count { it.status == "completed" }} available",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = p.muted,
                                    modifier = Modifier.padding(start = 78.dp, end = Gutter, bottom = 10.dp).clickable { open = !open },
                                )
                                if (open) {
                                    for (k in kids) {
                                        RequestItem(nav, k, m, admin, onAct = { act(k, it) }, onRemove = { remove(k, false) }, onDeleteFiles = { confirmFiles = k to it }, indent = 24.dp)
                                    }
                                }
                            }
                        }
                    }
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }

    confirmFiles?.let { (r, st) ->
        ConfirmDialog(
            title = "Delete files",
            text = if (st == "completed") "Permanently delete the songs this request downloaded from the library?" else "Delete the leftover/partial files of this download?",
            okLabel = "Delete",
            danger = true,
            onDismiss = { confirmFiles = null },
        ) { remove(r, true) }
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

/** common.js `requestRow`: cover, what, kind badge · via source · message · by user · ago, progress, the actions. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun RequestItem(
    nav: Nav,
    r: RequestRow,
    me: Me?,
    admin: Boolean,
    onAct: (String) -> Unit,
    onRemove: () -> Unit,
    onDeleteFiles: (status: String) -> Unit,
    indent: androidx.compose.ui.unit.Dp = 0.dp,
) {
    val p = MaterialTheme.palette
    Row(Modifier.fillMaxWidth().padding(start = Gutter + indent, end = Gutter, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.Top) {
        Cover(r.coverUrl.ifEmpty { null }, nav.graph.imageLoader, size = 48.dp, corner = 8.dp, label = r.title.ifEmpty { r.artist })
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(r.what, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                if (r.whatNote.isNotEmpty()) Text(" · " + r.whatNote, style = MaterialTheme.typography.bodyMedium, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Row(Modifier.padding(top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                Badge(r.kind)
                val bits = ArrayList<String>()
                if (r.attempts > 1) bits.add("${r.attempts} attempts")
                if (r.source.isNotEmpty()) bits.add("via ${r.source}")
                if (r.message.isNotEmpty()) bits.add(r.message)
                if (admin && r.username.isNotEmpty()) bits.add("by ${r.username}")
                bits.add(formatAgo(r.createdAt))
                Text(" " + bits.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            if (r.isActive && r.progress > 0) {
                Box(Modifier.padding(top = 6.dp).fillMaxWidth(0.8f).height(5.dp).clip(RoundedCornerShape(99.dp)).background(p.panel2)) {
                    Box(Modifier.fillMaxWidth(r.progress.coerceIn(0, 100) / 100f).fillMaxHeight().background(p.gradient))
                }
            }
            FlowRow(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                StatusBadge(r.status, Modifier.align(Alignment.CenterVertically))
                if (admin && r.status == "pending") {
                    SmallButton("Approve", iconRes = R.drawable.ic_check, primary = true) { onAct("approve") }
                    SmallButton("Deny") { onAct("deny") }
                }
                if ((r.kind == "album" || r.kind == "track") && r.status != "completed" && me?.can("pick") == true) {
                    SmallButton("Pick", iconRes = R.drawable.ic_search) {
                        nav.open(Screen.Picker(PickerOpts(kind = r.kind, mbid = r.mbid, artist = r.artist, title = r.title, album = r.album, lengthSec = r.lengthSec, requestId = r.id)))
                    }
                }
                if (r.status == "failed" || r.status == "cancelled" || r.kind == "playlist") {
                    SmallButton("Retry", iconRes = R.drawable.ic_refresh) { onAct("retry") }
                }
                if (r.status in CANCELLABLE_STATUSES) {
                    SmallButton("Cancel", iconRes = R.drawable.ic_close) { onAct("cancel") }
                }
                if (!r.isActive) {
                    MoreButton(
                        entries = listOf(
                            MenuEntry("Remove from list", R.drawable.ic_close) { onRemove() },
                            if (admin) MenuEntry(if (r.status == "completed") "Delete downloaded files" else "Delete leftover files", R.drawable.ic_delete, danger = true) { onDeleteFiles(r.status) } else null,
                        ),
                        iconSize = 18.dp,
                    )
                }
            }
        }
    }
}
