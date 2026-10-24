package app.encore.android.features.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.arr
import app.encore.android.core.dbl
import app.encore.android.core.long
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.settingsApi
import app.encore.android.data.repo.formatAgo
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.ReportUi
import app.encore.android.ui.components.RowDivider
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Settings → Blocked (web-rs/src/pages/report.rs): the quality check of every download, the reports with Try again
 * and Dismiss, the blocked sources with Undo, and the songs the quality checks flagged with Check again and Replace
 * (the phone's Report a problem sheet).
 */
@Composable
fun BlockedScreen(nav: Nav, admin: Boolean, onBack: () -> Unit) {
    val api = nav.graph.settingsApi
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var version by remember { mutableIntStateOf(0) }
    var values by remember { mutableStateOf<JsonObject?>(null) }
    var reports by remember { mutableStateOf<List<JsonObject>?>(null) }
    var blocks by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var tracks by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var error by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf("") }
    LaunchedEffect(version) {
        try {
            withContext(Dispatchers.IO) {
                val v = if (admin) api.adminSettings() else JsonObject(emptyMap())
                val r = api.reports().arr("reports").objectsOf()
                val b = if (admin) api.blocks().arr("blocks").objectsOf() else emptyList()
                val s = if (admin) api.suspect().arr("tracks").objectsOf() else emptyList()
                values = v
                reports = r
                blocks = b
                tracks = s
            }
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
        busy = ""
    }
    /** A row's button: the call, its toast, then the page again (the button stays usable after an error). */
    fun act(key: String, ok: String, call: () -> Unit) {
        busy = key
        scope.launch {
            try {
                withContext(Dispatchers.IO) { call() }
                if (ok.isNotEmpty()) Toasts.show(ok, "ok")
                version += 1
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Something went wrong", "err")
                busy = ""
            }
        }
    }

    SettingsPage("Blocked", onBack) {
        val r = reports
        if (r == null) {
            if (error.isNotEmpty()) ErrorBox(error) { version += 1 } else Spinner(24.dp)
            return@SettingsPage
        }
        SetCard(Modifier.padding(bottom = 16.dp)) {
            Text("Blocked", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Muted("When someone reports a song or album (⋯ → Report a problem), the exact source it came from is blocked – that Soulseek file or folder, torrent, Usenet post, streaming item or YouTube video – and the music is fetched again from somewhere else. Undo a block to allow that source again.")
            val v = values
            if (admin && v != null) {
                val locked = SchemaRules.source(v, "quality_check_on_import") == "env"
                SwitchRow(
                    "Check every download before it enters the library",
                    SchemaRules.truthy(v["quality_check_on_import"]),
                    enabled = !locked,
                    help = "Fake lossless (a lossy file saved as FLAC), upsampled hi-res, very quiet audio and the wrong length are turned away and the next copy is tried. The Quality scan under Scheduled tasks checks the songs already in the library.",
                ) { on ->
                    scope.serverCall(ok = "Saved", after = { res: JsonObject -> values = res }) { api.putSettings(JsonObject(mapOf("quality_check_on_import" to JsonPrimitive(on)))) }
                }
            }
        }
        SetCard(Modifier.padding(bottom = 16.dp)) {
            Text("Reports", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            if (r.isEmpty()) Muted("Nothing reported yet.")
            r.forEachIndexed { i, rep ->
                if (i > 0) RowDivider(startIndent = 0.dp)
                val id = rep.long("id")
                val st = rep.str("status")
                val (label, kind) = SettingsRules.reportStatus(st)
                val album = rep.str("kind") == "album" && rep.str("album_id").isNotEmpty()
                Column(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            "${rep.str("artist")} – ${rep.str("title")}",
                            style = MaterialTheme.typography.bodyMedium,
                            fontWeight = FontWeight.SemiBold,
                            color = if (album) p.accent else p.text,
                            modifier = Modifier.weight(1f).clickable(enabled = album) { nav.open(Screen.Album(rep.str("album_id"))) },
                        )
                        Badge(label, kind = kind)
                    }
                    Muted(if (rep.str("kind") == "album") "album" else "song")
                    Text(
                        rep.str("reason_label").ifEmpty { rep.str("reason") } + if (rep.str("note").isNotEmpty()) " · “${rep.str("note")}”" else "",
                        style = MaterialTheme.typography.bodySmall,
                    )
                    Muted(
                        (if (admin && rep.str("username").isNotEmpty()) "${rep.str("username")} · " else "") + formatAgo(rep.dbl("created_at")) +
                            (if (rep.str("source").isNotEmpty()) " · from ${rep.str("source")}" else "") +
                            (if (rep.str("message").isNotEmpty()) " · ${rep.str("message")}" else ""),
                        color = p.faint,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        if (SettingsRules.canRetry(st)) SmallButton("Try again", iconRes = R.drawable.ic_refresh, enabled = busy.isEmpty()) {
                            act("r$id", if (st == "lyrics_none") "Looking for other lyrics" else "Looking for a replacement") { api.retryReport(id) }
                        }
                        if (admin && SettingsRules.canDismiss(st)) SmallButton("Dismiss", enabled = busy.isEmpty()) {
                            act("d$id", "Dismissed") { api.dismissReport(id) }
                        }
                    }
                }
            }
        }
        if (admin) {
            SetCard(Modifier.padding(bottom = 16.dp)) {
                Text("Blocked sources", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                if (blocks.isEmpty()) Muted("No blocked sources.")
                blocks.forEachIndexed { i, b ->
                    if (i > 0) RowDivider(startIndent = 0.dp)
                    val id = b.long("id")
                    Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(b.str("label").ifEmpty { b.str("key") }, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 2)
                            Muted(
                                b.str("kind") +
                                    (if (b.str("reason_label").isNotEmpty()) " · ${b.str("reason_label")}" else "") +
                                    (if (b.str("title").isNotEmpty()) " · ${b.str("artist")} – ${b.str("title")}" else "") +
                                    (if (b.str("username").isNotEmpty()) " · ${b.str("username")}" else "") +
                                    " · ${formatAgo(b.dbl("created_at"))}",
                                color = p.faint,
                            )
                        }
                        SmallButton("Undo", enabled = busy.isEmpty()) { act("u$id", "Unblocked – that source may be used again") { api.unblock(id) } }
                    }
                }
            }
            SetCard {
                Text("Suspect quality", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                Muted("Songs the quality checks flagged. Replace one to block where it came from and get it again.")
                if (tracks.isEmpty()) Muted("Nothing flagged.")
                tracks.forEachIndexed { i, t ->
                    if (i > 0) RowDivider(startIndent = 0.dp)
                    val id = t.long("id")
                    Column(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(
                            "${t.str("artist")} – ${t.str("title")}",
                            style = MaterialTheme.typography.bodyMedium,
                            fontWeight = FontWeight.SemiBold,
                            color = p.accent,
                            modifier = Modifier.clickable { nav.open(Screen.Album(t.str("album_id"))) },
                        )
                        Text(t.str("summary"), style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn)
                        Muted(
                            t.str("format").uppercase() + (if (SchemaRules.truthy(t["bitrate"])) " ${t.str("bitrate")}k" else "") +
                                if (t.str("album").isNotEmpty()) " · ${t.str("album")}" else "",
                            color = p.faint,
                        )
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            SmallButton("Check again", iconRes = R.drawable.ic_refresh, enabled = busy.isEmpty()) {
                                busy = "c$id"
                                scope.launch {
                                    try {
                                        val q = withContext(Dispatchers.IO) { api.checkTrack(id) }
                                        val bad = q.str("status") == "suspect"
                                        Toasts.show(if (bad) q.str("summary") else "Looks fine now", if (bad) "err" else "ok")
                                        version += 1
                                    } catch (e: CancellationException) {
                                        throw e
                                    } catch (e: Exception) {
                                        Toasts.show(e.message ?: "Something went wrong", "err")
                                        busy = ""
                                    }
                                }
                            }
                            // the Report a problem sheet; it reloads this page when the report went through
                            SmallButton("Replace", primary = true) {
                                val codes = t.arr("issues").objectsOf().map { it.str("code") }
                                ReportUi.track(id, t.str("title"), SettingsRules.reasonFor(codes)) { version += 1 }
                            }
                        }
                    }
                }
            }
        }
    }
}
