package app.encore.android.features.requests

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
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
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.PickerOpts
import app.encore.android.data.repo.RequestRow
import app.encore.android.data.repo.formatAgo
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.StatusBadge
import app.encore.android.ui.screens.features
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * What a list of request rows can do (web-rs/src/pages/common.rs `bind_request_actions`), one per list: Approve, Deny,
 * Retry and Cancel (POST /api/requests/{id}/{action}), Remove from list (DELETE /api/requests/{id}), the admin's
 * Delete files (`?files=true`, after a confirm), Pick (the picker, for this request) and the details sheet.
 * [me] is set by the screen once GET /api/auth/me has answered.
 */
@Stable
class RequestActions(val nav: Nav, private val scope: CoroutineScope, private val reload: () -> Unit) {
    var me by mutableStateOf<Me?>(null)
    val admin: Boolean get() = me?.isAdmin == true

    /** The row whose files the admin asked to delete (the confirm dialog is open). */
    internal var confirmFiles by mutableStateOf<RequestRow?>(null)

    /** The request whose details sheet is open (0: none). */
    internal var detailsFor by mutableStateOf(0L)

    /** Rows with an action on its way: their buttons wait (the web disables the button it pressed). */
    internal var busy by mutableStateOf(emptySet<Long>())

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
        if (RequestRules.canRemove(r)) MenuEntry("Remove from list", R.drawable.ic_close) { remove(r, false) } else null,
        if (RequestRules.canRemove(r) && admin) MenuEntry(RequestRules.filesLabel(r.status), R.drawable.ic_delete, danger = true) { confirmFiles = r } else null,
    )
}

@Composable
fun rememberRequestActions(nav: Nav, reload: () -> Unit): RequestActions {
    val scope = rememberCoroutineScope()
    val latest = rememberUpdatedState(reload)
    return remember(nav) { RequestActions(nav, scope) { latest.value() } }
}

/** The buttons of a request (common.rs `request_row`): Approve, Deny, Pick, Retry, Cancel – whichever apply. */
@Composable
private fun RequestButtons(a: RequestActions, r: RequestRow) {
    val wait = r.id in a.busy
    if (RequestRules.canApprove(r, a.admin)) {
        SmallButton("Approve", iconRes = R.drawable.ic_check, primary = true, enabled = !wait) { a.act(r, "approve") }
        SmallButton("Deny", danger = true, enabled = !wait) { a.act(r, "deny") }
    }
    if (RequestRules.canPick(r, a.me)) SmallButton("Pick", iconRes = R.drawable.ic_search) { a.pick(r) }
    if (RequestRules.canRetry(r, a.me)) SmallButton("Retry", iconRes = R.drawable.ic_refresh, enabled = !wait) { a.act(r, "retry") }
    if (RequestRules.canCancel(r)) SmallButton("Cancel", iconRes = R.drawable.ic_close, enabled = !wait) { a.act(r, "cancel") }
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
            text = RequestRules.filesConfirm(r.status),
            okLabel = "Delete",
            danger = true,
            onDismiss = { a.confirmFiles = null },
        ) { a.remove(r, true) }
    }
    if (a.detailsFor > 0L) {
        // the row as the list has it now (the list reloads every few seconds, so the sheet follows the request)
        val r = rows.firstOrNull { it.id == a.detailsFor }
        if (r != null) RequestDetailsSheet(a, r, onDismiss = { a.detailsFor = 0L })
        else LaunchedEffect(a.detailsFor) { a.detailsFor = 0L }
    }
}

/**
 * The details of one request: the whole status message (the row cuts it at two lines), what the row squeezes into its
 * status line, the candidate being fetched, and the worker's log – one line per source tried, with each source's own
 * error – and the request's buttons.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun RequestDetailsSheet(a: RequestActions, r: RequestRow, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val d = RequestRules.details(r, a.admin, formatAgo(r.createdAt))
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
                Badge(r.kind)
                StatusBadge(r.status)
            }
            if (r.isActive && r.progress > 0) ProgressBar(r.progress, Modifier.padding(top = 10.dp).fillMaxWidth())
            if (d.message.isNotEmpty()) {
                SelectionContainer {
                    Text(d.message, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(top = 12.dp))
                }
            }
            Column(Modifier.padding(top = 10.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                for (f in d.facts) Text(f, style = MaterialTheme.typography.bodySmall, color = p.muted)
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
            FlowRow(Modifier.padding(top = 18.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                RequestButtons(a, r)
                if (RequestRules.canRemove(r)) {
                    val wait = r.id in a.busy
                    SmallButton("Remove from list", iconRes = R.drawable.ic_close, enabled = !wait) { a.remove(r, false) }
                    if (a.admin) SmallButton(RequestRules.filesLabel(r.status), iconRes = R.drawable.ic_delete, danger = true, enabled = !wait) { a.confirmFiles = r }
                }
            }
        }
    }
}
