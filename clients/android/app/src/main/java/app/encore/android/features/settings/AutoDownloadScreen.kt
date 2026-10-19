package app.encore.android.features.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.arr
import app.encore.android.core.dbl
import app.encore.android.core.obj
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.settingsApi
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatNumber
import app.encore.android.ui.screens.Spinner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

private val SWITCHES: Map<String, Pair<String, String>> = mapOf(
    "auto_request_likes" to ("Liked songs" to "A song liked in Finamp, Plexamp, YouTube Music, Spotify, Tidal, Deezer… that isn’t in the library yet gets downloaded."),
    "discover_catalog_merge" to ("Show releases MusicBrainz is missing" to "Artist pages also list albums/EPs/singles found on Deezer and Apple Music that aren’t in MusicBrainz, so they can be requested."),
    "auto_request_followed" to ("New tracks in followed playlists" to "Playlists set to “Follow source” pull new tracks every sync and download the missing ones."),
    "auto_approve" to ("Skip the approval queue for everyone" to "Off: users’ manual requests wait for an admin. Auto-downloads for enabled users are always approved."),
    "quality_first" to ("Highest quality first" to "Try every lossless source (Qobuz/Tidal/Deezer, then FLAC on Soulseek – 24-bit preferred) before torrents/Usenet, MP3s and YouTube Music."),
    "upgrade_lossy" to ("Keep upgrading lossy albums" to "Periodically look for lossless copies of albums you only have as MP3/YouTube rips. Better files replace the old ones; playlists follow automatically."),
)

/**
 * Settings → Auto-download (web-rs/src/settings/autodownload.rs): what Encore downloads on its own, how it picks
 * quality and for whom; Save, Run now and Look for upgrades now (both save first, as on the web).
 */
@Composable
fun AutoDownloadScreen(nav: Nav, onBack: () -> Unit, onOpen: (String) -> Unit) {
    val api = nav.graph.settingsApi
    val scope = rememberCoroutineScope()
    var version by remember { mutableIntStateOf(0) }
    var data by remember { mutableStateOf<JsonObject?>(null) }
    var error by remember { mutableStateOf("") }
    // what the page holds: the settings by key (switches "true"/"false", the rest as typed) and the users' switches
    val values = remember { mutableStateMapOf<String, String>() }
    val users = remember { mutableStateMapOf<String, Boolean>() }
    LaunchedEffect(version) {
        try {
            val d = withContext(Dispatchers.IO) { api.autodownload() }
            data = d
            error = ""
            values.clear()
            d.obj("settings")?.forEach { (k, v) -> values[k] = SchemaRules.textOf(v) }
            users.clear()
            for (u in d.arr("users").objectsOf()) users[u.str("id")] = SchemaRules.truthy(u["auto_download"])
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    fun on(k: String): Boolean = values[k] == "true" || values[k] == "1"
    /** `collect()`: `{settings: {key: checked | value}, users: {id: checked}}`. */
    fun collect(): JsonObject {
        val s = LinkedHashMap<String, JsonElement>()
        for (k in SWITCHES.keys) if (values.containsKey(k)) s[k] = JsonPrimitive(on(k))
        for (k in listOf("discover_auto_count", "sync_interval_minutes", "upgrade_lossy_days", "album_upgrade")) if (values.containsKey(k)) s[k] = JsonPrimitive(values[k] ?: "")
        return JsonObject(mapOf("settings" to JsonObject(s), "users" to JsonObject(users.mapValues { JsonPrimitive(it.value) })))
    }

    SettingsPage("Auto-download", onBack, subtitle = "Decide what Encore downloads on its own, how it picks quality, and for whom.") {
        val d = data
        if (d == null) {
            if (error.isNotEmpty()) ErrorBox(error) { version += 1 } else Spinner(24.dp)
            return@SettingsPage
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(bottom = 12.dp)) {
            EncoreButton("Run now", iconRes = R.drawable.ic_refresh) {
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) {
                            api.putAutodownload(collect())
                            api.autodownloadRunNow()
                        }
                        Toasts.show("Checking likes and followed playlists now…", "ok")
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "Something went wrong", "err")
                    }
                }
            }
            EncoreButton("Save", iconRes = R.drawable.ic_check, primary = true) {
                scope.serverCall(ok = "Auto-download rules saved", after = { r: JsonObject -> data = r }) { api.putAutodownload(collect()) }
            }
        }
        val stats = d.obj("stats")
        StatTiles(
            listOf(
                formatNumber(stats?.dbl("auto_active")?.toLong() ?: 0) to "Auto-downloads in progress",
                formatNumber(stats?.dbl("album_waiting")?.toLong() ?: 0) to "Songs waiting on their album",
                formatNumber(stats?.dbl("lossy_albums")?.toLong() ?: 0) to "Albums only in lossy quality",
                formatNumber(stats?.dbl("upgrades_done")?.toLong() ?: 0) to "Upgraded to lossless",
            ),
        )
        SetCard(Modifier.padding(top = 16.dp)) {
            Text("What gets downloaded automatically", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            AdSwitch("auto_request_likes", ::on) { values["auto_request_likes"] = it.toString() }
            NumberLine("Auto-discover: from each user’s ListenBrainz Weekly Exploration, download the top", values["discover_auto_count"].orEmpty(), "new songs every week (0 = off)") { values["discover_auto_count"] = it }
            AdSwitch("discover_catalog_merge", ::on) { values["discover_catalog_merge"] = it.toString() }
            AdSwitch("auto_request_followed", ::on) { values["auto_request_followed"] = it.toString() }
            AdSwitch("auto_approve", ::on) { values["auto_approve"] = it.toString() }
            NumberLine("Check for new likes / playlist tracks every", values["sync_interval_minutes"].orEmpty(), "minutes (0 = never)") { values["sync_interval_minutes"] = it }
        }
        SetCard(Modifier.padding(top = 12.dp)) {
            Text("Quality", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Dropdown(
                "When a song is part of an album",
                listOf(
                    "album_ep" to "Download the whole album or EP it’s from (recommended)",
                    "album" to "Download the whole album (studio albums only)",
                    "off" to "Only download the song",
                ),
                values["album_upgrade"].orEmpty(),
            ) { values["album_upgrade"] = it }
            Muted("Uses the official studio release from MusicBrainz – never compilations, live albums or soundtracks. If the album doesn’t contain that exact version (e.g. a remix), the song is fetched on its own.")
            AdSwitch("quality_first", ::on) { values["quality_first"] = it.toString() }
            AdSwitch("upgrade_lossy", ::on) { values["upgrade_lossy"] = it.toString() }
            NumberLine("Retry each lossy album every", values["upgrade_lossy_days"].orEmpty(), "days") { values["upgrade_lossy_days"] = it }
            SmallButton("Look for upgrades now", iconRes = R.drawable.ic_refresh) {
                scope.launch {
                    try {
                        val r = withContext(Dispatchers.IO) {
                            api.putAutodownload(collect())
                            api.autodownloadUpgradeNow()
                        }
                        Toasts.show(if (SchemaRules.truthy(r["queued"])) "Looking for lossless copies of ${r.str("queued")} album(s)" else "Nothing to upgrade right now", "ok")
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "Something went wrong", "err")
                    }
                }
            }
            Muted("Which sources are searched, and in what order:")
            TextLink("Settings → Apps & sources → Download pipeline") { onOpen("pipeline") }
        }
        PageSection("Users", "Who auto-downloads are enabled for")
        SetCard(flat = true) {
            for (u in d.arr("users").objectsOf()) {
                val id = u.str("id")
                Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(u.str("username"), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold)
                            if (u.str("role") == "admin" || u.str("role") == "owner") {
                                androidx.compose.foundation.layout.Spacer(Modifier.width(6.dp))
                                Badge("admin")
                            }
                        }
                        Muted("Followed playlists: ${u.str("followed")} · Connected accounts: ${u.str("connections")}")
                    }
                    Switch(checked = users[id] == true, onCheckedChange = { users[id] = it })
                }
            }
        }
    }
}

@Composable
private fun AdSwitch(key: String, on: (String) -> Boolean, onChange: (Boolean) -> Unit) {
    val (label, help) = SWITCHES.getValue(key)
    SwitchRow(label, on(key), help = help, bold = true, onChange = onChange)
}

@Composable
private fun NumberLine(label: String, value: String, unit: String, onChange: (String) -> Unit) {
    Column(Modifier.padding(vertical = 4.dp)) {
        Muted(label)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FormField("", value, onChange, keyboard = KeyboardType.Number, modifier = Modifier.width(110.dp))
            Muted(unit)
        }
    }
}
