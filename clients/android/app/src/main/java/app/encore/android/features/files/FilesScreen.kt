package app.encore.android.features.files

import android.content.ContentResolver
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.AppGraph
import app.encore.android.R
import app.encore.android.data.api.ApiException
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MenuSeparator
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.RowDivider
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDay
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.components.formatSize
import app.encore.android.ui.screens.EncoreField
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.IOException

/**
 * The admin file manager (0.33; web System → Files): the roots (the library, staging, the download folders, Trash),
 * a folder's files with breadcrumbs, sorting and multi-select (a long press starts it), New folder, Rename, Move (a
 * folder picker over the roots), Move to trash, the Trash with Restore / Delete forever / Empty trash, Upload (files,
 * or a whole folder, streamed from the phone), Edit tags for audio files, and Import… with "Keep the files where they
 * are". Admins only; opened from Settings → Server → Files or the Home menu.
 *
 * A search box over each folder: as you type it filters the folder, and once typing pauses it searches the whole root
 * by name at any depth (`GET /api/files/search`, 0.37.1); each result shows its folder's path, and a tap opens that
 * folder with the file picked (a folder opens itself).
 */
@Composable
fun FilesScreen(nav: Nav, startRoot: String = "") {
    val api = remember(nav.graph) { nav.graph.files }
    val p = MaterialTheme.palette
    var roots by remember { mutableStateOf<List<FileRoot>?>(null) }
    var rootsError by remember { mutableStateOf<String?>(null) }
    var rootsKey by remember { mutableIntStateOf(0) }
    var root by remember { mutableStateOf<FileRoot?>(null) }
    var path by remember { mutableStateOf("") }
    // the folder the browsing entered through a link (its path): nothing at or below it is written (client-shapes §2.4;
    // the server's `via_link` says the same for a folder it reached through a link, §4.17)
    var linkAt by remember { mutableStateOf<String?>(null) }
    // the entry to pick once the folder opened is listed (a search result's file)
    var pick by remember { mutableStateOf<String?>(null) }
    fun under(p: String): Boolean = linkAt != null && FilesRules.linkAfter(linkAt, p, false) != null
    /** Opens [p]; [link]: it is a link to a folder (the first one on the way is remembered); [pickName]: picked there. */
    fun go(p: String, link: Boolean = false, pickName: String? = null) {
        linkAt = FilesRules.linkAfter(linkAt, p, link)
        path = p
        pick = pickName
    }

    // opened on a root (0.36: "Open Trash" after albums went to the Trash): that root once the roots are in
    var started by remember { mutableStateOf(startRoot.isEmpty()) }
    LaunchedEffect(rootsKey) {
        rootsError = null
        try {
            roots = withContext(Dispatchers.IO) { api.roots() }
            if (!started) {
                started = true
                roots?.firstOrNull { it.id == startRoot }?.let { root = it; go("") }
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            rootsError = if (e is ApiException && e.code == 404) "This server has no file manager yet (Encore 0.33 or newer has one)." else e.message ?: "Couldn't load the folders"
        }
    }

    // Back: up a folder, then out of the root, then out of the page
    BackHandler(enabled = root != null) {
        if (path.isNotEmpty()) go(parentOf(path)) else root = null
    }

    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(top = 4.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = {
                val r = root
                when {
                    r == null -> nav.back()
                    path.isNotEmpty() -> go(parentOf(path))
                    else -> root = null
                }
            }) { Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back") }
            Text(root?.name ?: "Files", style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        }
        UploadPanel()
        val r = root
        when {
            rootsError != null -> ErrorBox(rootsError ?: "") { rootsKey++ }
            roots == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(color = p.accent, trackColor = p.border)
            }
            r == null -> RootsList(roots ?: emptyList()) { picked -> root = picked; go("") }
            r.isTrash -> TrashView(nav, api, roots ?: emptyList())
            else -> FolderView(
                nav, api, roots ?: emptyList(), r, path, throughLink = under(path), onOpen = { p, link -> go(p, link) },
                pick = pick, onPicked = { pick = null }, onFound = { p, link, name -> go(p, link, name) },
            )
        }
    }
}

private fun parentOf(path: String): String = FilesRules.parentOf(path)

private fun join(path: String, name: String): String = FilesRules.join(path, name)

// ---- roots ------------------------------------------------------------------------------------------------------

@Composable
private fun RootsList(roots: List<FileRoot>, onPick: (FileRoot) -> Unit) {
    val p = MaterialTheme.palette
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            Text(
                "The folders Encore may change.",
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
                modifier = Modifier.padding(horizontal = Gutter, vertical = 8.dp),
            )
        }
        items(roots, key = { it.id }) { r ->
            Row(
                Modifier.fillMaxWidth().clickable { onPick(r) }.padding(horizontal = Gutter, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(painterResource(if (r.isTrash) R.drawable.ic_delete else R.drawable.ic_folder), contentDescription = null, tint = if (r.isTrash) p.muted else p.accent)
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(r.name, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                        // the server lists Trash as not writable (nothing is written into it but Move to trash)
                        if (!r.writable && !r.isTrash) Badge("Read-only")
                    }
                    if (r.path.isNotEmpty()) Text(r.path, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                if (r.freeBytes > 0) Text("${formatSize(r.freeBytes)} free", style = MaterialTheme.typography.bodySmall, color = p.faint)
            }
            RowDivider()
        }
    }
}

// ---- a folder -----------------------------------------------------------------------------------------------------

/** What a dialog of the folder view is about. */
private sealed interface FolderDialog {
    data object NewFolder : FolderDialog
    data class Rename(val name: String) : FolderDialog
    data class Move(val names: List<String>) : FolderDialog
    data class Trash(val names: List<String>) : FolderDialog
    data class Tags(val name: String) : FolderDialog
    data class Import(val folder: String) : FolderDialog
    data class Upload(val items: List<UploadItem>) : FolderDialog
}

@OptIn(ExperimentalFoundationApi::class, ExperimentalLayoutApi::class)
@Composable
private fun FolderView(
    nav: Nav,
    api: FilesApi,
    roots: List<FileRoot>,
    root: FileRoot,
    path: String,
    /** this folder was reached through a link: browsing only (the link itself is changed from its own folder) */
    throughLink: Boolean,
    /** open a folder; true when the entry opened is a link */
    onOpen: (String, Boolean) -> Unit,
    /** an entry to pick once this folder is listed (a search result's file), then [onPicked] */
    pick: String? = null,
    onPicked: () -> Unit = {},
    /** a search result tapped: open (folder, it is a link, the name to pick there) */
    onFound: (String, Boolean, String?) -> Unit = { p, link, _ -> onOpen(p, link) },
) {
    val p = MaterialTheme.palette
    // the server's word on it: the folder was reached through a link (§4.17 `via_link`)
    var viaLink by remember(root.id, path) { mutableStateOf(false) }
    val linked = throughLink || viaLink
    val canWrite = root.writable && !linked
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var sort by remember { mutableStateOf("name") }
    var dir by remember { mutableStateOf("asc") }
    var reload by remember { mutableIntStateOf(0) }
    val entries = remember(root.id, path) { mutableStateListOf<FileEntry>() }
    var total by remember(root.id, path) { mutableIntStateOf(0) }
    var loading by remember(root.id, path) { mutableStateOf(true) }
    var error by remember(root.id, path) { mutableStateOf<String?>(null) }
    val selected = remember(root.id, path) { mutableStateMapOf<String, Boolean>() }
    var dialog by remember { mutableStateOf<FolderDialog?>(null) }
    val uploadsDone by FileUploads.done.collectAsState()
    // one scroll position per folder, so a search result can be scrolled to
    val listState = remember(root.id, path) { LazyListState() }
    // the search: its words filter this folder at once and search the whole root after a pause (a new folder starts
    // with a blank box, its words were for the one left)
    var query by remember(root.id, path) { mutableStateOf("") }
    var hits by remember(root.id, path) { mutableStateOf<FileHits?>(null) }
    var finding by remember(root.id, path) { mutableStateOf(false) }
    var findError by remember(root.id, path) { mutableStateOf<String?>(null) }
    val shown = if (query.isBlank()) entries.toList() else entries.filter { FilesRules.matches(it.name, query) }

    LaunchedEffect(root.id, query) {
        findError = null
        if (query.isBlank()) {
            hits = null
            finding = false
            return@LaunchedEffect
        }
        // a newer word cancels this one before it asks
        delay(250)
        finding = true
        try {
            hits = withContext(Dispatchers.IO) { api.search(root.id, query.trim()) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            hits = null
            findError = e.message ?: "Couldn't search"
        }
        finding = false
    }

    LaunchedEffect(root.id, path, sort, dir, reload, uploadsDone) {
        loading = true
        error = null
        try {
            val page = withContext(Dispatchers.IO) { api.list(root.id, path, sort, dir, 0) }
            entries.clear()
            entries.addAll(page.entries)
            total = page.total
            viaLink = page.viaLink
            selected.keys.retainAll(page.entries.map { it.name }.toSet())
            // a search result's file: picked and scrolled to
            pick?.let { name ->
                val at = page.entries.indexOfFirst { it.name == name }
                if (at >= 0) {
                    selected[name] = true
                    // a request, kept until the list is laid out (it isn't while the folder loads)
                    listState.requestScrollToItem(at)
                }
                onPicked()
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Couldn't open the folder"
        }
        loading = false
    }

    fun refresh() { reload++ }

    fun run(done: String, block: () -> String) {
        scope.launch {
            try {
                val failed = withContext(Dispatchers.IO) { block() }
                if (failed.isEmpty()) Toasts.show(done, "ok") else Toasts.show(failed, "warn")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
            selected.clear()
            refresh()
        }
    }

    /**
     * A name dialog's work (New folder, Rename): when it went, the dialog closes and [done] is said; when the server
     * says no, its words go under the name box ([fail]) and the dialog stays, as on the web.
     */
    fun named(done: String, fail: (String) -> Unit, block: () -> Unit) {
        scope.launch {
            try {
                withContext(Dispatchers.IO) { block() }
                dialog = null
                Toasts.show(done, "ok")
                selected.clear()
                refresh()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                fail(e.message ?: "That didn't work")
            }
        }
    }

    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) {
            scope.launch {
                val items = withContext(Dispatchers.IO) { FilesApi.documents(context.contentResolver, uris) }
                if (items.isNotEmpty()) dialog = FolderDialog.Upload(items)
            }
        }
    }
    val pickFolder = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        if (uri != null) {
            scope.launch {
                val items = withContext(Dispatchers.IO) { try { FilesApi.tree(context.contentResolver, uri) } catch (_: Exception) { emptyList() } }
                if (items.isEmpty()) Toasts.show("That folder has no files", "info") else dialog = FolderDialog.Upload(items)
            }
        }
    }

    val picked = selected.keys.toList()
    Column(Modifier.fillMaxSize()) {
        Breadcrumbs(root, path) { onOpen(it, false) }
        if (linked) {
            Text(
                "This folder is reached through a link, so Encore doesn't change anything in it.",
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
                modifier = Modifier.padding(horizontal = Gutter, vertical = 2.dp),
            )
        }
        EncoreField(
            value = query,
            onValueChange = { q ->
                query = q
                // the picked entries the filter hides are let go (nothing hidden is moved or trashed)
                if (q.isNotBlank()) selected.keys.retainAll(entries.filter { FilesRules.matches(it.name, q) }.map { it.name }.toSet())
            },
            placeholder = "Search ${root.name} by name",
            leadingIcon = R.drawable.ic_search,
            imeAction = ImeAction.Search,
            modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 4.dp),
        )
        if (picked.isEmpty()) {
            var uploadMenu by remember { mutableStateOf(false) }
            var sortMenu by remember { mutableStateOf(false) }
            FlowRow(
                Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Box {
                    SmallButton(FilesRules.sortButton(sort, dir)) { sortMenu = true }
                    PopMenu(sortMenu, { sortMenu = false }, FilesRules.SORTS.map { (k, label) ->
                        // the chosen sort says its order; choosing it again turns the order round
                        MenuEntry(label + if (k == sort) " (" + FilesRules.sortOrderText(k, dir) + ")" else "") {
                            val (s, d) = FilesRules.nextSort(sort, dir, k)
                            sort = s
                            dir = d
                        }
                    })
                }
                if (canWrite) {
                    SmallButton("New folder", iconRes = R.drawable.ic_create_folder) { dialog = FolderDialog.NewFolder }
                    Box {
                        SmallButton("Upload", iconRes = R.drawable.ic_upload) { uploadMenu = true }
                        PopMenu(uploadMenu, { uploadMenu = false }, listOf(
                            MenuEntry("Upload files", R.drawable.ic_file) { pickFiles.launch(arrayOf("*/*")) },
                            MenuEntry("Upload a folder", R.drawable.ic_folder) { pickFolder.launch(null) },
                        ))
                    }
                }
                if (!linked) SmallButton("Import…", iconRes = R.drawable.ic_download) { dialog = FolderDialog.Import(path) }
            }
        } else {
            // multi-select: what to do with the picked entries
            FlowRow(
                Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text("${picked.size} selected", style = MaterialTheme.typography.labelLarge, modifier = Modifier.align(Alignment.CenterVertically))
                if (canWrite) {
                    if (picked.size == 1) SmallButton("Rename", iconRes = R.drawable.ic_edit) { dialog = FolderDialog.Rename(picked[0]) }
                    SmallButton("Move", iconRes = R.drawable.ic_move) { dialog = FolderDialog.Move(picked) }
                    SmallButton(FilesRules.deleteLabel(FilesRules.trashKeep(roots).first), iconRes = R.drawable.ic_delete) { dialog = FolderDialog.Trash(picked) }
                }
                SmallButton("Select all") { for (e in shown) selected[e.name] = true }
                SmallButton("Cancel") { selected.clear() }
            }
        }
        when {
            loading && entries.isEmpty() -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(color = p.accent, trackColor = p.border)
            }
            error != null -> ErrorBox(error ?: "") { refresh() }
            entries.isEmpty() && query.isBlank() -> EmptyState(R.drawable.ic_folder, "This folder is empty", if (canWrite) "Upload files here, or make a new folder." else "")
            else -> LazyColumn(Modifier.fillMaxSize(), state = listState, contentPadding = PaddingValues(bottom = 24.dp)) {
                if (shown.isEmpty() && query.isNotBlank()) {
                    item {
                        Text(
                            "Nothing in this folder matches “${query.trim()}”",
                            style = MaterialTheme.typography.bodyMedium,
                            color = p.muted,
                            modifier = Modifier.padding(horizontal = Gutter, vertical = 12.dp),
                        )
                    }
                }
                items(shown, key = { it.name }) { e ->
                    val isSel = selected[e.name] == true
                    FileRow(
                        e = e,
                        selected = isSel,
                        selecting = picked.isNotEmpty(),
                        onClick = {
                            when {
                                picked.isNotEmpty() -> if (isSel) selected.remove(e.name) else selected[e.name] = true
                                e.isDir -> onOpen(join(path, e.name), e.kind == "link")
                                else -> selected[e.name] = true
                            }
                        },
                        onLongClick = { if (isSel) selected.remove(e.name) else selected[e.name] = true },
                        menu = listOf(
                            if (e.isDir) MenuEntry("Open", R.drawable.ic_folder) { onOpen(join(path, e.name), e.kind == "link") } else null,
                            if (e.audio?.trackId?.let { it > 0L } == true) MenuEntry("Play", R.drawable.ic_play) { playTrack(nav, e.audio.trackId) } else null,
                            if (e.isAudio && e.kind == "file" && canWrite) MenuEntry("Edit tags", R.drawable.ic_edit) { dialog = FolderDialog.Tags(e.name) } else null,
                            // nothing is imported through a link
                            if (e.isDir && e.kind != "link" && !linked) MenuEntry("Import…", R.drawable.ic_download) { dialog = FolderDialog.Import(join(path, e.name)) } else null,
                            MenuSeparator,
                            // on a link these act on the link itself, never on what it points to
                            if (canWrite) MenuEntry("Rename", R.drawable.ic_edit) { dialog = FolderDialog.Rename(e.name) } else null,
                            if (canWrite) MenuEntry("Move", R.drawable.ic_move) { dialog = FolderDialog.Move(listOf(e.name)) } else null,
                            if (canWrite) MenuEntry(FilesRules.deleteLabel(FilesRules.trashKeep(roots).first), R.drawable.ic_delete, danger = true) { dialog = FolderDialog.Trash(listOf(e.name)) } else null,
                        ),
                    )
                }
                if (entries.size < total) {
                    item {
                        Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
                            SmallButton("Show more (${total - entries.size})") {
                                scope.launch {
                                    try {
                                        val more = withContext(Dispatchers.IO) { api.list(root.id, path, sort, dir, entries.size) }
                                        entries.addAll(more.entries.filter { m -> entries.none { it.name == m.name } })
                                        total = more.total
                                    } catch (e: CancellationException) {
                                        throw e
                                    } catch (e: Exception) {
                                        Toasts.show(e.message ?: "Couldn't load more", "err")
                                    }
                                }
                            }
                        }
                    }
                }
                // the search across the root, under the folder's rows
                if (query.isNotBlank()) {
                    // no keys on these rows: a key of theirs could be the same as a file's name
                    item {
                        val words = when {
                            finding -> "Searching ${root.name}…"
                            findError != null -> findError ?: ""
                            hits != null -> FilesRules.foundLine(hits?.hits?.size ?: 0, hits?.truncated == true, root.name)
                            else -> ""
                        }
                        if (words.isNotEmpty()) {
                            Row(Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 16.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                                Icon(painterResource(R.drawable.ic_search), contentDescription = null, tint = p.muted, modifier = Modifier.size(18.dp))
                                Spacer(Modifier.width(8.dp))
                                Text(words, style = MaterialTheme.typography.labelLarge, color = if (findError != null) EncoreColors.Err else p.text)
                            }
                        }
                    }
                    if (!finding) {
                        itemsIndexed(hits?.hits ?: emptyList()) { _, h ->
                            HitRow(h, root.name) {
                                val (to, link, name) = FilesRules.hitTarget(h)
                                if (to == path) {
                                    // in this folder already: the search goes and the file is picked
                                    query = ""
                                    selected.clear()
                                    if (name != null) {
                                        selected[name] = true
                                        val at = entries.indexOfFirst { it.name == name }
                                        if (at >= 0) listState.requestScrollToItem(at)
                                    }
                                } else {
                                    onFound(to, link, name)
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    when (val d = dialog) {
        null -> {}
        FolderDialog.NewFolder -> NameDialog("New folder", "", "Make folder", onDismiss = { dialog = null }) { name, fail ->
            named("Folder made", fail) { api.mkdir(root.id, path, name) }
        }
        is FolderDialog.Rename -> NameDialog("Rename", d.name, "Rename", onDismiss = { dialog = null }) { name, fail ->
            named("Renamed", fail) { api.rename(root.id, join(path, d.name), name) }
        }
        is FolderDialog.Trash -> {
            val (keep, days) = FilesRules.trashKeep(roots)
            AlertDialog(
                onDismissRequest = { dialog = null },
                containerColor = p.panel,
                titleContentColor = p.text,
                textContentColor = p.muted,
                title = { Text(FilesRules.deleteTitle(d.names, keep)) },
                text = { Text(FilesRules.deleteBody(keep, days)) },
                confirmButton = {
                    TextButton(onClick = {
                        dialog = null
                        run(FilesRules.deletedLine(d.names.size, keep)) { api.trash(root.id, d.names.map { join(path, it) }) }
                    }) { Text(FilesRules.deleteLabel(keep), color = EncoreColors.Err, fontWeight = FontWeight.SemiBold) }
                },
                dismissButton = { TextButton(onClick = { dialog = null }) { Text("Cancel", color = p.muted) } },
            )
        }
        is FolderDialog.Move -> FolderPicker(api, roots.filter { !it.isTrash && it.writable }, root, path, title = if (d.names.size == 1) "Move “${d.names[0]}”" else "Move ${d.names.size} items", onDismiss = { dialog = null }) { toRoot, toPath ->
            dialog = null
            if (toRoot.id == root.id && toPath == path) return@FolderPicker
            run(if (d.names.size == 1) "Moved" else "Moved ${d.names.size} items") { api.move(root.id, d.names.map { join(path, it) }, toRoot.id, toPath) }
        }
        is FolderDialog.Tags -> TagsDialog(api, root, join(path, d.name), onDismiss = { dialog = null }, onSaved = { dialog = null; refresh() })
        is FolderDialog.Import -> ImportSheet(api, root, d.folder, onDismiss = { dialog = null }, onDone = { dialog = null; refresh() })
        is FolderDialog.Upload -> UploadDialog(d.items, root, path, onDismiss = { dialog = null }) { conflict ->
            dialog = null
            FileUploads.start(nav.graph, context.contentResolver, root, path, d.items, conflict)
        }
    }
}

/** One search result: its icon, its name and the path of the folder it is in; a tap opens it. */
@Composable
private fun HitRow(h: FileHit, rootName: String, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    val e = h.entry
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = Gutter, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val icon = when {
            e.kind == "link" -> R.drawable.ic_link_file
            e.isDir -> R.drawable.ic_folder
            e.isAudio -> R.drawable.ic_note
            else -> R.drawable.ic_file
        }
        Icon(painterResource(icon), contentDescription = null, tint = if (e.isDir) p.accent else p.muted, modifier = Modifier.size(24.dp))
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(e.name, fontWeight = if (e.isDir) FontWeight.SemiBold else FontWeight.Normal, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(FilesRules.place(rootName, h.parent), style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (!e.isDir) Text(formatSize(e.size), style = MaterialTheme.typography.bodySmall, color = p.faint)
    }
    RowDivider()
}

private fun playTrack(nav: Nav, id: Long) {
    nav.graph.scope.launch {
        val t = try { withContext(Dispatchers.IO) { nav.graph.api.track(id) } } catch (_: Exception) { null }
        if (t == null) Toasts.show("That song isn't in the library", "err") else nav.player.play(listOf(t))
    }
}

@Composable
private fun Breadcrumbs(root: FileRoot, path: String, onOpen: (String) -> Unit) {
    val p = MaterialTheme.palette
    val parts = if (path.isEmpty()) emptyList() else path.split('/')
    LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
        item {
            Text(
                root.name,
                style = MaterialTheme.typography.labelLarge,
                color = if (parts.isEmpty()) p.text else p.accent,
                modifier = Modifier.clip(RoundedCornerShape(6.dp)).clickable { onOpen("") }.padding(horizontal = 4.dp, vertical = 6.dp),
            )
        }
        itemsIndexed(parts) { i, part ->
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("›", color = p.faint, modifier = Modifier.padding(end = 4.dp))
                Text(
                    part,
                    style = MaterialTheme.typography.labelLarge,
                    color = if (i == parts.lastIndex) p.text else p.accent,
                    maxLines = 1,
                    modifier = Modifier.clip(RoundedCornerShape(6.dp)).clickable { onOpen(parts.take(i + 1).joinToString("/")) }.padding(horizontal = 4.dp, vertical = 6.dp),
                )
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun FileRow(e: FileEntry, selected: Boolean, selecting: Boolean, onClick: () -> Unit, onLongClick: () -> Unit, menu: List<MenuEntry?>) {
    val p = MaterialTheme.palette
    Row(
        Modifier
            .fillMaxWidth()
            .background(if (selected) p.accent.copy(alpha = 0.10f) else p.bg)
            .combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(start = Gutter, end = 4.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (selecting) {
            Checkbox(
                checked = selected,
                onCheckedChange = { onClick() },
                colors = CheckboxDefaults.colors(checkedColor = p.accent, checkmarkColor = p.onAccent, uncheckedColor = p.muted),
            )
        } else {
            val icon = when {
                e.kind == "link" -> R.drawable.ic_link_file
                e.isDir -> R.drawable.ic_folder
                e.isAudio -> R.drawable.ic_note
                else -> R.drawable.ic_file
            }
            Icon(painterResource(icon), contentDescription = null, tint = if (e.isDir) p.accent else p.muted, modifier = Modifier.padding(end = 14.dp).size(24.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(e.name, fontWeight = if (e.isDir) FontWeight.SemiBold else FontWeight.Normal, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val a = e.audio
            val line = when {
                e.kind == "link" -> "Link to " + e.target.ifEmpty { "somewhere else" }
                a != null && (a.title.isNotEmpty() || a.artist.isNotEmpty()) ->
                    listOf(listOf(a.artist, a.title).filter { it.isNotEmpty() }.joinToString(" – "), formatDuration((a.durationSec * 1000).toLong()), formatSize(e.size)).filter { it.isNotEmpty() }.joinToString(" · ")
                e.isDir -> formatDay(e.mtime)
                else -> listOf(formatSize(e.size), formatDay(e.mtime)).joinToString(" · ")
            }
            Text(line, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (!selecting) MoreButton(menu)
    }
}

// ---- dialogs ---------------------------------------------------------------------------------------------------

/**
 * A dialog with one name box ("New folder" / "Make folder", "Rename" / "Rename"). The name is checked as it is typed
 * (the server's rule and words, [FilesRules.nameProblem]); [onSubmit] gets the name and a callback for the server's
 * refusal, which shows under the box while the dialog stays open (the caller closes it when the name went).
 */
@Composable
private fun NameDialog(title: String, initial: String, confirm: String, onDismiss: () -> Unit, onSubmit: (String, (String) -> Unit) -> Unit) {
    val p = MaterialTheme.palette
    var text by remember { mutableStateOf(initial) }
    var refused by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val name = text.trim()
    val problem = FilesRules.nameProblem(name)
    // a rename to the same name has nothing to do
    val ok = problem == null && !busy && !(initial.isNotEmpty() && name == initial)
    // an empty box says nothing yet; the button waits
    val shown = refused ?: if (name.isEmpty()) null else problem
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(title) },
        text = {
            Column {
                OutlinedTextField(value = text, onValueChange = { text = it; refused = null }, singleLine = true, placeholder = { Text("Name") })
                if (shown != null) Text(shown, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err, modifier = Modifier.padding(top = 6.dp))
            }
        },
        confirmButton = {
            TextButton(enabled = ok, onClick = {
                busy = true
                onSubmit(name) { why -> busy = false; refused = why }
            }) { Text(confirm, color = if (ok) p.text else p.faint, fontWeight = FontWeight.SemiBold) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/** Pick a folder over the roots ("Move here"). */
@Composable
private fun FolderPicker(api: FilesApi, roots: List<FileRoot>, startRoot: FileRoot, startPath: String, title: String, onDismiss: () -> Unit, onPick: (FileRoot, String) -> Unit) {
    val p = MaterialTheme.palette
    var root by remember { mutableStateOf(roots.firstOrNull { it.id == startRoot.id } ?: roots.firstOrNull()) }
    var path by remember { mutableStateOf(if (root?.id == startRoot.id) startPath else "") }
    var dirs by remember { mutableStateOf<List<FileEntry>?>(null) }
    LaunchedEffect(root?.id, path) {
        dirs = null
        val r = root ?: return@LaunchedEffect
        dirs = try { withContext(Dispatchers.IO) { api.list(r.id, path, "name", "asc", 0).entries.filter { it.kind == "dir" } } } catch (_: Exception) { emptyList() }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.text,
        title = { Text(title) },
        text = {
            Column(Modifier.heightIn(max = 420.dp)) {
                LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    items(roots, key = { it.id }) { r ->
                        Badge(r.name, kind = if (r.id == root?.id) "accent" else "", onClick = { root = r; path = "" })
                    }
                }
                Spacer(Modifier.height(8.dp))
                Text(FilesRules.place(root?.name ?: "", path), style = MaterialTheme.typography.bodySmall, color = p.muted)
                Spacer(Modifier.height(4.dp))
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 320.dp)) {
                    if (path.isNotEmpty()) {
                        item {
                            Row(Modifier.fillMaxWidth().clickable { path = parentOf(path) }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                                Icon(painterResource(R.drawable.ic_arrow_upward), contentDescription = null, tint = p.muted, modifier = Modifier.size(20.dp))
                                Spacer(Modifier.width(10.dp))
                                Text("Up one folder", color = p.muted)
                            }
                        }
                    }
                    val list = dirs
                    if (list == null) {
                        item { Text("Loading…", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(vertical = 8.dp)) }
                    } else if (list.isEmpty()) {
                        item { Text("No folders here", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(vertical = 8.dp)) }
                    } else {
                        items(list, key = { it.name }) { d ->
                            Row(Modifier.fillMaxWidth().clickable { path = join(path, d.name) }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                                Icon(painterResource(R.drawable.ic_folder), contentDescription = null, tint = p.accent, modifier = Modifier.size(20.dp))
                                Spacer(Modifier.width(10.dp))
                                Text(d.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            TextButton(enabled = root != null, onClick = { root?.let { onPick(it, path) } }) { Text("Move here", color = p.text, fontWeight = FontWeight.SemiBold) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/** The labels of the Fix song dialog (SongFix.kt, the web's tag editor: "Track #", "Year"), with "Disc #" and "Genre". */
private val TAG_LABELS = mapOf(
    "title" to "Title",
    "artist" to "Artist",
    "album" to "Album",
    "albumartist" to "Album artist",
    "tracknumber" to "Track #",
    "discnumber" to "Disc #",
    "date" to "Year",
    "genre" to "Genre",
)

/** Edit tags of one audio file (the tag writer of the Fix song dialog). */
@Composable
private fun TagsDialog(api: FilesApi, root: FileRoot, path: String, onDismiss: () -> Unit, onSaved: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val values = remember { mutableStateMapOf<String, String>() }
    var original by remember { mutableStateOf<Map<String, String>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    LaunchedEffect(path) {
        try {
            val t = withContext(Dispatchers.IO) { api.tags(root.id, path) }
            values.clear()
            values.putAll(t)
            original = t
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Couldn't read the tags"
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.text,
        title = { Text("Edit tags") },
        text = {
            Column(Modifier.heightIn(max = 460.dp).verticalScroll(rememberScrollState())) {
                Text(path.substringAfterLast('/'), style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.height(8.dp))
                when {
                    error != null -> Text(error ?: "", color = EncoreColors.Err, style = MaterialTheme.typography.bodySmall)
                    original == null -> Text("Loading…", color = p.muted, style = MaterialTheme.typography.bodySmall)
                    else -> for (k in FilesApi.TAGS) {
                        val numeric = k == "tracknumber" || k == "discnumber" || k == "date"
                        OutlinedTextField(
                            value = values[k] ?: "",
                            onValueChange = { values[k] = it },
                            label = { Text(TAG_LABELS[k] ?: k) },
                            singleLine = true,
                            keyboardOptions = if (numeric) KeyboardOptions(keyboardType = KeyboardType.Number) else KeyboardOptions.Default,
                            modifier = Modifier.fillMaxWidth().padding(bottom = 6.dp),
                        )
                    }
                }
            }
        },
        confirmButton = {
            val orig = original
            TextButton(enabled = orig != null && !saving, onClick = {
                val changed = FilesApi.TAGS.filter { (values[it] ?: "") != (orig?.get(it) ?: "") }.associateWith { values[it] ?: "" }
                if (changed.isEmpty()) { onDismiss(); return@TextButton }
                saving = true
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) { api.putTags(root.id, path, changed) }
                        Toasts.show("Tags saved", "ok")
                        onSaved()
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "Couldn't write the tags", "err")
                    }
                    saving = false
                }
            }) { Text(if (saving) "Saving…" else "Save", color = p.text, fontWeight = FontWeight.SemiBold) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/** The files picked to upload: how many, how big, and what to do with names already there. */
@Composable
private fun UploadDialog(items: List<UploadItem>, root: FileRoot, path: String, onDismiss: () -> Unit, onStart: (String) -> Unit) {
    val p = MaterialTheme.palette
    var conflict by remember { mutableStateOf("rename") }
    val size = items.sumOf { it.size.coerceAtLeast(0L) }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(if (items.size == 1) "Upload “${items[0].name.substringAfterLast('/')}”?" else "Upload ${items.size} files?") },
        text = {
            Column {
                Text("To " + FilesRules.place(root.name, path) + (if (size > 0) " · ${formatSize(size)}" else ""), style = MaterialTheme.typography.bodySmall)
                Spacer(Modifier.height(10.dp))
                Text("When a file with the same name is there", style = MaterialTheme.typography.bodyMedium, color = p.text)
                // a replaced file is moved to Trash first, never deleted (client-shapes §4.3)
                for ((k, label) in listOf("rename" to "Keep both (the new one gets “(2)”)", "replace" to "Replace it (the file that is there goes to Trash)", "skip" to "Skip the new one")) {
                    Row(Modifier.fillMaxWidth().clickable { conflict = k }.padding(vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                        androidx.compose.material3.RadioButton(selected = conflict == k, onClick = { conflict = k })
                        Text(label, style = MaterialTheme.typography.bodySmall, color = p.text)
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = { onStart(conflict) }) { Text("Upload", color = p.text, fontWeight = FontWeight.SemiBold) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

// ---- import --------------------------------------------------------------------------------------------------

/** The import preview's empty answer (the server says it with a 404, §4.14). */
private const val NO_SONGS = "No songs in this folder"

/**
 * Import… on a folder: the album folders in it with the release each was matched to (the match review), or – with
 * "Keep the files where they are" – the songs added to the library where and as they are.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ImportSheet(api: FilesApi, root: FileRoot, folder: String, onDismiss: () -> Unit, onDone: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    // the library's own folder is already where songs live: keeping them in place is the only sensible import
    // (and a read-only folder can only be imported in place)
    var keep by remember { mutableStateOf(root.id == "library" || !root.writable) }
    val mode = if (keep) "in_place" else "move"
    var albums by remember { mutableStateOf<List<ImportAlbum>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val skip = remember { mutableStateMapOf<String, Boolean>() }
    var others by remember { mutableStateOf<ImportAlbum?>(null) }
    var applying by remember { mutableStateOf(false) }

    LaunchedEffect(mode) {
        albums = null
        error = null
        try {
            albums = withContext(Dispatchers.IO) { api.importPreview(root.id, folder, mode) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: ApiException) {
            // the server's 404 for a folder without songs is the empty answer, not an error (as the web shows it)
            if (e.code == 404 && e.message == NO_SONGS) albums = emptyList() else error = e.message ?: "Couldn't read the folder"
        } catch (e: Exception) {
            error = e.message ?: "Couldn't read the folder"
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        LazyColumn(Modifier.fillMaxWidth().fillMaxHeight(0.9f)) {
            item {
                Column(Modifier.padding(horizontal = 20.dp)) {
                    Text("Import", style = MaterialTheme.typography.titleMedium)
                    Text(FilesRules.place(root.name, folder), style = MaterialTheme.typography.bodySmall, color = p.muted)
                    Row(Modifier.fillMaxWidth().clickable { keep = !keep }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text("Keep the files where they are", style = MaterialTheme.typography.bodyMedium)
                            Text(
                                if (keep) "The songs join the library where and as they are: nothing is moved, renamed or retagged."
                                else "Each album is tagged as the release it matches and moved into the library.",
                                style = MaterialTheme.typography.bodySmall,
                                color = p.muted,
                            )
                        }
                        Switch(
                            checked = keep,
                            onCheckedChange = { keep = it },
                            colors = SwitchDefaults.colors(checkedTrackColor = p.accent, checkedThumbColor = p.onAccent, uncheckedTrackColor = p.panel2, uncheckedThumbColor = p.muted, uncheckedBorderColor = p.border),
                        )
                    }
                }
            }
            val list = albums
            when {
                error != null -> item { Text(error ?: "", color = EncoreColors.Err, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(20.dp)) }
                list == null -> item {
                    Row(Modifier.padding(20.dp), verticalAlignment = Alignment.CenterVertically) {
                        CircularProgressIndicator(color = p.accent, trackColor = p.border, modifier = Modifier.size(20.dp))
                        Spacer(Modifier.width(10.dp))
                        Text(if (keep) "Reading the folder…" else "Looking for matches…", color = p.muted, style = MaterialTheme.typography.bodySmall)
                    }
                }
                list.isEmpty() -> item { Text(NO_SONGS, color = p.muted, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(20.dp)) }
                else -> items(list, key = { it.folder }) { a ->
                    ImportAlbumCard(a, keep, on = skip[a.folder] != true, onToggle = { skip[a.folder] = !(skip[a.folder] ?: false) }, onOthers = { others = a })
                }
            }
            item {
                val chosen = list?.filter { skip[it.folder] != true } ?: emptyList()
                Row(Modifier.fillMaxWidth().padding(20.dp), horizontalArrangement = Arrangement.End) {
                    TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) }
                    Spacer(Modifier.width(8.dp))
                    TextButton(enabled = chosen.isNotEmpty() && !applying, onClick = {
                        applying = true
                        scope.launch {
                            try {
                                val done = withContext(Dispatchers.IO) { api.importApply(root.id, folder, mode, chosen.map { it.folder to (if (keep) null else it.release?.id) }) }
                                // some album folders weren't imported: the server's line names the first and why
                                Toasts.show(done.message, if (done.failed > 0) "warn" else "ok")
                                onDone()
                            } catch (e: CancellationException) {
                                throw e
                            } catch (e: Exception) {
                                Toasts.show(e.message ?: "The import didn't work", "err")
                            }
                            applying = false
                        }
                    }) {
                        val n = chosen.size
                        Text(if (applying) "Importing…" else FilesRules.importButton(n), color = if (n > 0) p.text else p.faint, fontWeight = FontWeight.SemiBold)
                    }
                }
            }
        }
    }

    others?.let { a ->
        OtherMatches(api, root, a, mode, onDismiss = { others = null }) { updated ->
            others = null
            albums = albums?.map { if (it.folder == a.folder) updated else it }
        }
    }
}

@Composable
private fun ImportAlbumCard(a: ImportAlbum, keep: Boolean, on: Boolean, onToggle: () -> Unit, onOthers: () -> Unit) {
    val p = MaterialTheme.palette
    var open by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(12.dp)
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 5.dp).clip(shape).background(p.bg2).border(1.dp, if (on) p.border else p.border.copy(alpha = 0.4f), shape).padding(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Checkbox(checked = on, onCheckedChange = { onToggle() }, colors = CheckboxDefaults.colors(checkedColor = p.accent, checkmarkColor = p.onAccent, uncheckedColor = p.muted))
            Column(Modifier.weight(1f)) {
                Text(listOf(a.artist, a.album).filter { it.isNotEmpty() }.joinToString(" – ").ifEmpty { a.folder }, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text("${a.folder.ifEmpty { "This folder" }} · ${a.files} files", style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (!keep) {
            val r = a.release
            Text(
                if (r != null) "Matches " + r.line + (if (a.confidence > 0) " · ${Math.round(a.confidence * 100)}% sure" else "") else "No match found: it is imported with its own tags.",
                style = MaterialTheme.typography.bodySmall,
                color = if (r != null && a.confidence >= 0.8) p.text else EncoreColors.Warn,
                modifier = Modifier.padding(start = 12.dp, top = 4.dp),
            )
            Row(Modifier.padding(start = 8.dp, top = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SmallButton("Other matches") { onOthers() }
                if (a.rows.isNotEmpty()) SmallButton(if (open) "Hide the files" else "Show the files") { open = !open }
            }
            if (open) {
                for (row in a.rows.take(60)) {
                    Text(
                        row.oldTitle.ifEmpty { row.path.substringAfterLast('/') } + (if (row.newTitle.isNotEmpty()) "  →  " + (if (row.newTrack > 0) "${row.newTrack}. " else "") + row.newTitle else "  (no match)"),
                        style = MaterialTheme.typography.bodySmall,
                        color = if (row.newTitle.isNotEmpty()) p.muted else EncoreColors.Warn,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(start = 12.dp, top = 2.dp),
                    )
                }
            }
        }
    }
}

/**
 * Other releases for one album folder: the match review's search over its metadata sources (MusicBrainz first, as
 * on the web); picking one shows its preview.
 */
@Composable
private fun OtherMatches(api: FilesApi, root: FileRoot, a: ImportAlbum, mode: String, onDismiss: () -> Unit, onPicked: (ImportAlbum) -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var provider by remember { mutableStateOf(FilesApi.PROVIDERS[0].first) }
    var artist by remember { mutableStateOf(a.artist) }
    var album by remember { mutableStateOf(a.album) }
    var found by remember { mutableStateOf<List<ImportRelease>?>(null) }
    var failed by remember { mutableStateOf<String?>(null) }
    var searching by remember { mutableStateOf(false) }
    var seq by remember { mutableIntStateOf(0) }
    fun search() {
        searching = true
        failed = null
        val n = ++seq
        val (source, ar, al) = Triple(provider, artist.trim(), album.trim())
        scope.launch {
            val res = try {
                withContext(Dispatchers.IO) { api.importMatch(root.id, a.folder, ar, al, source) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (n == seq) failed = "The search didn't work: " + (e.message ?: "")
                emptyList()
            }
            // a search for another source that answered later is not shown
            if (n == seq) {
                found = res
                searching = false
            }
        }
    }
    LaunchedEffect(Unit) { search() }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.text,
        title = { Text("Other matches") },
        text = {
            Column(Modifier.heightIn(max = 480.dp)) {
                LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    items(FilesApi.PROVIDERS, key = { it.first }) { (k, label) ->
                        Badge(label, kind = if (k == provider) "accent" else "", onClick = { if (k != provider) { provider = k; search() } })
                    }
                }
                Spacer(Modifier.height(6.dp))
                OutlinedTextField(value = artist, onValueChange = { artist = it }, singleLine = true, label = { Text("Artist") }, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(value = album, onValueChange = { album = it }, singleLine = true, label = { Text("Album") }, modifier = Modifier.fillMaxWidth())
                Spacer(Modifier.height(6.dp))
                SmallButton(if (searching) "Searching…" else "Search", enabled = !searching) { search() }
                Spacer(Modifier.height(6.dp))
                LazyColumn(Modifier.fillMaxWidth().heightIn(max = 280.dp)) {
                    val list = found
                    val why = failed
                    if (why != null && !searching) item { Text(why, color = EncoreColors.Err, style = MaterialTheme.typography.bodySmall) }
                    else if (list != null && list.isEmpty() && !searching) item { Text("Nothing found", color = p.muted, style = MaterialTheme.typography.bodySmall) }
                    items(list ?: emptyList(), key = { it.id }) { r ->
                        Column(Modifier.fillMaxWidth().clickable {
                            scope.launch {
                                try {
                                    val one = withContext(Dispatchers.IO) { api.importPreview(root.id, a.folder, mode, r.id) }.firstOrNull()
                                    onPicked(one ?: a.copy(release = r, confidence = r.score))
                                } catch (e: CancellationException) {
                                    throw e
                                } catch (e: Exception) {
                                    Toasts.show(e.message ?: "Couldn't use that release", "err")
                                }
                            }
                        }.padding(vertical = 8.dp)) {
                            Text(r.line, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            Text((if (r.trackCount > 0) "${r.trackCount} songs · " else "") + (if (r.score > 0) "${Math.round(r.score * 100)}% match" else ""), style = MaterialTheme.typography.bodySmall, color = p.muted)
                        }
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onDismiss) { Text("Close", color = p.muted) } },
    )
}

// ---- trash ------------------------------------------------------------------------------------------------------

@Composable
private fun TrashView(nav: Nav, api: FilesApi, roots: List<FileRoot>) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var items by remember { mutableStateOf<List<TrashItem>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    var confirmEmpty by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf<TrashItem?>(null) }
    LaunchedEffect(reload) {
        error = null
        try {
            items = withContext(Dispatchers.IO) { api.trashItems() }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Couldn't open the trash"
        }
    }
    fun run(done: String, block: () -> String) {
        scope.launch {
            try {
                val failed = withContext(Dispatchers.IO) { block() }
                if (failed.isEmpty()) Toasts.show(done, "ok") else Toasts.show(failed, "warn")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
            reload++
        }
    }
    val list = items
    when {
        error != null -> ErrorBox(error ?: "") { reload++ }
        list == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(color = p.accent, trackColor = p.border) }
        list.isEmpty() -> FilesRules.trashKeep(roots).let { (keep, days) -> EmptyState(R.drawable.ic_delete, "The trash is empty", FilesRules.trashEmptyLine(keep, days)) }
        else -> LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
            item {
                Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("${list.size} in the trash", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.weight(1f))
                    SmallButton("Empty trash", iconRes = R.drawable.ic_delete) { confirmEmpty = true }
                }
            }
            items(list, key = { it.id }) { t ->
                val rootName = roots.firstOrNull { it.id == t.root }?.name ?: t.root
                Row(Modifier.fillMaxWidth().padding(start = Gutter, end = 4.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon(painterResource(if (t.kind == "dir") R.drawable.ic_folder else R.drawable.ic_file), contentDescription = null, tint = p.muted, modifier = Modifier.padding(end = 14.dp).size(24.dp))
                    Column(Modifier.weight(1f)) {
                        Text(t.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            listOf(FilesRules.trashFrom(rootName, t.path), formatDay(t.at), if (t.by.isNotEmpty()) "by ${t.by}" else "", if (t.size > 0) formatSize(t.size) else "")
                                .filter { it.isNotEmpty() }.joinToString(" · "),
                            style = MaterialTheme.typography.bodySmall,
                            color = p.muted,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    MoreButton(listOf(
                        MenuEntry("Restore", R.drawable.ic_restore) { run("Restored") { api.restore(listOf(t.id)) } },
                        MenuEntry("Delete forever", R.drawable.ic_delete, danger = true) { confirmDelete = t },
                    ))
                }
                RowDivider()
            }
        }
    }
    if (confirmEmpty) {
        AlertDialog(
            onDismissRequest = { confirmEmpty = false },
            containerColor = p.panel,
            titleContentColor = p.text,
            textContentColor = p.muted,
            title = { Text("Empty the trash?") },
            text = { Text("Everything in it is deleted forever. This can't be undone.") },
            confirmButton = { TextButton(onClick = { confirmEmpty = false; run("The trash is empty") { api.emptyTrash(); "" } }) { Text("Empty trash", color = EncoreColors.Err, fontWeight = FontWeight.SemiBold) } },
            dismissButton = { TextButton(onClick = { confirmEmpty = false }) { Text("Cancel", color = p.muted) } },
        )
    }
    confirmDelete?.let { t ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            containerColor = p.panel,
            titleContentColor = p.text,
            textContentColor = p.muted,
            title = { Text("Delete “${t.name}” forever?") },
            text = { Text("This can't be undone.") },
            confirmButton = { TextButton(onClick = { confirmDelete = null; run("Deleted") { api.deleteForever(t.id); "" } }) { Text("Delete forever", color = EncoreColors.Err, fontWeight = FontWeight.SemiBold) } },
            dismissButton = { TextButton(onClick = { confirmDelete = null }) { Text("Cancel", color = p.muted) } },
        )
    }
}

// ---- uploads ----------------------------------------------------------------------------------------------------

/** Where an upload is: file n of m, its name, the bytes sent of all. */
data class UploadProgress(val done: Int, val count: Int, val name: String, val sent: Long, val total: Long, val failed: Int)

/**
 * The uploads, one file per request, in the app's scope so they go on when the page is left; [UploadService] keeps
 * the app in the foreground until the batch ends, so Android does not freeze or close it halfway. One batch at a
 * time; [done] is bumped when a batch ends (the folder that is open reloads).
 */
object FileUploads {
    private val _progress = MutableStateFlow<UploadProgress?>(null)
    val progress: StateFlow<UploadProgress?> = _progress.asStateFlow()
    private val _done = MutableStateFlow(0)
    val done: StateFlow<Int> = _done.asStateFlow()
    @Volatile private var cancelled = false
    private var job: Job? = null

    fun start(graph: AppGraph, resolver: ContentResolver, root: FileRoot, path: String, items: List<UploadItem>, conflict: String) {
        if (job?.isActive == true) {
            Toasts.show("Wait for the upload that is running", "info")
            return
        }
        if (items.isEmpty()) return
        cancelled = false
        val api = graph.files
        val total = items.sumOf { it.size.coerceAtLeast(0L) }
        // set before the service starts: it shows this and stops when it goes back to null
        _progress.value = UploadProgress(0, items.size, items[0].name, 0L, total, 0)
        job = graph.scope.launch {
            var sentBefore = 0L
            var ok = 0
            var failed = 0
            var firstError = ""
            try {
                for ((i, item) in items.withIndex()) {
                    if (cancelled) break
                    _progress.value = UploadProgress(i, items.size, item.name, sentBefore, total, failed)
                    try {
                        val out = withContext(Dispatchers.IO) {
                            api.upload(root.id, path, conflict, item, resolver) { sent ->
                                if (cancelled) throw IOException("Cancelled")
                                _progress.value = UploadProgress(i, items.size, item.name, sentBefore + sent, total, failed)
                            }
                        }
                        if (out.result == "skipped") { failed++; if (firstError.isEmpty()) firstError = "${item.name.substringAfterLast('/')}: ${out.reason}" } else ok++
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        if (cancelled) break
                        failed++
                        if (firstError.isEmpty()) firstError = "${item.name.substringAfterLast('/')}: ${e.message ?: "failed"}"
                    }
                    sentBefore += item.size.coerceAtLeast(0L)
                }
            } finally {
                _progress.value = null
            }
            _done.value = _done.value + 1
            val (line, kind) = FilesRules.uploadEnd(ok, items.size, cancelled, firstError)
            Toasts.show(line, kind)
        }
        UploadService.start(graph.app)
    }

    fun cancel() { cancelled = true }
}

/** The running upload: file n of m, a progress line, Stop. */
@Composable
private fun UploadPanel() {
    val pr by FileUploads.progress.collectAsState()
    val u = pr ?: return
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(12.dp)
    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp).clip(shape).background(p.bg2).border(1.dp, p.border, shape).padding(10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Uploading ${u.done + 1} of ${u.count}", style = MaterialTheme.typography.labelLarge)
                Text(u.name, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            SmallButton("Stop") { FileUploads.cancel() }
        }
        Spacer(Modifier.height(6.dp))
        if (u.total > 0) LinearProgressIndicator(progress = { (u.sent.toFloat() / u.total).coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth(), color = p.accent, trackColor = p.border)
        else LinearProgressIndicator(modifier = Modifier.fillMaxWidth(), color = p.accent, trackColor = p.border)
    }
}

// ---- the entry in Settings --------------------------------------------------------------------------------------

/** Settings → Server → Files (admins, on a server that has the file manager). */
@Composable
fun FilesEntry(nav: Nav, onOpen: () -> Unit) {
    val available by FilesAccess.available.collectAsState()
    var admin by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        admin = try { withContext(Dispatchers.IO) { nav.graph.api.me().isAdmin } } catch (_: Exception) { false }
        if (admin) FilesAccess.probe(nav.graph)
    }
    if (!admin || available != true) return
    val p = MaterialTheme.palette
    Column {
        Text("Server", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(start = Gutter, end = Gutter, top = 24.dp, bottom = 12.dp))
        Row(Modifier.fillMaxWidth().clickable(onClick = onOpen).padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Files")
                Text("Browse, upload, move, rename and import in the library and download folders", style = MaterialTheme.typography.bodySmall, color = p.muted)
            }
            Icon(painterResource(R.drawable.ic_folder), contentDescription = null, tint = p.muted)
        }
    }
}
