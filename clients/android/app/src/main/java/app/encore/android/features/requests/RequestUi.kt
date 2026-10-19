package app.encore.android.features.requests

import android.content.Context
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.PickerOpts
import app.encore.android.data.repo.RequestRow
import app.encore.android.data.repo.StorageHealth
import app.encore.android.data.repo.formatAgo
import app.encore.android.features.settings.copyToClipboard
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.copyLabel
import app.encore.android.ui.components.rememberCopied
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.StatusBadge
import app.encore.android.ui.screens.features
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.text.DateFormat
import java.util.Date

/** A unix time as the details and the copied text show it: the phone's date and time, with seconds. */
internal fun requestDate(t: Double): String =
    if (t <= 0) "—" else DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.MEDIUM).format(Date((t * 1000).toLong()))

/**
 * What a list of request rows can do (web-rs/src/pages/common.rs `bind_request_actions`), one per list: Approve, Deny,
 * Retry, Import again and Cancel (POST /api/requests/{id}/{action}), Remove from list (DELETE /api/requests/{id}),
 * the admin's Delete files (`?files=true`, after a confirm), Pick (the picker, for this request), the details sheet
 * and its Copy. [me] is set by the screen once GET /api/auth/me has answered.
 */
@Stable
class RequestActions(val nav: Nav, private val scope: CoroutineScope, private val reload: () -> Unit) {
    var me by mutableStateOf<Me?>(null)
    val admin: Boolean get() = me?.isAdmin == true

    /** The row whose files the admin asked to delete (the confirm dialog is open). */
    internal var confirmFiles by mutableStateOf<RequestRow?>(null)

    /** Cancel ("cancel") or Remove from list ("remove") of a request that keeps downloaded files, asked first. */
    internal var confirmLose by mutableStateOf<Pair<RequestRow, String>?>(null)

    /** The request whose details sheet is open (0: none). */
    internal var detailsFor by mutableStateOf(0L)

    /** Rows with an action on its way: their buttons wait (the web disables the button it pressed). */
    internal var busy by mutableStateOf(emptySet<Long>())

    /** The server's version for the copied text (GET /api/auth/info, asked once when a request is first copied). */
    private var serverVersion: String? = null

    /**
     * Copy: the request as text for a bug report ([RequestRules.requestText], the web's lines), with the server's
     * version; "Copied to clipboard", or "Copy failed – select and copy manually". [done] hears how it went (the
     * sheet sits above the toasts and says so itself).
     */
    fun copy(r: RequestRow, context: Context, done: (Boolean) -> Unit = {}) {
        scope.launch {
            val version = serverVersion ?: loadVersion().also { if (it.isNotEmpty()) serverVersion = it }
            done(copyToClipboard(context, RequestRules.requestText(r, version, ::requestDate)))
        }
    }

    /** The server's version, "" when it can't be asked (the text then says "unknown"). */
    private suspend fun loadVersion(): String =
        try {
            withContext(Dispatchers.IO) { nav.features.info().version }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            ""
        }

    fun act(r: RequestRow, action: String) {
        if (r.id in busy) return
        busy = busy + r.id
        scope.launch {
            try {
                withContext(Dispatchers.IO) { nav.features.requestAction(r.id, action) }
                reload()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
            busy = busy - r.id
        }
    }

    /** Cancel: a request that keeps downloaded files asks first – they go with it (the web's confirm). */
    fun cancel(r: RequestRow) {
        if (r.hasFiles) confirmLose = r to "cancel" else act(r, "cancel")
    }

    /** Remove from list: the same for a request that keeps downloaded files. */
    fun removeFromList(r: RequestRow) {
        if (r.hasFiles) confirmLose = r to "remove" else remove(r, false)
    }

    fun remove(r: RequestRow, files: Boolean) {
        // one at a time per row: a second tap while the first waits would answer "Request not found"
        if (r.id in busy) return
        busy = busy + r.id
        scope.launch {
            try {
                val n = withContext(Dispatchers.IO) { nav.features.deleteRequest(r.id, files) }
                if (files) Toasts.show("$n file(s) deleted", "ok")
                if (detailsFor == r.id) detailsFor = 0L
                reload()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
            busy = busy - r.id
        }
    }

    /** Pick: the picker for this request (the pick replaces what it would download). */
    fun pick(r: RequestRow) {
        detailsFor = 0L
        nav.open(Screen.Picker(PickerOpts(kind = r.kind, mbid = r.mbid, artist = r.artist, title = r.title, album = r.album, lengthSec = r.lengthSec, requestId = r.id)))
    }

    fun showDetails(r: RequestRow) {
        detailsFor = r.id
    }

    /** The row's ⋯ menu (also on a long press): Details, and for a request that stopped moving Remove / Delete files. */
    fun menuEntries(r: RequestRow): List<MenuEntry?> = listOf(
        MenuEntry("Details", R.drawable.ic_note) { showDetails(r) },
        if (RequestRules.canRemove(r)) MenuEntry("Remove from list", R.drawable.ic_close) { removeFromList(r) } else null,
        if (RequestRules.canRemove(r) && admin) MenuEntry(RequestRules.filesLabel(r.status, r.hasFiles), R.drawable.ic_delete, danger = true) { confirmFiles = r } else null,
    )
}

@Composable
fun rememberRequestActions(nav: Nav, reload: () -> Unit): RequestActions {
    val scope = rememberCoroutineScope()
    val latest = rememberUpdatedState(reload)
    return remember(nav) { RequestActions(nav, scope) { latest.value() } }
}

/**
 * The buttons of a request (common.rs `request_row`): Approve, Deny, Pick, Retry or Import again (kept files: nothing
 * is downloaded again), Cancel – whichever apply.
 */
@Composable
private fun RequestButtons(a: RequestActions, r: RequestRow) {
    val wait = r.id in a.busy
    if (RequestRules.canApprove(r, a.admin)) {
        SmallButton("Approve", iconRes = R.drawable.ic_check, primary = true, enabled = !wait) { a.act(r, "approve") }
        SmallButton("Deny", danger = true, enabled = !wait) { a.act(r, "deny") }
    }
    if (RequestRules.canPick(r, a.me)) SmallButton("Pick", iconRes = R.drawable.ic_search) { a.pick(r) }
    if (RequestRules.canImportAgain(r)) SmallButton("Import again", iconRes = R.drawable.ic_refresh, enabled = !wait) { a.act(r, "import-again") }
    if (RequestRules.canRetry(r, a.me)) SmallButton("Retry", iconRes = R.drawable.ic_refresh, enabled = !wait) { a.act(r, "retry") }
    if (RequestRules.canCancel(r)) SmallButton("Cancel", iconRes = R.drawable.ic_close, enabled = !wait) { a.cancel(r) }
}

@Composable
private fun ProgressBar(progress: Int, modifier: Modifier = Modifier) {
    val p = MaterialTheme.palette
    Box(modifier.height(5.dp).clip(RoundedCornerShape(99.dp)).background(p.panel2)) {
        Box(Modifier.fillMaxWidth(progress.coerceIn(0, 100) / 100f).fillMaxHeight().background(p.gradient))
    }
}

/**
 * One request (common.rs `request_row`): cover, what, kind badge · attempts · via source · message · by user · ago,
 * progress, the status and its buttons, and ⋯. A tap opens the details sheet with the whole message and the log; a
 * long press opens ⋯ (the web's right-click).
 */
@OptIn(ExperimentalLayoutApi::class, ExperimentalFoundationApi::class)
@Composable
fun RequestItem(a: RequestActions, r: RequestRow, indent: Dp = 0.dp) {
    val p = MaterialTheme.palette
    var menu by remember { mutableStateOf(false) }
    Row(
        Modifier
            .fillMaxWidth()
            .combinedClickable(onClickLabel = "Details", onLongClickLabel = "More", onClick = { a.showDetails(r) }, onLongClick = { menu = true })
            .padding(start = Gutter + indent, end = Gutter, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Cover(r.coverUrl.ifEmpty { null }, a.nav.graph.imageLoader, size = 48.dp, corner = 8.dp, label = r.title.ifEmpty { r.artist })
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(r.what, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                if (r.whatNote.isNotEmpty()) Text(" · " + r.whatNote, style = MaterialTheme.typography.bodyMedium, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Row(Modifier.padding(top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                Badge(r.kind)
                Text(" " + RequestRules.statusLine(r, a.admin, formatAgo(r.createdAt)), style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            if (r.isActive && r.progress > 0) ProgressBar(r.progress, Modifier.padding(top = 6.dp).fillMaxWidth(0.8f))
            FlowRow(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                StatusBadge(r.status, Modifier.align(Alignment.CenterVertically))
                RequestButtons(a, r)
                Box {
                    IconButton(onClick = { menu = true }, modifier = Modifier.size(32.dp)) {
                        Icon(painterResource(R.drawable.ic_more_vert), contentDescription = "More", tint = p.muted, modifier = Modifier.size(18.dp))
                    }
                    PopMenu(menu, { menu = false }, a.menuEntries(r))
                }
            }
        }
    }
}

/** The dialogs of a request list: the admin's "Delete files" confirm and the details sheet of the row in [rows]. */
@Composable
fun RequestSheets(a: RequestActions, rows: List<RequestRow>) {
    a.confirmFiles?.let { r ->
        ConfirmDialog(
            title = "Delete files",
            text = RequestRules.filesConfirm(r.status, r.hasFiles),
            okLabel = "Delete",
            danger = true,
            onDismiss = { a.confirmFiles = null },
        ) { a.remove(r, true) }
    }
    a.confirmLose?.let { (r, what) ->
        val cancel = what == "cancel"
        ConfirmDialog(
            title = if (cancel) "Cancel this request?" else "Remove this request?",
            text = RequestRules.KEPT_FILES_LOST,
            okLabel = if (cancel) "Cancel request" else "Remove",
            danger = true,
            onDismiss = { a.confirmLose = null },
        ) { if (cancel) a.act(r, "cancel") else a.remove(r, false) }
    }
    if (a.detailsFor > 0L) {
        // the row as the list has it now (the list reloads every few seconds, so the sheet follows the request)
        val r = rows.firstOrNull { it.id == a.detailsFor }
        if (r != null) RequestDetailsSheet(a, r, onDismiss = { a.detailsFor = 0L })
        else LaunchedEffect(a.detailsFor) { a.detailsFor = 0L }
    }
}

/**
 * The details of one request: the whole status message (the row cuts it at two lines), what a waiting request waits
 * for, what the row squeezes into its status line, the candidate being fetched, the failed step with its operation,
 * path and errno, the worker's log – one line per source tried, with each source's own error – streamrip's output,
 * Copy (all of it as text, for a bug report) and the request's buttons.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun RequestDetailsSheet(a: RequestActions, r: RequestRow, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val copied = rememberCopied()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val d = RequestRules.details(r, a.admin, formatAgo(r.createdAt), ::requestDate)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 22.dp).padding(bottom = 24.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Cover(r.coverUrl.ifEmpty { null }, a.nav.graph.imageLoader, size = 56.dp, corner = 8.dp, label = r.title.ifEmpty { r.artist })
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f)) {
                    Text(d.title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                    if (d.note.isNotEmpty()) Text(d.note, style = MaterialTheme.typography.bodyMedium, color = p.muted)
                }
            }
            FlowRow(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Badge(r.kind, modifier = Modifier.align(Alignment.CenterVertically))
                StatusBadge(r.status, Modifier.align(Alignment.CenterVertically))
                SmallButton(copyLabel(copied.value), iconRes = if (copied.value == true) R.drawable.ic_check else R.drawable.ic_content_copy) {
                    a.copy(r, context) { copied.value = it }
                }
            }
            if (copied.value == false) Text(Toasts.COPY_FAILED, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err, modifier = Modifier.padding(top = 6.dp))
            if (d.status.isNotEmpty()) Text(d.status, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn, modifier = Modifier.padding(top = 8.dp))
            if (r.isActive && r.progress > 0) ProgressBar(r.progress, Modifier.padding(top = 10.dp).fillMaxWidth())
            if (d.message.isNotEmpty()) {
                SelectionContainer {
                    Text(d.message, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(top = 12.dp))
                }
            }
            Column(Modifier.padding(top = 10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                for (f in d.facts) Text(f, style = MaterialTheme.typography.bodySmall, color = p.muted)
            }
            // the failed file or program step: its sentence, then step, operation, path and errno
            if (d.error.isNotEmpty() || d.errorFacts.isNotEmpty()) {
                Text("Error", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 16.dp, bottom = 4.dp))
                SelectionContainer {
                    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        if (d.error.isNotEmpty() && d.error != d.message) Text(d.error, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err)
                        for (f in d.errorFacts) Text(f, style = MaterialTheme.typography.bodySmall, color = p.muted)
                    }
                }
            }
            if (d.log.isNotEmpty()) {
                Text("Log", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 16.dp, bottom = 4.dp))
                SelectionContainer {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        for (e in d.log) {
                            Column {
                                if (e.source.isNotEmpty()) Text(e.source, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.SemiBold)
                                Text(e.text, style = MaterialTheme.typography.bodySmall, color = p.muted)
                            }
                        }
                    }
                }
            }
            // streamrip's own output (stdout and stderr, up to 20,000 characters), whole and selectable
            if (d.output.isNotEmpty()) {
                Text(d.outputTitle, style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 16.dp))
                if (d.outputHead.isNotEmpty()) Text(d.outputHead, style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(bottom = 4.dp))
                SelectionContainer {
                    Text(
                        d.output, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace, color = p.muted,
                        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(p.panel2).padding(10.dp),
                    )
                }
            }
            FlowRow(Modifier.padding(top = 18.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                RequestButtons(a, r)
                if (RequestRules.canRemove(r)) {
                    val wait = r.id in a.busy
                    SmallButton("Remove from list", iconRes = R.drawable.ic_close, enabled = !wait) { a.removeFromList(r) }
                    if (a.admin) SmallButton(RequestRules.filesLabel(r.status, r.hasFiles), iconRes = R.drawable.ic_delete, danger = true, enabled = !wait) { a.confirmFiles = r }
                }
            }
            if (RequestRules.canImportAgain(r)) Text(RequestRules.IMPORT_AGAIN_HELP, style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(top = 6.dp))
        }
    }
}

/**
 * The storage banner over a request list (common.rs `storage_banner`): imports are paused because a folder can't be
 * written. Asks GET /api/requests/storage when shown and every 30 s (every 5 s while imports are paused, so it goes as
 * soon as the request shows the import done, as the web's). Admins read the folder, the operation and the
 * error and get Check again (POST /api/requests/storage/check: "Imports resumed" or "Imports are still paused");
 * everyone else reads the plain words. Nothing while imports go on, or from a server without the check.
 */
@Composable
fun StorageBanner(nav: Nav, admin: Boolean, modifier: Modifier = Modifier, onResumed: () -> Unit = {}) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var state by remember { mutableStateOf<StorageHealth?>(null) }
    var checking by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        while (true) {
            try {
                state = withContext(Dispatchers.IO) { nav.features.requestStorage() }
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // offline or a server error: the banner keeps what it last knew
            }
            delay(if (state?.paused == true) 5_000 else 30_000)
        }
    }
    val text = RequestRules.storageBanner(state, admin) ?: return
    val shape = RoundedCornerShape(10.dp)
    Row(
        modifier
            .fillMaxWidth()
            .padding(horizontal = Gutter, vertical = 6.dp)
            .clip(shape)
            .background(EncoreColors.Warn.copy(alpha = 0.10f))
            .border(1.dp, EncoreColors.Warn.copy(alpha = 0.40f), shape)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Icon(painterResource(R.drawable.ic_warning), contentDescription = null, tint = EncoreColors.Warn, modifier = Modifier.padding(top = 1.dp).size(18.dp))
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            SelectionContainer { Text(text, style = MaterialTheme.typography.bodySmall, color = p.text) }
            if (admin) {
                Spacer(Modifier.height(8.dp))
                SmallButton(if (checking) "Checking…" else "Check again", iconRes = R.drawable.ic_refresh, enabled = !checking) {
                    checking = true
                    scope.launch {
                        try {
                            val s = withContext(Dispatchers.IO) { nav.features.checkStorage() }
                            state = s
                            val (msg, kind) = RequestRules.checkToast(s)
                            Toasts.show(msg, kind)
                            if (!s.paused) onResumed()
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            Toasts.show(e.message ?: "That didn't work", "err")
                        }
                        checking = false
                    }
                }
            }
        }
    }
}
