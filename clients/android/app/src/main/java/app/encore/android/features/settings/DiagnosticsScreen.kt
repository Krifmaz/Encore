package app.encore.android.features.settings

import android.Manifest
import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
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
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.AppGraph
import app.encore.android.R
import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.obj
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.SettingsApi
import app.encore.android.data.api.settingsApi
import app.encore.android.data.repo.formatAgo
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.CopyIconButton
import app.encore.android.ui.components.DetailsDialog
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.RowDivider
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDay
import app.encore.android.ui.components.formatSize
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody
import okio.BufferedSink
import okio.source
import java.text.DateFormat
import java.util.Date

/**
 * Settings → Diagnostics (web-rs/src/settings/diagnostics.rs): the server's own update card first, right under
 * Copy report / Run checks (admins: Check now, Restart server, Update now, and the wait until the server is back);
 * then every connection, folder and tool check with the raw answers, the Server block and where settings come from
 * (folded away until tapped); the database backups (Back up now, Upload a backup…, Download into the phone's
 * Downloads folder, Restore…); and the phone app's crash reports.
 */
@Composable
fun DiagnosticsScreen(nav: Nav, onBack: () -> Unit) {
    val api = nav.graph.settingsApi
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var run by remember { mutableIntStateOf(0) }
    var data by remember { mutableStateOf<JsonObject?>(null) }
    var checking by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf("") }
    LaunchedEffect(run) {
        checking = true
        try {
            data = withContext(Dispatchers.IO) {
                val d = api.diagnostics()
                // Encore's VPN for Soulseek as one more connection check (older servers have no /system/vpn)
                val vpn = try { api.vpn() } catch (_: Exception) { null }
                if (vpn == null) d else {
                    val checks = (d.arr("checks") ?: kotlinx.serialization.json.JsonArray(emptyList())) + SettingsRules.vpnCheck(vpn)
                    JsonObject(d + ("checks" to kotlinx.serialization.json.JsonArray(checks)))
                }
            }
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
        checking = false
    }
    SettingsPage(
        "Diagnostics",
        onBack,
        subtitle = "Runs every connection, folder and tool check from inside the Encore container and shows the raw answers. Keys are hidden.",
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(bottom = 12.dp)) {
            EncoreButton("Copy report", iconRes = R.drawable.ic_content_copy, enabled = data != null) {
                data?.let { copyToClipboard(context, SettingsRules.diagnosticsReport(it, ::dateTime) { b -> formatSize(b.toLong()) }) }
            }
            EncoreButton("Run checks", iconRes = R.drawable.ic_refresh, primary = true, enabled = !checking) { run += 1 }
        }
        // Updates sit at the top: it is the part people come here for most, and it no longer hides under the checks
        ServerUpdateCard(api, onServerBack = { run += 1 })
        val d = data
        when {
            checking && d == null -> Row(verticalAlignment = Alignment.CenterVertically) { Spinner(18.dp); Spacer(Modifier.width(8.dp)); Muted("Checking…") }
            d == null -> ErrorBox(error) { run += 1 }
            else -> Checks(d)
        }
        Backups(nav.graph, api)
        Crashes(api)
    }
}

private fun dateTime(t: Double): String = if (t <= 0) "—" else DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date((t * 1000).toLong()))

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Checks(d: JsonObject) {
    val checks = d.arr("checks").objectsOf()
    val bad = checks.count { !it.bool("ok") }
    SetCard(Modifier.padding(bottom = 4.dp)) {
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Badge(if (bad > 0) "$bad problem${if (bad > 1) "s" else ""}" else "All good", kind = if (bad > 0) "err" else "ok")
            Muted("Encore ${d.str("version")} · running as uid ${SettingsRules.jsStr(d, "uid")} · ${dateTime(d.dbl("time"))}")
        }
    }
    d.obj("machine")?.let { m ->
        PageSection("Server", "What Encore sees of the machine it runs on")
        SetCard(flat = true) {
            for ((k, v) in SettingsRules.serverRows(m) { b -> formatSize(b.toLong()) }) {
                Row(Modifier.fillMaxWidth()) {
                    Muted(k, Modifier.width(150.dp))
                    Text(v, style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f))
                }
            }
        }
    }
    for ((kind, title) in SettingsRules.CHECK_KINDS) {
        val group = checks.filter { it.str("kind") == kind }
        if (group.isEmpty()) continue
        PageSection(title)
        SetCard(flat = true) {
            group.forEachIndexed { i, c ->
                if (i > 0) RowDivider(startIndent = 0.dp)
                CheckItem(c)
            }
        }
    }
    DownloadFolders(d, ::dateTime)
    SettingsSources(d)
}

/**
 * "Where settings come from": every setting key with its source badge (env VAR or saved in Encore) and its value.
 * The list is long, so it starts folded: the heading is a toggle with a chevron, and the rows show once it is tapped.
 * The open/closed choice lasts while the page is on screen (a Run checks keeps it).
 */
@Composable
private fun SettingsSources(d: JsonObject) {
    val p = MaterialTheme.palette
    var open by remember { mutableStateOf(false) }
    val rows = d.arr("settings").objectsOf()
    Column(Modifier.fillMaxWidth().clickable { open = !open }) {
        PageSection("Where settings come from", "Environment variables always win over values saved in Encore") {
            Icon(
                painterResource(R.drawable.ic_expand_more),
                contentDescription = if (open) "Hide settings" else "Show ${rows.size} settings",
                tint = p.muted,
                // the chevron points down while folded and flips up once the list is open
                modifier = Modifier.rotate(if (open) 180f else 0f),
            )
        }
    }
    if (!open) return
    SetCard(flat = true) {
        rows.forEachIndexed { i, s ->
            if (i > 0) RowDivider(startIndent = 0.dp)
            Column(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Mono(s.str("key"), Modifier.weight(1f))
                    if (s.str("source") == "env") Badge("env ${s.str("env")}", kind = "info") else Badge("saved in Encore")
                }
                Mono(SettingsRules.jsStr(s, "value"), color = p.muted)
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun CheckItem(c: JsonObject) {
    val p = MaterialTheme.palette
    val ok = c.bool("ok")
    var raw by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.Top) {
        Dot(ok, Modifier.padding(top = 6.dp))
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(c.str("name"), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold)
                if (SchemaRules.truthy(c["status"])) Badge("HTTP ${c.str("status")}", kind = if (ok) "ok" else "err")
                if (!SettingsRules.nullish(c, "ms")) Muted("${c.str("ms")} ms", color = p.faint)
            }
            if (c.str("request").isNotEmpty()) Mono(c.str("request"), color = p.faint)
            Muted(c.str("detail"), color = if (ok) p.muted else EncoreColors.Err)
            if (c.str("body").isNotEmpty()) {
                TextLink("Raw response", color = p.muted) { raw = !raw }
                if (raw) Mono(c.str("body").take(4000), color = p.muted)
            }
        }
    }
}

// ---- the server's update ------------------------------------------------------------------------------------------

/** The README section on "Update now" (the Docker socket mount) and "Restart server". */
private const val README_UPDATES = "https://github.com/Krifmaz/Encore#updates-and-restart"

/** What the card shows under its buttons while a restart or an update runs. */
private class Status(val text: String, val spinner: Boolean = false, val error: Boolean = false, val back: Boolean = false, val reload: Boolean = false)

/**
 * The Updates card (admins; the server answers 403 to anyone else and an older server 404: then nothing shows):
 * this server's version, the newest release with its notes, Check now, Restart server and Update now when the
 * server can update itself; after a restart or an update it asks /api/health every 2 s until the server is back.
 */
@Composable
fun ServerUpdateCard(api: SettingsApi, onServerBack: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var load by remember { mutableIntStateOf(0) }
    var forceNext by remember { mutableStateOf(false) }
    var d by remember { mutableStateOf<JsonObject?>(null) }
    var hidden by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }
    var checking by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var status by remember { mutableStateOf<Status?>(null) }
    var notesOpen by remember { mutableStateOf(false) }
    var askRestart by remember { mutableStateOf(false) }
    var askUpdate by remember { mutableStateOf(false) }
    LaunchedEffect(load) {
        val force = forceNext
        forceNext = false
        try {
            d = withContext(Dispatchers.IO) { api.serverUpdate(force) }
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: ApiException) {
            if (e.code == 403 || e.code == 404) hidden = true else error = e.message ?: ""
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
        checking = false
    }
    if (hidden) return

    suspend fun waitBack(w: SettingsRules.Wait, from: String) {
        val start = System.currentTimeMillis()
        var down = false
        var answeringSince: Long? = null
        var late = false
        while (true) {
            delay(SettingsRules.POLL_MS)
            val version = withContext(Dispatchers.IO) { api.health() }
            val now = System.currentTimeMillis()
            if (version == null) { down = true; answeringSince = null } else if (answeringSince == null) answeringSince = now
            val answeringFor = answeringSince?.let { now - it } ?: 0L
            when (val b = SettingsRules.judge(w, from, version, down, answeringFor, now - start)) {
                is SettingsRules.Back.Running -> { status = Status("Encore ${b.version} is running", back = true, reload = true); return }
                SettingsRules.Back.SameVersion -> {
                    status = Status("The server is back, still on Encore $from: the update did not go through. Try Update now again, or update by hand (docker compose pull && docker compose up -d).", error = true, reload = true)
                    return
                }
                SettingsRules.Back.Late -> if (!late) { late = true; status = Status("The server did not come back yet – check docker ps", spinner = true) }
                SettingsRules.Back.Waiting -> {}
            }
            if (now - start >= SettingsRules.GIVE_UP_MS) {
                status = Status("The server did not come back yet – check docker ps", error = true, reload = true)
                return
            }
        }
    }

    PageSection("Updates") {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            SmallButton(if (checking) "Checking…" else "Check now", iconRes = R.drawable.ic_refresh, enabled = !checking && !busy) {
                checking = true
                forceNext = true
                load += 1
            }
            SmallButton("Restart server", enabled = !busy) { askRestart = true }
        }
    }
    val u = d
    if (u == null) {
        if (error.isNotEmpty()) SetCard { Text(error, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err) } else Spinner(18.dp)
        return
    }
    val current = u.str("current")
    val latest = u.str("latest")
    val newer = SchemaRules.truthy(u["is_newer"]) && latest.isNotEmpty()
    val dev = u.bool("dev")
    val ready = u.str("self_update") == "ready"
    SetCard {
        Text("Encore $current" + if (dev) " · development build" else "", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            when {
                latest.isEmpty() -> if (u.str("error").isEmpty()) Muted("Not checked yet – press Check now")
                newer -> {
                    Badge("Encore $latest is available", kind = "info")
                    val day = publishedDay(u.str("published_at"))
                    if (day.isNotEmpty()) Muted("released $day")
                }
                else -> Muted("You have the newest version")
            }
            if (u.dbl("checked_at") > 0) Muted("checked ${formatAgo(u.dbl("checked_at"))}", color = p.faint)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (u.str("url").isNotEmpty()) SmallButton("View on GitHub", iconRes = R.drawable.ic_open_in_new) { openSignInTab(context, u.str("url"), p.bg) }
            if (ready && newer) SmallButton("Update now", primary = true, enabled = !busy) { askUpdate = true }
        }
        if (dev) Muted("A build without a release number: any release counts as newer.")
        if (u.str("error").isNotEmpty()) Text("Couldn't check for updates: ${u.str("error")}", style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err)
        val notes = SettingsRules.notesText(u.str("notes"))
        if (notes.isNotEmpty()) {
            TextLink("What changed in $latest", color = p.muted) { notesOpen = !notesOpen }
            if (notesOpen) Text(notes + if (u.bool("notes_cut")) "\n…" else "", style = MaterialTheme.typography.bodySmall)
        }
        if (!ready) {
            val reason = u.str("self_update_reason").ifEmpty { "The Docker socket is not mounted" }.trimEnd('.')
            Muted("Update now is not available here: $reason.")
            TextLink("How to turn it on") { openSignInTab(context, README_UPDATES, p.bg) }
        }
        status?.let { s ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                when {
                    s.spinner -> Spinner(14.dp)
                    s.back -> Dot(true)
                    s.error -> Dot(false)
                }
                Column(Modifier.weight(1f)) {
                    if (s.back) Text("Back", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold)
                    Text(s.text, style = MaterialTheme.typography.bodySmall, color = if (s.error && !s.reload) EncoreColors.Err else p.muted)
                }
                // the web reloads the page: here the card and the checks are asked for again
                if (s.reload) SmallButton("Reload", primary = s.back) {
                    status = null
                    busy = false
                    load += 1
                    onServerBack()
                }
            }
        }
    }
    if (askRestart) {
        ConfirmDialog("Restart the server?", "Playback stops for about ten seconds.", "Restart", danger = true, onDismiss = { askRestart = false }) {
            busy = true
            status = Status("Restarting – waiting for the server…", spinner = true)
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { api.restartServer() }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Toasts.show(e.message ?: "Something went wrong", "err")
                    status = null
                    busy = false
                    return@launch
                }
                waitBack(SettingsRules.Wait.RESTART, current)
            }
        }
    }
    if (askUpdate) {
        ConfirmDialog("Update to $latest?", "The container is replaced by a new one with the same settings; this takes a minute.", "Update now", onDismiss = { askUpdate = false }) {
            busy = true
            status = Status("Downloading Encore $latest – this can take a few minutes…", spinner = true)
            scope.launch {
                try {
                    val r = withContext(Dispatchers.IO) { api.serverUpdateNow(latest) }
                    status = Status(r.str("message").ifEmpty { "Updating to $latest; the server restarts in a moment" }, spinner = true)
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    status = Status(e.message ?: "Something went wrong", error = true)
                    busy = false
                    return@launch
                }
                waitBack(SettingsRules.Wait.UPDATE, current)
            }
        }
    }
}

/** GitHub's `published_at` (an ISO date) as "Oct 12, 2026" ("" when missing). */
private fun publishedDay(iso: String): String {
    if (iso.isEmpty()) return ""
    return try { formatDay(java.time.Instant.parse(iso).epochSecond.toDouble()) } catch (_: Exception) { "" }
}

// ---- backups ---------------------------------------------------------------------------------------------------------

@Composable
private fun Backups(graph: AppGraph, api: SettingsApi) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var list by remember { mutableStateOf<List<JsonObject>?>(null) }
    var error by remember { mutableStateOf("") }
    var working by remember { mutableStateOf("") }
    var making by remember { mutableStateOf(false) }
    var restore by remember { mutableStateOf<String?>(null) }
    var version by remember { mutableIntStateOf(0) }
    LaunchedEffect(version) {
        try {
            list = withContext(Dispatchers.IO) { api.backups() }.arr("backups").objectsOf()
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
        working = ""
    }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        working = "Uploading and checking…"
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { api.uploadBackup(displayName(context, uri), streamBody(context, uri)) }
                Toasts.show("Saved as backups/${r.obj("created")?.str("name").orEmpty()}", "ok")
                list = r.arr("backups").objectsOf()
                working = ""
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message?.ifEmpty { null } ?: "Upload failed", "err")
                version += 1
            }
        }
    }
    // Download (BackupDownload.kt): on Android 8–9 the Downloads folder needs the storage permission, asked first
    var downloadAfterAsking by remember { mutableStateOf<String?>(null) }
    val askStorage = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        val name = downloadAfterAsking
        downloadAfterAsking = null
        if (name == null) return@rememberLauncherForActivityResult
        if (granted) BackupDownload.start(graph, api, name) else Toasts.show("Encore needs access to the phone's storage to save the backup in Downloads", "err")
    }
    fun download(name: String) {
        if (BackupDownload.needsPermission(context)) {
            downloadAfterAsking = name
            askStorage.launch(Manifest.permission.WRITE_EXTERNAL_STORAGE)
        } else {
            BackupDownload.start(graph, api, name)
        }
    }
    PageSection("Backups") {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            SmallButton("Upload a backup…", iconRes = R.drawable.ic_upload, enabled = working.isEmpty()) {
                pick.launch(arrayOf("application/vnd.sqlite3", "application/x-sqlite3", "application/octet-stream", "*/*"))
            }
            SmallButton("Back up now", primary = true, enabled = !making && working.isEmpty()) {
                making = true
                scope.launch {
                    try {
                        val r = withContext(Dispatchers.IO) { api.backupNow() }
                        Toasts.show("Backed up to backups/${r.obj("created")?.str("name").orEmpty()}", "ok")
                        list = r.arr("backups").objectsOf()
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "Something went wrong", "err")
                    }
                    making = false
                }
            }
        }
    }
    SetCard(flat = true) {
        Muted("Encore keeps copies of its database here (daily, and before every upgrade). Download one now and then to keep it somewhere else.")
        val l = list
        when {
            working.isNotEmpty() -> Row(verticalAlignment = Alignment.CenterVertically) { Spinner(16.dp); Spacer(Modifier.width(8.dp)); Muted(working) }
            l == null && error.isNotEmpty() -> Muted(error)
            l == null -> Spinner(16.dp)
            l.isEmpty() -> Muted("No backups yet – press Back up now, or check the log: Encore takes one at every start when none exists.")
            else -> l.forEachIndexed { i, b ->
                if (i > 0) RowDivider(startIndent = 0.dp)
                val name = b.str("name")
                Column(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Mono(name)
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Muted("${formatSize(b.dbl("size").toLong())} · ${dateTime(b.dbl("time"))}")
                        Badge(b.str("kind"))
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        SmallButton("Download", iconRes = R.drawable.ic_download) { download(name) }
                        SmallButton("Restore…", iconRes = R.drawable.ic_restore) { restore = name }
                    }
                }
            }
        }
    }
    restore?.let { name ->
        ConfirmDialog(
            "Restore this backup?",
            "Encore replaces its current database with $name. A safety copy of the current data is taken first, then the server reopens the database: everything saved since that backup (library index, requests, playlists, settings and sign-ins) is replaced, and this page reloads – you may have to sign in again.",
            "Restore",
            danger = true,
            onDismiss = { restore = null },
        ) {
            working = "Restoring – the server reopens its database, this page reloads in a moment…"
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { api.restoreBackup(name) }
                    Toasts.show("Backup restored", "ok")
                    delay(2500)
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Toasts.show(e.message ?: "Something went wrong", "err")
                }
                version += 1
            }
        }
    }
}

/** The picked file's name ("backup.db" when the provider does not say). */
private fun displayName(context: Context, uri: Uri): String = try {
    context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
        if (c.moveToFirst()) c.getString(0) else null
    }
} catch (_: Exception) {
    null
}?.takeIf { it.isNotBlank() } ?: "backup.db"

/** The picked file streamed into the upload (a database can be gigabytes: it is never held in memory). */
private fun streamBody(context: Context, uri: Uri): RequestBody = object : RequestBody() {
    override fun contentType(): MediaType = "application/octet-stream".toMediaType()
    override fun writeTo(sink: BufferedSink) {
        val input = context.contentResolver.openInputStream(uri) ?: throw java.io.IOException("Couldn't read the file")
        input.source().use { sink.writeAll(it) }
    }
}


// ---- the phone app's crashes -------------------------------------------------------------------------------------------

@Composable
private fun Crashes(api: SettingsApi) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var version by remember { mutableIntStateOf(0) }
    var list by remember { mutableStateOf<List<JsonObject>?>(null) }
    var error by remember { mutableStateOf("") }
    var asking by remember { mutableStateOf(false) }
    // the crash opened whole: (kind, text)
    var whole by remember { mutableStateOf<Pair<String, String>?>(null) }
    LaunchedEffect(version) {
        try {
            list = withContext(Dispatchers.IO) { api.crashes() }.arr("crashes").objectsOf()
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    PageSection("App crashes") { SmallButton("Clear", enabled = !list.isNullOrEmpty()) { asking = true } }
    SetCard(flat = true) {
        Muted("The phone app sends a report the next time it opens after it crashed or the system closed it. The newest 50 are kept.")
        val l = list
        when {
            l == null && error.isNotEmpty() -> Muted(error)
            l == null -> Spinner(16.dp)
            l.isEmpty() -> Muted("No app crashes reported.")
            else -> l.forEachIndexed { i, c ->
                if (i > 0) RowDivider(startIndent = 0.dp)
                var open by remember(c) { mutableStateOf(false) }
                val first = c.str("first_line").ifEmpty { "Crash" }
                val kind = SettingsRules.crashKind(c.str("kind"))
                val meta = listOf("device", "app_version", "os", "user").map { c.str(it) }.filter { it.isNotEmpty() }
                // the whole report to read and copy: the cut-off first line opens it, Copy takes all of it
                val full = SettingsRules.crashText(first, kind, meta.joinToString(" · "), dateTime(c.dbl("time")), c.str("text"))
                Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.Top) {
                    Dot(false, Modifier.padding(top = 6.dp))
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text(
                                first, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold, maxLines = 2,
                                modifier = Modifier.weight(1f, fill = false).clickable(onClickLabel = "Details") { whole = kind to full },
                            )
                            Badge(kind)
                            CopyIconButton(full, size = 28.dp)
                        }
                        Muted("${meta.joinToString(" · ")} · ${dateTime(c.dbl("time"))}")
                        TextLink("Full report", color = p.muted) { open = !open }
                        if (open) Mono(c.str("text").take(20000), color = p.muted)
                    }
                }
            }
        }
    }
    whole?.let { (kind, text) -> DetailsDialog(kind, text, onDismiss = { whole = null }) }
    if (asking) {
        ConfirmDialog("Clear app crashes?", "All crash reports from the phone app are removed.", "Clear", danger = true, onDismiss = { asking = false }) {
            scope.serverCall(after = { _: Unit -> version += 1 }, onError = { msg -> Toasts.show(msg, "err"); version += 1 }) { api.clearCrashes() }
        }
    }
}
