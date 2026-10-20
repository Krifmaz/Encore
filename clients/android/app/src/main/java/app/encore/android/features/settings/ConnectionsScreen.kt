package app.encore.android.features.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
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
import app.encore.android.data.repo.formatAgo
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** A sign-in waiting in a browser tab (Spotify, Tidal, Last.fm, Plex): what the dialog shows and what it polls. */
private class Waiting(val svc: String, val label: String, val url: String, val pinId: JsonElement? = null) {
    /** the wait (Plex 4 minutes, the others 2) counts from here, also when coming back to the app looks again */
    val since: Long = System.currentTimeMillis()
    val limitMs: Long = SettingsRules.signInWaitMs(plex = pinId != null)
}

/**
 * Settings → Connections (web-rs/src/settings/connections.rs): one card per service with its state, switches,
 * Import playlists and Disconnect; the connect forms (username and password, token, Deezer profile, Last.fm, the
 * YouTube Music code or pasted headers); Sync now.
 *
 * The web sends the whole page to Spotify's, Tidal's or Last.fm's sign-in and comes back to it. The phone opens the
 * sign-in in a browser tab over the app and asks the server for the connection every 2 s for 2 minutes (and as soon
 * as the app is back in front) until it is there, as the web's Plex popup does (Plex: 4 minutes, as on the web). A
 * Last.fm account connected by username only counts once it can scrobble.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ConnectionsScreen(nav: Nav, admin: Boolean, onBack: () -> Unit) {
    val api = nav.graph.settingsApi
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var version by remember { mutableIntStateOf(0) }
    var d by remember { mutableStateOf<JsonObject?>(null) }
    var error by remember { mutableStateOf("") }
    var syncing by remember { mutableStateOf(false) }
    var form by remember { mutableStateOf<String?>(null) }
    var lastfm by remember { mutableStateOf(false) }
    var ytmusic by remember { mutableStateOf(false) }
    var waiting by remember { mutableStateOf<Waiting?>(null) }
    var setupError by remember { mutableStateOf<Pair<String, String>?>(null) }
    var disconnect by remember { mutableStateOf<String?>(null) }
    var playlistsOf by remember { mutableStateOf<String?>(null) }
    var poke by remember { mutableIntStateOf(0) }
    LaunchedEffect(version) {
        try {
            d = withContext(Dispatchers.IO) { api.connections() }
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    // back from the browser tab: look at once (the poll below goes on until the connection is there)
    OnResume { if (waiting != null) poke += 1 }

    fun label(svc: String): String = d?.obj("services")?.obj(svc)?.str("label")?.ifEmpty { null } ?: svc

    /** Spotify, Tidal and Last.fm: the server's sign-in link in a browser tab, then the wait. */
    fun startRedirect(svc: String) {
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { api.connect(svc, JsonObject(emptyMap())) }
                val url = r.str("url")
                waiting = Waiting(svc, label(svc), url)
                openSignInTab(context, url, p.bg)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                val msg = e.message ?: "Something went wrong"
                // the admin side is not ready (no Public URL / no client id): a dialog the user can read, not a passing toast
                if (SettingsRules.isSetupError(msg)) setupError = svc to msg else Toasts.show(msg, "err")
            }
        }
    }

    fun connect(svc: String) {
        when (SettingsRules.connectionInfo(svc).second) {
            "oauth" -> startRedirect(svc)
            "lastfm" -> lastfm = true
            "ytmusic" -> ytmusic = true
            "plex" -> scope.launch {
                try {
                    val r = withContext(Dispatchers.IO) { api.connect("plex", JsonObject(emptyMap())) }
                    waiting = Waiting("plex", "Plex", r.str("url"), r["pin_id"] ?: JsonNull)
                    openSignInTab(context, r.str("url"), p.bg)
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Toasts.show(e.message ?: "Something went wrong", "err")
                }
            }
            else -> form = svc
        }
    }

    SettingsPage("Connections", onBack) {
        val data = d
        if (data == null) {
            if (error.isNotEmpty()) ErrorBox(error) { version += 1 } else Spinner(24.dp)
            return@SettingsPage
        }
        val sync = data.obj("sync")
        val last = if (sync != null && SchemaRules.truthy(sync["last"])) " Last sync ${formatAgo(sync.dbl("last"))}." else ""
        Muted("Link your accounts so likes and playlists stay the same in Finamp, Plexamp, Navidrome and here.$last", Modifier.padding(bottom = 10.dp))
        EncoreButton("Sync now", iconRes = R.drawable.ic_refresh, primary = true, enabled = !syncing, modifier = Modifier.padding(bottom = 14.dp)) {
            syncing = true
            scope.launch {
                try {
                    val r = withContext(Dispatchers.IO) { api.syncNow() }
                    val (msg, errors) = SettingsRules.syncToast(r)
                    Toasts.show(msg, if (errors) "err" else "ok")
                    version += 1
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Toasts.show(e.message ?: "Something went wrong", "err")
                }
                syncing = false
            }
        }
        val services = data.obj("services") ?: JsonObject(emptyMap())
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            for ((k, sEl) in services) {
                val s = sEl as? JsonObject ?: continue
                val c = data.obj("connections")?.obj(k)
                val available = SchemaRules.truthy(data.obj("available")?.get(k))
                ServiceCard(k, s, c, available, api, onConnect = { connect(k) }, onDisconnect = { disconnect = k }, onPlaylists = { playlistsOf = k })
            }
        }
    }

    // ---- dialogs ----
    waiting?.let { w ->
        // every 2 s for 2 minutes (Plex: its PIN check, for 4 minutes as the web), and at once when the app comes back
        LaunchedEffect(w, poke) {
            if (poke == 0) delay(2000)
            while (System.currentTimeMillis() - w.since < w.limitMs) {
                try {
                    if (w.pinId != null) {
                        val c = withContext(Dispatchers.IO) { api.plexCheck(w.pinId) }
                        if (SchemaRules.truthy(c["done"])) {
                            waiting = null
                            Toasts.show("Plex connected (${c.str("account")})", "ok")
                            version += 1
                            return@LaunchedEffect
                        }
                    } else {
                        val now = withContext(Dispatchers.IO) { api.connections() }
                        // Last.fm may already be there by username only: done once it can scrobble
                        if (SettingsRules.signInDone(w.svc, now.obj("connections")?.get(w.svc))) {
                            waiting = null
                            d = now
                            Toasts.show("${w.label} connected", "ok")
                            return@LaunchedEffect
                        }
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    if (w.pinId != null) {
                        waiting = null
                        Toasts.show(e.message ?: "Something went wrong", "err")
                        return@LaunchedEffect
                    }
                }
                delay(2000)
            }
            if (waiting == w) {
                waiting = null
                Toasts.show("${w.label} sign-in timed out", "err")
            }
        }
        FormDialog("Sign in to ${w.label}", onDismiss = { waiting = null }) {
            EncoreButton("Open ${w.label} sign-in", iconRes = R.drawable.ic_link, primary = true) { openSignInTab(context, w.url, p.bg) }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Spinner(14.dp)
                Spacer(Modifier.width(8.dp))
                Muted("Approve Encore in the ${w.label} page that opens, then come back here. Waiting…")
            }
        }
    }
    setupError?.let { (svc, msg) ->
        FormDialog("Connect ${label(svc)}", onDismiss = { setupError = null }, cancelLabel = "Close") {
            Muted(msg)
            if (admin) {
                val base = (d?.str("base_url")?.ifEmpty { null } ?: api.base).trimEnd('/')
                Text("Redirect URI to register at the service:", style = MaterialTheme.typography.bodySmall)
                Mono("$base/api/connections/$svc/callback")
            }
        }
    }
    form?.let { svc -> ConnectFormDialog(api, svc, onDismiss = { form = null }) { form = null; version += 1 } }
    if (lastfm) {
        LastfmDialog(api, onDismiss = { lastfm = false }, onSignIn = { lastfm = false; startRedirect("lastfm") }) { lastfm = false; version += 1 }
    }
    if (ytmusic) {
        YtMusicDialog(api, oauth = SchemaRules.truthy(d?.obj("available_oauth")?.get("ytmusic")), onDismiss = { ytmusic = false }) { ytmusic = false; version += 1 }
    }
    disconnect?.let { svc ->
        ConfirmDialog("Disconnect", "Disconnect ${label(svc)}?", "Disconnect", danger = true, onDismiss = { disconnect = null }) {
            scope.serverCall(after = { _: Unit -> version += 1 }) { api.disconnect(svc) }
        }
    }
    playlistsOf?.let { svc ->
        RemotePlaylistsDialog(api, svc, label(svc), onDismiss = { playlistsOf = null }) { url ->
            playlistsOf = null
            nav.open(Screen.PlaylistImport(null, url))
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ServiceCard(
    k: String,
    s: JsonObject,
    c: JsonObject?,
    available: Boolean,
    api: SettingsApi,
    onConnect: () -> Unit,
    onDisconnect: () -> Unit,
    onPlaylists: () -> Unit,
) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    // the switches as the user set them (PATCH answers nothing worth repainting from)
    val flags = remember(c) { mutableStateMapOf<String, Boolean>() }
    fun flag(name: String): Boolean = flags[name] ?: (c != null && SchemaRules.truthy(c[name]))
    fun setFlag(name: String, on: Boolean) {
        flags[name] = on
        scope.launch {
            try { withContext(Dispatchers.IO) { api.setConnectionFlag(k, name, on) } } catch (e: CancellationException) { throw e } catch (_: Exception) {}
        }
    }
    SetCard {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(s.str("label"), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            if (c != null) {
                if (c.str("last_error").isNotEmpty()) Badge("error", kind = "err") else Badge("connected", kind = "ok")
            }
        }
        Muted(SettingsRules.connectionInfo(k).first)
        when {
            c != null -> {
                Text(
                    "Signed in as ${c.str("account")}" + if (SchemaRules.truthy(c["last_sync"])) " · synced ${formatAgo(c.dbl("last_sync"))}" else "",
                    style = MaterialTheme.typography.bodySmall,
                )
                if (c.str("last_error").isNotEmpty()) Text(c.str("last_error"), style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err)
                if (c.bool("can_scrobble")) FlagRow("Scrobble what I play", flag("scrobble")) { setFlag("scrobble", it) }
                FlagRow("Sync likes", flag("sync_likes")) { setFlag("sync_likes", it) }
                if (s.str("kind") != "stream") FlagRow("Sync playlists", flag("sync_playlists")) { setFlag("sync_playlists", it) }
                if (k == "lastfm" && !c.bool("can_scrobble") && c.containsKey("can_scrobble")) {
                    Muted("Connected by username only – sign in with Last.fm to scrobble.")
                    SmallButton("Sign in with Last.fm", iconRes = R.drawable.ic_link) { onConnect() }
                }
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (k != "lastfm") SmallButton("Import playlists", iconRes = R.drawable.ic_download) { onPlaylists() }
                    SmallButton("Disconnect") { onDisconnect() }
                }
            }
            available -> EncoreButton("Connect", iconRes = R.drawable.ic_link) { onConnect() }
            else -> Muted("Not set up yet – an admin needs to add it in Settings.", color = p.faint)
        }
    }
}

@Composable
private fun FlagRow(label: String, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Switch(checked = on, onCheckedChange = onChange)
        Spacer(Modifier.width(10.dp))
        Text(label, style = MaterialTheme.typography.bodyMedium)
    }
}

/** Username and password (Jellyfin, Navidrome), a token (ListenBrainz) or a Deezer profile. */
@Composable
private fun ConnectFormDialog(api: SettingsApi, svc: String, onDismiss: () -> Unit, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    val kind = SettingsRules.connectionInfo(svc).second
    val fields = remember { mutableStateMapOf<String, String>() }
    FormDialog("Connect", onDismiss = onDismiss, okLabel = "Connect", onOk = {
        val body = JsonObject(fields.mapValues { JsonPrimitive(it.value) })
        scope.serverCall(after = { r: JsonObject ->
            Toasts.show("Connected as ${r.str("account")}", "ok")
            onDone()
        }) { api.connect(svc, body) }
    }) {
        when (kind) {
            "userpass" -> {
                FormField("Username", fields["username"].orEmpty(), { fields["username"] = it })
                FormField("Password", fields["password"].orEmpty(), { fields["password"] = it }, password = true)
            }
            // readable as on the web, but with the password keyboard: no suggestions, nothing learned
            "token" -> FormField(
                "User token",
                fields["token"].orEmpty(),
                { fields["token"] = it },
                help = "listenbrainz.org → Settings → User token",
                keyboard = androidx.compose.ui.text.input.KeyboardType.Password,
            )
            "profile" -> FormField(
                "Deezer profile link or user id",
                fields["profile"].orEmpty(),
                { fields["profile"] = it },
                placeholder = "https://www.deezer.com/profile/123456",
                help = "Your favourites must be public",
                keyboard = androidx.compose.ui.text.input.KeyboardType.Uri,
            )
            else -> FormField("Last.fm username", fields["username"].orEmpty(), { fields["username"] = it })
        }
    }
}

/** Sign in on last.fm (scrobbling), or just a username (loved tracks and recommendations). */
@Composable
private fun LastfmDialog(api: SettingsApi, onDismiss: () -> Unit, onSignIn: () -> Unit, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var more by remember { mutableStateOf(false) }
    var user by remember { mutableStateOf("") }
    FormDialog("Connect Last.fm", onDismiss = onDismiss) {
        Muted("Sign in on last.fm and allow Encore – then everything you play here is scrobbled, and For you suggests artists from your listening.")
        Muted("Only approve Encore on Last.fm when you started the sign-in yourself.")
        EncoreButton("Sign in with Last.fm", iconRes = R.drawable.ic_link, primary = true) { onSignIn() }
        TextLink("Or just enter a username (loved tracks and recommendations, no scrobbling)", color = MaterialTheme.palette.muted) { more = !more }
        if (more) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FormField("", user, { user = it }, placeholder = "Last.fm username", modifier = Modifier.weight(1f))
                SmallButton("Connect", enabled = user.isNotBlank()) {
                    scope.serverCall(after = { r: JsonObject ->
                        Toasts.show("Connected as ${r.str("account")}", "ok")
                        onDone()
                    }) { api.connect("lastfm", buildJsonObject { put("username", user.trim()) }) }
                }
            }
        }
    }
}

/**
 * YouTube Music: a code to enter at google.com/device (when an admin added a Google OAuth client), polled at the
 * interval Google gives; or the pasted headers of a signed-in music.youtube.com request.
 */
@Composable
private fun YtMusicDialog(api: SettingsApi, oauth: Boolean, onDismiss: () -> Unit, onDone: () -> Unit) {
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var mode by remember { mutableStateOf(if (oauth) "code" else "headers") }
    var code by remember { mutableStateOf<JsonObject?>(null) }
    var starting by remember { mutableStateOf(false) }
    var headers by remember { mutableStateOf("") }
    code?.let { c ->
        LaunchedEffect(c) {
            val interval = c.dbl("interval").let { if (it == 0.0) 5.0 else it }
            for (i in 0 until 180) {
                delay((interval * 1000).toLong())
                val r = try { withContext(Dispatchers.IO) { api.ytmusicCheck(c["device_code"] ?: JsonNull) } } catch (e: CancellationException) { throw e } catch (_: Exception) { null }
                if (r != null && SchemaRules.truthy(r["done"])) {
                    Toasts.show("YouTube Music connected (${r.str("account")})", "ok")
                    onDone()
                    return@LaunchedEffect
                }
            }
        }
    }
    FormDialog("Connect YouTube Music", onDismiss = onDismiss) {
        if (oauth) SegRow(listOf("code" to "Sign in with a code", "headers" to "Paste browser headers"), mode) { mode = it }
        if (mode == "code") {
            Muted("You'll get a short code to enter at google.com/device with the Google account you use for YouTube Music.")
            val c = code
            if (c == null) {
                EncoreButton("Get code", iconRes = R.drawable.ic_link, primary = true, enabled = !starting) {
                    starting = true
                    scope.launch {
                        try {
                            code = withContext(Dispatchers.IO) { api.connect("ytmusic", JsonObject(emptyMap())) }
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            Toasts.show(e.message ?: "Something went wrong", "err")
                        }
                        starting = false
                    }
                }
            } else {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("Go to", style = MaterialTheme.typography.bodyMedium)
                    TextLink(c.str("url")) { openSignInTab(context, c.str("url"), p.bg) }
                    Text("and enter", style = MaterialTheme.typography.bodyMedium)
                }
                Text(c.str("user_code"), style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold, fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace)
                SmallButton("Copy", iconRes = R.drawable.ic_content_copy) { copyToClipboard(context, c.str("user_code")) }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Spinner(14.dp)
                    Spacer(Modifier.width(8.dp))
                    Muted("Waiting for you to approve…")
                }
            }
        } else {
            if (!oauth) Muted("Signing in with a code needs a Google OAuth client, which no admin has added in Settings → Integrations yet. Paste the headers of a signed-in music.youtube.com request instead – it works the same.")
            Muted("1. Open music.youtube.com in a desktop browser while signed in, press F12 → Network.")
            Muted("2. Click around, find a browse request (POST), right-click → Copy → Copy request headers.")
            Muted("3. Paste below.")
            FormField("", headers, { headers = it }, placeholder = "accept: */*\ncookie: …", minLines = 6, mono = true)
            EncoreButton("Connect", primary = true, enabled = headers.isNotBlank()) {
                scope.serverCall(after = { r: JsonObject ->
                    Toasts.show("YouTube Music connected (${r.str("account")})", "ok")
                    onDone()
                }) { api.connect("ytmusic", buildJsonObject { put("headers", headers) }) }
            }
        }
    }
}

/** The playlists on a service, imported one by one (with Follow), and Import Liked Songs where the service has a link. */
@Composable
private fun RemotePlaylistsDialog(api: SettingsApi, svc: String, label: String, onDismiss: () -> Unit, onLiked: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    var rows by remember { mutableStateOf<List<JsonObject>?>(null) }
    var error by remember { mutableStateOf("") }
    val follow = remember { mutableStateMapOf<String, Boolean>() }
    val done = remember { mutableStateMapOf<String, Boolean>() }
    val busy = remember { mutableStateMapOf<String, Boolean>() }
    LaunchedEffect(svc) {
        try {
            rows = withContext(Dispatchers.IO) { api.remotePlaylists(svc) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    FormDialog("Playlists on $label", onDismiss = onDismiss, cancelLabel = "Close") {
        SettingsRules.likedUrl(svc)?.let { url -> EncoreButton("♥ Import Liked Songs") { onLiked(url) } }
        val list = rows
        when {
            error.isNotEmpty() -> Text(error, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err)
            list == null -> Spinner(20.dp)
            list.isEmpty() -> Muted("No playlists found")
            else -> for (pl in list) {
                val id = pl.str("id")
                Column(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
                    Text(pl.str("name"), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold, maxLines = 2)
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Switch(checked = follow[id] ?: true, onCheckedChange = { follow[id] = it })
                        Text("Follow", style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f))
                        if (done[id] == true) {
                            Badge("imported", kind = "ok")
                        } else {
                            SmallButton("Import", iconRes = R.drawable.ic_download, primary = true, enabled = busy[id] != true) {
                                busy[id] = true
                                scope.launch {
                                    try {
                                        val r = withContext(Dispatchers.IO) { api.importRemote(svc, id, pl.str("name"), pl.str("url"), follow[id] ?: true) }
                                        val dup = SchemaRules.truthy(r["duplicates_skipped"])
                                        Toasts.show("Imported ${pl.str("name")}: ${r.str("added")} tracks" + if (dup) ", ${r.str("duplicates_skipped")} duplicates skipped" else "", "ok")
                                        done[id] = true
                                    } catch (e: CancellationException) {
                                        throw e
                                    } catch (e: Exception) {
                                        Toasts.show(e.message ?: "Something went wrong", "err")
                                    }
                                    busy[id] = false
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
