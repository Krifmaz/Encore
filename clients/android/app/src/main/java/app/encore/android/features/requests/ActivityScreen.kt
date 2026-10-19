package app.encore.android.features.requests

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.repo.ActivityLine
import app.encore.android.data.repo.SlskTransfer
import app.encore.android.data.repo.SystemDownloads
import app.encore.android.data.repo.TorrentDownload
import app.encore.android.data.repo.formatAgo
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.DetailsDialog
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatSize
import app.encore.android.ui.screens.LoadingBox
import app.encore.android.ui.screens.features
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The Log's level filter (activity.rs `#lvl`): (level, label). */
private val LEVELS = listOf("" to "All levels", "info" to "Info", "warning" to "Warnings", "error" to "Errors")


/**
 * Activity, for admins (web-rs/src/pages/activity.rs, shown under the web's Downloader): the storage banner while
 * imports are paused, Soulseek transfers with "Clear completed" and slskd's reason for a failed one, torrent and
 * Usenet downloads, and the server log with a level filter. A tap on a log line, a transfer's error or a download's
 * message opens it whole with Copy. Reloads every 5 s, like the web.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ActivityScreen(nav: Nav) {
    val features = nav.features
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var downloads by remember { mutableStateOf<SystemDownloads?>(null) }
    var log by remember { mutableStateOf<List<ActivityLine>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var level by remember { mutableIntStateOf(0) }
    var generation by remember { mutableIntStateOf(0) }
    // the line opened whole: (title, text)
    var details by remember { mutableStateOf<Pair<String, String>?>(null) }
    val open: (String, String) -> Unit = { title, text -> details = title to text }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()

    suspend fun load() {
        try {
            val (d, l) = withContext(Dispatchers.IO) { features.systemDownloads() to features.activity() }
            downloads = d
            log = l
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (downloads == null) error = e.message ?: "That didn't work"
        }
    }
    LaunchedEffect(generation) { load() }
    LaunchedEffect(Unit) {
        while (true) {
            delay(5000)
            load()
        }
    }

    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("Activity", scrollBehavior = scrollBehavior, onBack = nav.back) },
    ) { padding ->
        val d = downloads
        val err = error
        LazyColumn(Modifier.fillMaxSize().padding(padding)) {
            // imports paused because a folder can't be written (the web shows it on the Downloader too)
            item(key = "storage-banner") { StorageBanner(nav, admin = true) }
            when {
                err != null -> item { Box(Modifier.fillMaxWidth().height(200.dp)) { ErrorBox(err) { error = null; generation += 1 } } }
                d == null -> item { Box(Modifier.fillMaxWidth().height(200.dp)) { LoadingBox() } }
                else -> {
                    item {
                        SectionHead("Soulseek transfers", action = "Clear completed") {
                            scope.launch {
                                try {
                                    withContext(Dispatchers.IO) { features.clearSoulseekTransfers() }
                                    generation += 1
                                } catch (e: CancellationException) {
                                    throw e
                                } catch (e: Exception) {
                                    Toasts.show(e.message ?: "That didn't work", "err")
                                }
                            }
                        }
                    }
                    when {
                        d.slskdError.isNotEmpty() -> item {
                            Text(d.slskdError, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err, modifier = Modifier.padding(horizontal = Gutter))
                        }
                        d.slskd.isEmpty() -> item { EmptyState(R.drawable.ic_download, "No Soulseek transfers", "slskd may not be configured.") }
                        else -> d.slskd.take(200).forEachIndexed { i, f -> item(key = "sl-$i") { TransferRow(f, open) } }
                    }
                    item { SectionHead("Torrents & Usenet") }
                    if (d.torrents.isEmpty()) {
                        item { EmptyState(R.drawable.ic_download, "Nothing downloading from torrents or Usenet") }
                    } else {
                        d.torrents.forEachIndexed { i, t -> item(key = "tor-$i") { TorrentRow(t, open) } }
                    }
                    item { SectionHead("Log") }
                    item { ChoiceRow(LEVELS.map { it.second }, level, onSelect = { level = it }) }
                    val want = LEVELS[level].first
                    val rows = log.filter { want.isEmpty() || it.level == want }.take(200)
                    if (rows.isEmpty()) {
                        item { Text("No log entries", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(horizontal = Gutter, vertical = 12.dp)) }
                    } else {
                        rows.forEachIndexed { i, l -> item(key = "log-$i") { LogRow(l, open) } }
                    }
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }
    details?.let { (title, text) -> DetailsDialog(title, text, onDismiss = { details = null }) }
}

@Composable
private fun Progress(percent: Double) {
    val p = MaterialTheme.palette
    Box(Modifier.padding(top = 6.dp).fillMaxWidth().height(5.dp).clip(RoundedCornerShape(99.dp)).background(p.panel2)) {
        Box(Modifier.fillMaxWidth((percent / 100.0).coerceIn(0.0, 1.0).toFloat()).fillMaxHeight().background(p.gradient))
    }
}

/** activity.rs `slskd_row`: file, folder · size, user, state, slskd's reason for a failed one, progress and speed. */
@Composable
private fun TransferRow(f: SlskTransfer, open: (String, String) -> Unit) {
    val p = MaterialTheme.palette
    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(f.filename, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            Spacer(Modifier.width(8.dp))
            Badge(f.state, kind = RequestRules.transferKind(f.state))
        }
        Text(
            listOf(f.directory, formatSize(f.size.toLong()), f.username).filter { it.isNotEmpty() }.joinToString(" · "),
            style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis,
        )
        if (f.error.isNotEmpty()) {
            Text(
                f.error, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err,
                modifier = Modifier.padding(top = 2.dp).clickable(onClickLabel = "Details") { open("Transfer error", RequestRules.transferErrorText(f)) },
            )
        }
        Progress(f.progress)
        val pct = if (f.progress == Math.floor(f.progress)) f.progress.toLong().toString() else "%.1f".format(f.progress)
        Text("$pct%" + if (f.speed > 0) " · ${formatSize(f.speed.toLong())}/s" else "", style = MaterialTheme.typography.bodySmall, color = p.faint)
    }
}

/** activity.rs `torrent_row`: artist – title, the message (a tap opens it whole) and the progress. */
@Composable
private fun TorrentRow(t: TorrentDownload, open: (String, String) -> Unit) {
    val p = MaterialTheme.palette
    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp)) {
        Text("${t.artist} – ${t.title}", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
        if (t.message.isNotEmpty()) {
            Text(
                t.message, style = MaterialTheme.typography.bodySmall, color = p.muted,
                modifier = Modifier.clickable(onClickLabel = "Details") { open("${t.artist} – ${t.title}", t.message) },
            )
        }
        Progress(t.progress)
    }
}

/** activity.rs `log_row`: level, message, how long ago; a tap opens the line with its level and time, with Copy. */
@Composable
private fun LogRow(l: ActivityLine, open: (String, String) -> Unit) {
    val p = MaterialTheme.palette
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClickLabel = "Details") { open("Log entry", RequestRules.logText(l.level, l.message, requestDate(l.ts))) }
            .padding(horizontal = Gutter, vertical = 6.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Badge(l.level, kind = if (l.level == "error") "err" else if (l.level == "warning") "warn" else "", modifier = Modifier.width(72.dp))
        Spacer(Modifier.width(10.dp))
        Text(l.message, style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f))
        Spacer(Modifier.width(8.dp))
        Text(formatAgo(l.ts), style = MaterialTheme.typography.bodySmall, color = p.faint)
    }
}
