package app.encore.android.features.bugs

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import app.encore.android.R
import app.encore.android.data.repo.features
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatSize
import app.encore.android.ui.screens.BackBar
import app.encore.android.ui.screens.CardBox
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.EncoreField
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import coil3.compose.AsyncImage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.text.DateFormat
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** The web's `fmt.date`: the date and time in the phone's language; "—" for nothing. */
private fun formatDateTime(epochSeconds: Double): String {
    if (epochSeconds <= 0.0) return "—"
    return DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date((epochSeconds * 1000).toLong()))
}

/** What a report's page shows: the report and its thread, and whether the viewer is an admin. */
private class Loaded(val detail: BugReportDetail, val admin: Boolean)

/**
 * One report (web-rs/src/pages/bugreports.rs `render_one`): kind, title and status (a status select for admins), who
 * filed it, when, from which page, app version and device; the description with its pictures; the replies; the reply
 * box with pictures. Admins also get Export this report and Delete. Opening it marks it seen on the server.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun BugReportScreen(nav: Nav, id: Long) {
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var data by remember(id) { mutableStateOf<Loaded?>(null) }
    var error by remember(id) { mutableStateOf<String?>(null) }
    var generation by remember(id) { mutableStateOf(0) }
    var reply by remember(id) { mutableStateOf("") }
    var sending by remember(id) { mutableStateOf(false) }
    var exporting by remember(id) { mutableStateOf(false) }
    var confirmDelete by remember(id) { mutableStateOf(false) }
    var viewing by remember(id) { mutableStateOf<BugFile?>(null) }
    val picker = rememberAttachmentPicker()

    LaunchedEffect(id, generation) {
        try {
            val loaded = withContext(Dispatchers.IO) {
                val d = nav.graph.bugReports.detail(id)
                val admin = try { nav.graph.features.me().isAdmin } catch (_: Exception) { data?.admin ?: false }
                Loaded(d, admin)
            }
            data = loaded
            error = null
            // the server marked it seen: the badge may drop
            BugUnread.refresh(nav.graph)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (data == null) error = BugRules.errorWords(e) else Toasts.show(BugRules.errorWords(e), "err")
        }
    }

    fun sendReply() {
        val text = reply.trim()
        val files = picker.files.toList()
        if (text.isEmpty() && files.isEmpty()) {
            Toasts.show("Write a reply or add a file", "err")
            return
        }
        if (picker.busy) return
        sending = true
        scope.launch {
            try {
                withContext(Dispatchers.IO) { nav.graph.bugReports.reply(id, text, files, context.contentResolver) }
                Toasts.show("Reply sent", "ok")
                reply = ""
                picker.clear()
                generation += 1
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(BugRules.errorWords(e), "err")
            } finally {
                sending = false
            }
        }
    }

    fun setStatus(status: String) {
        scope.launch {
            try {
                withContext(Dispatchers.IO) { nav.graph.bugReports.setStatus(id, status) }
                Toasts.show("Status: ${BugRules.statusLabel(status)}", "ok")
                generation += 1
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(BugRules.errorWords(e), "err")
            }
        }
    }

    fun delete() {
        scope.launch {
            try {
                withContext(Dispatchers.IO) { nav.graph.bugReports.delete(id) }
                Toasts.show("Report deleted", "ok")
                BugUnread.refresh(nav.graph)
                nav.back()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(BugRules.errorWords(e), "err")
            }
        }
    }

    fun export() {
        if (exporting) return
        exporting = true
        scope.launch {
            try {
                val day = SimpleDateFormat("yyyy-MM-dd", Locale.US).format(Date())
                val file = withContext(Dispatchers.IO) {
                    nav.graph.bugReports.export(BugRules.exportOneQuery(id), File(exportDir(context), BugRules.exportName(day, id)))
                }
                shareZip(context, file)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(BugRules.errorWords(e), "err")
            } finally {
                exporting = false
            }
        }
    }

    Column(Modifier.fillMaxSize()) {
        BackBar("Bug reports", nav.back)
        val loaded = data
        val err = error
        when {
            loaded == null && err != null -> ErrorBox(err) { generation += 1 }
            loaded == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(color = p.accent, trackColor = p.border)
            }
            else -> {
                val r = loaded.detail.report ?: return@Column
                val admin = loaded.admin
                LazyColumn(Modifier.fillMaxSize()) {
                    item {
                        Column(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                Badge(BugRules.kindLabel(r.kind), kind = BugRules.kindBadge(r.kind), modifier = Modifier.align(Alignment.CenterVertically))
                                if (admin) {
                                    StatusSelect(r.status) { s -> if (s != r.status) setStatus(s) }
                                } else {
                                    Badge(BugRules.statusLabel(r.status), kind = BugRules.statusBadge(r.status), modifier = Modifier.align(Alignment.CenterVertically))
                                }
                            }
                            Text(r.title, style = MaterialTheme.typography.headlineSmall)
                            Text(
                                BugRules.metaLine(r.username, formatDateTime(r.createdAt), r.page, r.appVersion, r.device),
                                style = MaterialTheme.typography.bodySmall,
                                color = p.muted,
                            )
                            if (admin) {
                                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    SmallButton(if (exporting) "Exporting…" else "Export this report", iconRes = R.drawable.ic_download, enabled = !exporting) { export() }
                                    DangerButton("Delete", R.drawable.ic_delete) { confirmDelete = true }
                                }
                            }
                        }
                    }
                    item {
                        Box(Modifier.padding(horizontal = Gutter, vertical = 8.dp)) {
                            CardBox {
                                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                    SelectionContainer { Text(r.body, style = MaterialTheme.typography.bodyMedium) }
                                    FileList(r.files, nav) { viewing = it }
                                }
                            }
                        }
                    }
                    item { SectionHead("Replies") }
                    val messages = loaded.detail.messages
                    if (messages.isEmpty()) {
                        item {
                            Text(
                                "No replies yet.",
                                style = MaterialTheme.typography.bodySmall,
                                color = p.muted,
                                modifier = Modifier.padding(horizontal = Gutter, vertical = 8.dp),
                            )
                        }
                    } else {
                        items(messages, key = { it.id }) { m -> MessageItem(m, nav) { viewing = it } }
                    }
                    item {
                        Column(
                            Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 14.dp),
                            verticalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            EncoreField(
                                reply,
                                { reply = it.take(BugRules.MAX_BODY) },
                                placeholder = "Write a reply",
                                singleLine = false,
                                minLines = 3,
                                imeAction = androidx.compose.ui.text.input.ImeAction.Default,
                                modifier = Modifier.fillMaxWidth(),
                            )
                            AttachmentThumbs(picker, nav.graph.imageLoader)
                            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                                SmallButton("Add images", iconRes = R.drawable.ic_add, enabled = !sending) { picker.add() }
                                Spacer(Modifier.weight(1f))
                                EncoreButton("Send", primary = true, enabled = !sending && !picker.busy) { sendReply() }
                            }
                        }
                    }
                    item { Spacer(Modifier.height(24.dp)) }
                }
            }
        }
    }

    if (confirmDelete) {
        ConfirmDialog(
            "Delete this report?",
            "The report, its replies and its files are removed for good.",
            "Delete",
            danger = true,
            onDismiss = { confirmDelete = false },
            onConfirm = { delete() },
        )
    }
    viewing?.let { f -> FileViewer(nav, f, onDismiss = { viewing = null }) }
}

/** One reply: who (with "Admin" for an admin), when, the text and its files. */
@Composable
private fun MessageItem(m: BugMessage, nav: Nav, onOpen: (BugFile) -> Unit) {
    val p = MaterialTheme.palette
    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(m.username, fontWeight = FontWeight.Bold, style = MaterialTheme.typography.bodyMedium)
            if (m.isAdmin) Badge("Admin", kind = "accent")
            Text(formatDateTime(m.createdAt), style = MaterialTheme.typography.bodySmall, color = p.faint)
        }
        if (m.body.isNotEmpty()) SelectionContainer { Text(m.body, style = MaterialTheme.typography.bodyMedium) }
        FileList(m.files, nav, onOpen)
    }
    Box(Modifier.fillMaxWidth().padding(start = Gutter).height(1.dp).background(p.border))
}

/** A report's or a reply's files: pictures as squares, text files as a line with the name and size. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun FileList(files: List<BugFile>, nav: Nav, onOpen: (BugFile) -> Unit) {
    if (files.isEmpty()) return
    val p = MaterialTheme.palette
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (f in files) {
            if (f.isImage) {
                AsyncImage(
                    // the server's paths are relative ("/api/bug-reports/…"); the loader carries the session
                    model = nav.graph.api.url(f.url),
                    contentDescription = f.name,
                    imageLoader = nav.graph.imageLoader,
                    contentScale = ContentScale.Crop,
                    modifier = Modifier
                        .size(96.dp)
                        .clip(RoundedCornerShape(8.dp))
                        .background(p.panel2)
                        .border(1.dp, p.border, RoundedCornerShape(8.dp))
                        .clickable { onOpen(f) },
                )
            } else {
                Row(
                    Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .background(p.panel2)
                        .border(1.dp, p.border, RoundedCornerShape(8.dp))
                        .clickable { onOpen(f) }
                        .padding(horizontal = 10.dp, vertical = 7.dp),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(painterResource(R.drawable.ic_open_in_new), contentDescription = null, tint = p.muted, modifier = Modifier.size(16.dp))
                    Text(f.name, style = MaterialTheme.typography.bodySmall)
                    Text(formatSize(f.size), style = MaterialTheme.typography.bodySmall, color = p.faint)
                }
            }
        }
    }
}

/** A picture full size (tap to close), or a text file's text. */
@Composable
private fun FileViewer(nav: Nav, f: BugFile, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    if (f.isImage) {
        Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
            Box(
                Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.92f)).clickable(onClick = onDismiss),
                contentAlignment = Alignment.Center,
            ) {
                AsyncImage(
                    model = nav.graph.api.url(f.url),
                    contentDescription = f.name,
                    imageLoader = nav.graph.imageLoader,
                    contentScale = ContentScale.Fit,
                    modifier = Modifier.fillMaxSize().padding(8.dp),
                )
            }
        }
        return
    }
    var text by remember(f.url) { mutableStateOf<String?>(null) }
    LaunchedEffect(f.url) {
        text = try {
            withContext(Dispatchers.IO) { nav.graph.bugReports.text(f.url) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            BugRules.errorWords(e)
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.text,
        title = { Text(f.name) },
        text = {
            val t = text
            if (t == null) {
                CircularProgressIndicator(color = p.accent, trackColor = p.border)
            } else {
                Box(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
                    SelectionContainer {
                        Text(t, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Close", color = p.text) } },
    )
}

/** The status select of an admin (the web's `#bug-status`). */
@Composable
private fun StatusSelect(status: String, onSelect: (String) -> Unit) {
    val p = MaterialTheme.palette
    var open by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier
                .clip(RoundedCornerShape(8.dp))
                .background(p.bg2)
                .border(1.dp, p.border, RoundedCornerShape(8.dp))
                .clickable { open = true }
                .padding(start = 10.dp, end = 6.dp, top = 5.dp, bottom = 5.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(BugRules.statusLabel(status), style = MaterialTheme.typography.labelLarge)
            Icon(painterResource(R.drawable.ic_expand_more), contentDescription = "Status", tint = p.muted, modifier = Modifier.size(18.dp))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = p.panel) {
            for ((key, label, _) in BugRules.STATUSES) {
                DropdownMenuItem(
                    text = { Text(label, fontWeight = if (key == status) FontWeight.SemiBold else FontWeight.Normal) },
                    onClick = { open = false; onSelect(key) },
                )
            }
        }
    }
}

/** The web's `.btn.sm.danger`. */
@Composable
private fun DangerButton(label: String, iconRes: Int, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(8.dp)
    Row(
        Modifier
            .clip(shape)
            .background(EncoreColors.Err.copy(alpha = 0.10f))
            .border(1.dp, EncoreColors.Err.copy(alpha = 0.35f), shape)
            .clickable(onClick = onClick)
            .padding(horizontal = 11.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(painterResource(iconRes), contentDescription = null, tint = EncoreColors.Err, modifier = Modifier.size(16.dp))
        Text(label, style = MaterialTheme.typography.labelMedium, color = if (p.dark) Color(0xFFFCA5A5) else Color(0xFFB91C1C))
    }
}
