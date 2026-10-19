package app.encore.android.features.bugs

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Context
import android.content.Intent
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import app.encore.android.R
import app.encore.android.core.bool
import app.encore.android.core.parseJsonObject
import app.encore.android.core.str
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.features
import app.encore.android.data.repo.formatAgo
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.RowDivider
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.BackBar
import app.encore.android.ui.screens.EncoreField
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

// ---- Settings → This device → Bug reports ------------------------------------------------------------------------

/**
 * The entry on Settings → This device: "Bug reports" with the count of reports that have something new (the web's
 * side bar badge), refreshed whenever Settings shows it.
 */
@Composable
fun BugReportsEntry(nav: Nav, modifier: Modifier = Modifier) {
    val p = MaterialTheme.palette
    val unread by BugUnread.count.collectAsState()
    LaunchedEffect(Unit) { BugUnread.refresh(nav.graph) }
    Row(
        modifier.fillMaxWidth().clickable { nav.open(Screen.BugReports("Settings")) }.padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text("Bug reports")
            Text(
                "Report a problem or ask how something works",
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
            )
        }
        if (unread > 0) {
            Badge(unread.toString(), kind = "warn", modifier = Modifier.padding(end = 10.dp))
        }
        Icon(painterResource(R.drawable.ic_bug), contentDescription = null, tint = p.muted)
    }
}

// ---- the list's filter, kept on the phone (the web keeps it in localStorage) --------------------------------------

private const val PREFS = "encore_bugs"
private const val K_FILTER = "filter"

private fun loadFilter(context: Context): BugRules.Filter = try {
    val o = parseJsonObject(context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(K_FILTER, null))
    if (o == null) BugRules.Filter() else BugRules.Filter(o.str("status").ifEmpty { "all" }, o.str("kind"), o.str("q"), o.bool("mine"))
} catch (_: Exception) {
    BugRules.Filter()
}

private fun saveFilter(context: Context, f: BugRules.Filter) {
    val o: JsonObject = buildJsonObject {
        put("status", f.status)
        put("kind", f.kind)
        put("q", f.q)
        put("mine", f.mine)
    }
    try {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(K_FILTER, o.toString()).apply()
    } catch (_: Exception) {
    }
}

// ---- Bug reports -------------------------------------------------------------------------------------------------

/**
 * Bug reports (web-rs/src/pages/bugreports.rs `render`): the status tabs, the kind chips, the search, Everyone / Mine
 * and Export for admins, New report, and the list (newest activity first; a dot marks what is new since the last
 * look). Refreshes every minute while open, as the web does. [from] names the page it was opened from, which a new
 * report sends along.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun BugReportsScreen(nav: Nav, from: String) {
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var me by remember { mutableStateOf<Me?>(null) }
    var filter by remember { mutableStateOf(loadFilter(context)) }
    var rows by remember { mutableStateOf<List<BugReportRow>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var generation by remember { mutableStateOf(0) }
    var showNew by remember { mutableStateOf(false) }
    var exporting by remember { mutableStateOf(false) }
    val lastFilter = remember { arrayOfNulls<BugRules.Filter>(1) }
    val admin = me?.isAdmin == true

    suspend fun load() {
        try {
            val m = me ?: withContext(Dispatchers.IO) { nav.graph.features.me() }.also { me = it }
            val f = BugRules.normalize(filter, m.isAdmin)
            if (f != filter) filter = f
            rows = withContext(Dispatchers.IO) { nav.graph.bugReports.list(f, m.isAdmin) }
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (rows == null) error = BugRules.errorWords(e)
        }
    }

    LaunchedEffect(filter, generation) {
        // typing in the search waits a moment (the web's 250 ms), the tabs and chips don't
        val before = lastFilter[0]
        lastFilter[0] = filter
        if (before != null && before.q != filter.q) delay(250)
        saveFilter(context, filter)
        load()
    }
    LaunchedEffect(Unit) {
        // the badge agrees with the rows it stands next to; then the list keeps itself fresh
        BugUnread.refresh(nav.graph)
        while (true) {
            delay(60_000)
            load()
            BugUnread.refresh(nav.graph)
        }
    }

    fun export() {
        if (exporting) return
        exporting = true
        scope.launch {
            try {
                val day = SimpleDateFormat("yyyy-MM-dd", Locale.US).format(Date())
                val file = withContext(Dispatchers.IO) {
                    nav.graph.bugReports.export(BugRules.exportQuery(filter), File(exportDir(context), BugRules.exportName(day)))
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
        LazyColumn(Modifier.fillMaxSize()) {
            item {
                FlowRow(
                    Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (admin) {
                        Segmented(listOf("all" to "Everyone", "mine" to "Mine"), if (filter.mine) "mine" else "all") { v ->
                            filter = filter.copy(mine = v == "mine")
                        }
                        SmallButton(
                            if (exporting) "Exporting…" else "Export",
                            iconRes = R.drawable.ic_download,
                            enabled = !exporting,
                            modifier = Modifier.align(Alignment.CenterVertically),
                        ) { export() }
                    }
                    EncoreButton("New report", iconRes = R.drawable.ic_add, primary = true) { showNew = true }
                }
            }
            item {
                val tab = BugRules.TABS.indexOfFirst { it.first == filter.status }.coerceAtLeast(0)
                ChoiceRow(BugRules.TABS.map { it.second }, tab, { i -> filter = filter.copy(status = BugRules.TABS[i].first) })
            }
            item {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = Gutter),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    for ((key, label) in BugRules.KINDS) {
                        val on = filter.kind == key
                        Badge(label, kind = if (on) "accent" else "") {
                            filter = filter.copy(kind = if (on) "" else key)
                        }
                    }
                }
            }
            item {
                EncoreField(
                    filter.q,
                    { filter = filter.copy(q = it) },
                    placeholder = "Search reports",
                    leadingIcon = R.drawable.ic_search,
                    imeAction = ImeAction.Search,
                    modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 10.dp),
                )
            }
            val list = rows
            val err = error
            when {
                list == null && err != null -> item { Box(Modifier.height(260.dp)) { ErrorBox(err) { generation += 1 } } }
                list == null -> item {
                    Box(Modifier.fillMaxWidth().padding(40.dp), contentAlignment = Alignment.Center) {
                        CircularProgressIndicator(color = p.accent, trackColor = p.border)
                    }
                }
                list.isEmpty() -> item {
                    if (filter.active) {
                        EmptyState(R.drawable.ic_bug, "Nothing matches these filters.")
                    } else {
                        EmptyState(R.drawable.ic_bug, "No reports yet. New report writes the first one.")
                    }
                }
                else -> items(list, key = { it.id }) { r ->
                    ReportRow(r) { nav.open(Screen.BugReport(r.id)) }
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }

    if (showNew) {
        NewBugReportSheet(nav, page = from, onDismiss = { showNew = false })
    }
}

/** One report in the list: the new-since-last-look dot, the title, who and when, kind, status and the counts. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ReportRow(r: BugReportRow, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = Gutter, vertical = 12.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Box(
            Modifier
                .padding(top = 7.dp)
                .size(8.dp)
                .clip(CircleShape)
                .background(if (r.unread) EncoreColors.Warn else Color.Transparent)
                .then(if (r.unread) Modifier.semantics { contentDescription = "New since you last looked" } else Modifier),
        )
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            Text(
                r.title,
                style = MaterialTheme.typography.bodyLarge,
                fontWeight = FontWeight.SemiBold,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                "by ${r.username} · ${formatAgo(if (r.updatedAt > 0.0) r.updatedAt else r.createdAt)}",
                style = MaterialTheme.typography.bodySmall,
                color = p.faint,
            )
            FlowRow(
                Modifier.padding(top = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Badge(BugRules.kindLabel(r.kind), kind = BugRules.kindBadge(r.kind))
                Badge(BugRules.statusLabel(r.status), kind = BugRules.statusBadge(r.status))
                Text(
                    BugRules.countWords(r.messageCount, r.fileCount),
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    modifier = Modifier.align(Alignment.CenterVertically),
                )
            }
        }
    }
    RowDivider()
}

// ---- export: the zip goes to the share sheet ----------------------------------------------------------------------

/** Where exports wait to be shared: `cacheDir/shared/bug-reports` (the FileProvider's `shared` path). */
internal fun exportDir(context: Context): File = File(context.cacheDir, "shared/bug-reports").also { dir ->
    // one export at a time is enough: older ones go
    dir.listFiles()?.forEach { if (it.isFile) it.delete() }
}

/** Offers the zip to other apps (mail, a chat, Files) through Android's share sheet. */
internal fun shareZip(context: Context, file: File) {
    val uri = try {
        FileProvider.getUriForFile(context, context.packageName + ".files", file)
    } catch (_: IllegalArgumentException) {
        Toasts.show("This build of the app can't share files", "err")
        return
    }
    val send = Intent(Intent.ACTION_SEND)
        .setType("application/zip")
        .putExtra(Intent.EXTRA_STREAM, uri)
        .putExtra(Intent.EXTRA_SUBJECT, file.name)
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    send.clipData = ClipData.newRawUri(file.name, uri)
    try {
        context.startActivity(Intent.createChooser(send, "Export").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (_: ActivityNotFoundException) {
        Toasts.show("No app on this phone can take the file", "err")
    }
}
