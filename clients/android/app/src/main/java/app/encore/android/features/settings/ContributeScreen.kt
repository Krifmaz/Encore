package app.encore.android.features.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.obj
import app.encore.android.core.str
import app.encore.android.data.api.SettingsApi
import app.encore.android.data.api.settingsApi
import app.encore.android.ui.Nav
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatNumber
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Settings → Contribute (web-rs/src/pages/contribute.rs): AcoustID fingerprints and MusicBrainz ISRCs of songs the
 * audio confirmed (each with its switch, keys, numbers, last run, Send now and Save), and how albums missing from
 * MusicBrainz get added. Connect MusicBrainz opens MusicBrainz's sign-in in a browser tab; coming back, the page
 * looks again.
 */
@Composable
fun ContributeScreen(nav: Nav, onBack: () -> Unit) {
    val api = nav.graph.settingsApi
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val now = rememberNow()
    var version by remember { mutableIntStateOf(0) }
    var values by remember { mutableStateOf<JsonObject?>(null) }
    var d by remember { mutableStateOf<JsonObject?>(null) }
    var error by remember { mutableStateOf("") }
    var waitingForMb by remember { mutableStateOf(false) }
    // what was typed per key (switches "true"/"false"); secrets start empty: empty keeps the saved one
    val edits = remember { mutableStateMapOf<String, String>() }
    LaunchedEffect(version) {
        try {
            val (v, c) = withContext(Dispatchers.IO) { api.adminSettings() to api.contribute() }
            values = v
            d = c
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    // back from MusicBrainz's sign-in: look again
    OnResume { if (waitingForMb) { waitingForMb = false; version += 1 } }

    SettingsPage("Contribute", onBack) {
        val v = values
        val c = d
        if (v == null || c == null) {
            if (error.isNotEmpty()) ErrorBox(error) { version += 1 } else Spinner(24.dp)
            return@SettingsPage
        }
        val a = c.obj("acoustid")
        val m = c.obj("isrc")
        fun locked(k: String) = SchemaRules.source(v, k) == "env"
        fun save(part: List<String>, secrets: Set<String>, after: () -> Unit = {}) {
            val body = LinkedHashMap<String, JsonElement>()
            for (k in part) {
                if (locked(k)) continue
                val t = edits[k] ?: continue
                if (k in secrets) {
                    if (t.isBlank() || SchemaRules.isMasked(t)) continue
                    body[k] = JsonPrimitive(t.trim())
                } else if (t == "true" || t == "false") {
                    body[k] = JsonPrimitive(t == "true")
                } else {
                    body[k] = JsonPrimitive(t.trim())
                }
            }
            scope.serverCall(ok = "Settings saved", after = { _: JsonObject -> for (k in part) edits.remove(k); version += 1; after() }) { api.putSettings(JsonObject(body)) }
        }
        fun switchOn(k: String) = edits[k]?.let { it == "true" } ?: SchemaRules.truthy(v[k])

        SetCard(Modifier.padding(bottom = 16.dp)) {
            Text("Contribute", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Muted("Encore leans on AcoustID and MusicBrainz to know what a file is. Here it can give back: fingerprints of songs whose recording the audio confirmed, ISRCs MusicBrainz is missing, and albums that are only on Deezer or Apple Music. Only songs Encore is sure about are sent; nothing is sent until you add your own keys.")
        }

        // ---- AcoustID ----
        SetCard(Modifier.padding(bottom = 16.dp)) {
            CardHead("AcoustID fingerprints") {
                SmallButton("Send now", iconRes = R.drawable.ic_refresh, enabled = a?.bool("ready") == true) { run(scope, api, "acoustid") { version += 1 } }
            }
            Muted("When a download's audio confirms the MusicBrainz recording it was asked for, its fingerprint is sent to AcoustID in batches, once per song. Your user key says who sent it; the API key under Settings → Library → Verification & duplicates is Encore's application key.")
            StatTiles(
                listOf(
                    formatNumber(a?.dbl("queued")?.toLong() ?: 0) to "Waiting to send",
                    formatNumber(a?.dbl("submitted")?.toLong() ?: 0) to "Sent, being processed",
                    formatNumber(a?.dbl("imported")?.toLong() ?: 0) to "In AcoustID",
                    formatNumber(a?.dbl("failed")?.toLong() ?: 0) to "Failed",
                ),
            )
            if (a != null && a.str("why_not").isNotEmpty() && a.bool("enabled")) {
                Muted(a.str("why_not") + if (a.bool("can_fingerprint")) "" else ". fpcalc is not installed on this server")
            }
            LastRun(a?.obj("last"), now)
            SwitchRow("Send fingerprints of confirmed songs to AcoustID", switchOn("contribute_acoustid"), enabled = !locked("contribute_acoustid")) { edits["contribute_acoustid"] = it.toString() }
            FormField(
                "AcoustID user key",
                edits["acoustid_user_key"].orEmpty(),
                { edits["acoustid_user_key"] = it },
                help = "Sign in at acoustid.org and copy the user API key shown on your profile. It is not the application key: that one identifies Encore, this one identifies you.",
                placeholder = secretPlaceholder(v, "acoustid_user_key", locked("acoustid_user_key")),
                password = true,
                enabled = !locked("acoustid_user_key"),
            )
            TextLink("acoustid.org") { openSignInTab(context, "https://acoustid.org/new-application", p.bg) }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                EncoreButton("Save", iconRes = R.drawable.ic_check, primary = true) { save(listOf("contribute_acoustid", "acoustid_user_key"), setOf("acoustid_user_key")) }
            }
        }

        // ---- MusicBrainz ISRCs ----
        SetCard(Modifier.padding(bottom = 16.dp)) {
            CardHead("MusicBrainz ISRCs") {
                if (m?.bool("connected") == true) {
                    SmallButton("Send now", iconRes = R.drawable.ic_refresh) { run(scope, api, "isrc") { version += 1 } }
                    SmallButton("Disconnect") {
                        scope.serverCall(ok = "Disconnected", after = { _: Unit -> version += 1 }) { api.mbDisconnect() }
                    }
                } else {
                    SmallButton("Connect MusicBrainz", iconRes = R.drawable.ic_link, enabled = m?.bool("has_client") == true) {
                        // save the client id and secret first, as the web does
                        val body = LinkedHashMap<String, JsonElement>()
                        for (k in listOf("contribute_isrc", "mb_client_id", "mb_client_secret")) {
                            if (locked(k)) continue
                            val t = edits[k] ?: continue
                            if (k == "mb_client_secret" && (t.isBlank() || SchemaRules.isMasked(t))) continue
                            body[k] = if (t == "true" || t == "false") JsonPrimitive(t == "true") else JsonPrimitive(t.trim())
                        }
                        scope.launch {
                            try {
                                val url = withContext(Dispatchers.IO) {
                                    api.putSettings(JsonObject(body))
                                    api.mbConnect()
                                }.str("url")
                                waitingForMb = true
                                openSignInTab(context, url, p.bg)
                            } catch (e: CancellationException) {
                                throw e
                            } catch (e: Exception) {
                                Toasts.show(e.message ?: "Something went wrong", "err")
                                version += 1
                            }
                        }
                    }
                }
            }
            Muted("Songs from Deezer, Tidal and Qobuz carry the ISRC of the recording. When the audio confirmed the MusicBrainz recording and MusicBrainz does not list that ISRC yet, Encore adds it under your MusicBrainz account. ISRCs that MusicBrainz has on another recording are left for a person to look at.")
            Text(
                if (m?.bool("connected") == true) "Connected as ${m.str("account").ifEmpty { "your account" }}" else "Not connected",
                style = MaterialTheme.typography.bodySmall,
            )
            StatTiles(
                listOf(
                    formatNumber(m?.dbl("queued")?.toLong() ?: 0) to "Waiting to send",
                    formatNumber(m?.dbl("submitted")?.toLong() ?: 0) to "Added to MusicBrainz",
                    formatNumber(m?.dbl("skipped")?.toLong() ?: 0) to "Skipped",
                    formatNumber(m?.dbl("failed")?.toLong() ?: 0) to "Failed",
                ),
            )
            LastRun(m?.obj("last"), now)
            SwitchRow("Add missing ISRCs to MusicBrainz", switchOn("contribute_isrc"), enabled = !locked("contribute_isrc")) { edits["contribute_isrc"] = it.toString() }
            val callback = m?.str("redirect_uri")?.ifEmpty { null } ?: "<Public URL>/api/contribute/mb/callback (set the Public URL under General first)"
            FormField(
                "MusicBrainz application client ID",
                edits["mb_client_id"] ?: SchemaRules.textOf(v["mb_client_id"]),
                { edits["mb_client_id"] = it },
                help = "Register an application at musicbrainz.org/account/applications (type: web application) with the callback URL $callback.",
                enabled = !locked("mb_client_id"),
            )
            FormField(
                "MusicBrainz application client secret",
                edits["mb_client_secret"].orEmpty(),
                { edits["mb_client_secret"] = it },
                placeholder = secretPlaceholder(v, "mb_client_secret", locked("mb_client_secret")),
                password = true,
                enabled = !locked("mb_client_secret"),
            )
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                EncoreButton("Save", iconRes = R.drawable.ic_check, primary = true) {
                    save(listOf("contribute_isrc", "mb_client_id", "mb_client_secret"), setOf("mb_client_secret"))
                }
            }
        }

        SetCard {
            Text("Albums missing from MusicBrainz", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Muted("Artist pages list albums found on Deezer and Apple Music that MusicBrainz does not have. Their Add to MusicBrainz link opens the MusicBrainz release editor with the title, artists, date, label, barcode, track list and store links filled in. Check it, then enter the edit under your own MusicBrainz account. Once the album is in MusicBrainz, Encore re-tags your copy on its own.")
        }
    }
}

private fun secretPlaceholder(values: JsonObject, key: String, locked: Boolean): String = when {
    locked -> "set by environment"
    SchemaRules.truthy(values[key]) -> "saved – paste a new value to replace"
    else -> "not set"
}

/** "Last run 3 h ago: …" (in red when it failed). */
@Composable
private fun LastRun(l: JsonObject?, now: Double) {
    if (l == null || !SchemaRules.truthy(l["at"])) return
    val ok = l.bool("ok")
    Muted("Last run ${SettingsRules.ago(l.dbl("at"), now)}: ${l.str("message")}", color = if (ok) MaterialTheme.palette.muted else EncoreColors.Err)
}

private fun run(scope: kotlinx.coroutines.CoroutineScope, api: SettingsApi, what: String, after: () -> Unit) {
    scope.launch {
        try {
            val r = withContext(Dispatchers.IO) { api.contributeRun(what) }
            Toasts.show(r.str("message").ifEmpty { "Done" }, "ok")
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Toasts.show(e.message ?: "Something went wrong", "err")
        }
        after()
    }
}
