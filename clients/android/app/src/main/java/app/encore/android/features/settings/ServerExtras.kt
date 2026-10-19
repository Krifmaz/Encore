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
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
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
import app.encore.android.core.obj
import app.encore.android.core.parseJsonObject
import app.encore.android.core.str
import app.encore.android.data.api.SettingsApi
import app.encore.android.ui.components.Badge
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
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

// What the Settings → Apps & sources cards show besides their fields (web-rs/src/settings/apps.rs): the Download
// tools card, the Soulseek engine's status and the VPN's state with Test VPN, the Cadenza pre-render cache and the
// engine's own settings, the streamrip config reset, How requests are fulfilled; and General's API key and About.

// ---- Download tools ------------------------------------------------------------------------------------------------

/**
 * Settings → Apps & sources → Download tools: yt-dlp, streamrip, Cadenza, slskd and the Translator update in place,
 * with the automatic-update switches and the Translator's model (saved one key at a time).
 */
@Composable
fun DownloadToolsCard(api: SettingsApi, state: SchemaFormState, onOpen: (String) -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var tools by remember { mutableStateOf<List<JsonObject>?>(null) }
    var busy by remember { mutableStateOf<String?>(null) }
    var checking by remember { mutableStateOf(false) }
    var reset by remember { mutableStateOf<JsonObject?>(null) }
    suspend fun reload() {
        try { tools = SettingsRules.toolList(withContext(Dispatchers.IO) { api.tools() }) } catch (e: CancellationException) { throw e } catch (_: Exception) {}
    }
    // a server without the endpoint (or an error): nothing shows, as on the web
    LaunchedEffect(Unit) { reload() }
    val list = tools ?: return
    val values = state.values
    SetCard(Modifier.padding(bottom = 12.dp)) {
        CardHead("Download tools") {
            SmallButton(if (checking) "Checking…" else "Check for updates", iconRes = R.drawable.ic_refresh, enabled = !checking && busy == null) {
                checking = true
                scope.launch {
                    try { tools = SettingsRules.toolList(withContext(Dispatchers.IO) { api.toolsCheck() }) } catch (e: CancellationException) { throw e } catch (e: Exception) { Toasts.show(e.message ?: "Something went wrong", "err") }
                    checking = false
                }
            }
        }
        Muted("yt-dlp, streamrip, Cadenza, slskd and the Translator update in place – no new Encore image needed. Updates are kept in the config folder.")
        for (t in list) {
            ToolRow(t, busy) { action ->
                val id = t.str("id")
                when (action) {
                    "update" -> {
                        val name = t.str("name").ifEmpty { id }
                        val install = SettingsRules.toolInstall(t)
                        busy = id
                        scope.launch {
                            try {
                                val r = withContext(Dispatchers.IO) { api.toolUpdate(id) }
                                Toasts.show("$name ${if (install) "installed" else "updated to"} ${r.str("installed").ifEmpty { r.str("latest") }}", "ok")
                            } catch (e: CancellationException) {
                                throw e
                            } catch (e: Exception) {
                                Toasts.show(e.message ?: "Something went wrong", "err")
                            }
                            busy = null
                            reload()
                        }
                    }
                    "reset" -> reset = t
                }
            }
        }
        if (values.containsKey("translator_model")) {
            val locked = SchemaRules.source(values, "translator_model") == "env"
            Dropdown("Translator model", SettingsRules.TRANSLATOR_MODELS, SchemaRules.textOf(values["translator_model"]), enabled = !locked) { v ->
                scope.serverCall(after = { r: JsonObject ->
                    state.replaceValues(r)
                    Toasts.show("Saved – Install or Update the Translator to download this model", "ok")
                    scope.launch { reload() }
                }) { api.putSettings(JsonObject(mapOf("translator_model" to JsonPrimitive(v)))) }
            }
            Muted("Small runs on most servers (about 4 GB of memory while it works); Best translates better and needs about 7 GB. It only runs while there is something to translate.")
        }
        for ((key, label) in SettingsRules.TOOL_TOGGLES) {
            if (!values.containsKey(key)) continue
            val locked = SchemaRules.source(values, key) == "env"
            SwitchRow(label, SchemaRules.truthy(values[key]), enabled = !locked) { on ->
                scope.serverCall(ok = "Saved", after = { r: JsonObject -> state.replaceValues(r) }) { api.putSettings(JsonObject(mapOf(key to JsonPrimitive(on)))) }
            }
        }
        Muted("Every tool is checked daily; updates install by themselves only where the switch is on. Change the schedule under")
        TextLink("Scheduled tasks") { onOpen("tasks") }
    }
    reset?.let { t ->
        val (link, ask, done) = SettingsRules.toolReset(t)
        ConfirmDialog(link, ask, "OK", danger = true, onDismiss = { reset = null }) {
            val id = t.str("id")
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { api.toolReset(id) }
                    Toasts.show(done, "ok")
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Toasts.show(e.message ?: "Something went wrong", "err")
                }
                reload()
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ToolRow(t: JsonObject, busy: String?, onAction: (String) -> Unit) {
    val p = MaterialTheme.palette
    val id = t.str("id")
    Column(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(t.str("name").ifEmpty { id }, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
            Mono(if (SchemaRules.truthy(t["installed"])) t.str("installed") else "not installed", color = p.muted)
            if (t.str("source") == "updated" && id != "slskd" && id != "translator") Muted("(updated)")
        }
        Muted(t.str("note"))
        if (t.str("error").isNotEmpty()) Text(t.str("error"), style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (busy == id) {
                Spinner(14.dp)
                Muted(if (SettingsRules.toolInstall(t)) "Installing…" else "Updating…")
            } else {
                when (val a = SettingsRules.toolAction(t)) {
                    is SettingsRules.ToolAction.Install ->
                        SmallButton("Install" + if (a.latest.isNotEmpty()) " ${a.latest}" else "", iconRes = R.drawable.ic_download, primary = true, enabled = busy == null) { onAction("update") }
                    is SettingsRules.ToolAction.Update ->
                        SmallButton("Update to ${a.latest}", iconRes = R.drawable.ic_download, primary = true, enabled = busy == null) { onAction("update") }
                    is SettingsRules.ToolAction.Available -> Badge("${a.latest} available", kind = "warn")
                    SettingsRules.ToolAction.UpToDate -> Badge("Up to date", kind = "ok")
                    SettingsRules.ToolAction.NotChecked -> Muted("Not checked yet", color = p.faint)
                    SettingsRules.ToolAction.None -> {}
                }
            }
            if (t.bool("resettable") && t.bool("updatable") && busy == null) {
                TextLink(SettingsRules.toolReset(t).first, color = p.muted) { onAction("reset") }
            }
        }
    }
}

private fun JsonObject.bool(key: String): Boolean = SchemaRules.truthy(this[key])

// ---- Soulseek: the engine's status, the VPN's state and Test VPN ----------------------------------------------------

/** Settings → Soulseek → Status: how Encore's Soulseek engine is doing, asked again every 5 s while the card is open. */
@Composable
fun SoulseekStatusBlock(api: SettingsApi) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var status by remember { mutableStateOf<JsonObject?>(null) }
    var failed by remember { mutableStateOf(false) }
    var starting by remember { mutableStateOf(false) }
    var showLog by remember { mutableStateOf(false) }
    suspend fun refresh() {
        try {
            status = withContext(Dispatchers.IO) { api.soulseekStatus() }
            failed = false
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            failed = true
        }
    }
    // drawn only while the card is open: the poll stops when it closes, and waits while the app is in the background
    val onScreen = rememberOnScreen()
    LaunchedEffect(Unit) {
        while (true) {
            onScreen.awaitOnScreen()
            refresh()
            delay(5000)
        }
    }
    SetCard(Modifier.padding(top = 8.dp), flat = true) {
        CardHead("Status") {
            val st = status?.str("state").orEmpty()
            if (SettingsRules.canStartNow(st)) {
                SmallButton("Start now", enabled = !starting) {
                    starting = true
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { api.soulseekStart() }
                            Toasts.show("Starting Soulseek", "ok")
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            Toasts.show(e.message ?: "Something went wrong", "err")
                        }
                        starting = false
                        // the supervisor starts it on its next check: look again once it has had a moment
                        delay(1500)
                        refresh()
                    }
                }
            }
        }
        val s = status
        when {
            failed && s == null -> Muted("Status unavailable")
            s == null -> Muted("Checking…")
            else -> {
                val line = SettingsRules.soulseekLine(s)
                Muted(line.text, color = if (line.error) EncoreColors.Err else p.muted)
                if (line.warning.isNotEmpty()) Text(line.warning, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn)
                if (line.log.isNotEmpty()) {
                    TextLink("Engine log (last ${line.log.size} lines)", color = p.muted) { showLog = !showLog }
                    if (showLog) Mono(line.log.joinToString("\n"), color = p.muted)
                }
            }
        }
    }
}

/** Under the VPN for Soulseek fields: the tunnel's state (every 5 s while the card is open) and Test VPN. */
@Composable
fun VpnFooter(api: SettingsApi, card: SchemaCardScope) {
    val p = MaterialTheme.palette
    var vpn by remember { mutableStateOf<JsonObject?>(null) }
    var unavailable by remember { mutableStateOf(false) }
    var testing by remember { mutableStateOf(false) }
    var tested by remember { mutableStateOf<Pair<String, Boolean>?>(null) }
    val onScreen = rememberOnScreen()
    LaunchedEffect(Unit) {
        while (true) {
            onScreen.awaitOnScreen()
            if (!testing) {
                try {
                    vpn = withContext(Dispatchers.IO) { api.vpn() }
                    unavailable = false
                    tested = null
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                    unavailable = true
                }
            }
            delay(5000)
        }
    }
    Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Column(Modifier.weight(1f)) {
            val t = tested
            when {
                testing -> Row(verticalAlignment = Alignment.CenterVertically) { Spinner(12.dp); Spacer(Modifier.width(6.dp)); Muted("Testing…") }
                t != null -> Muted(t.first, color = if (t.second) EncoreColors.Err else p.muted)
                unavailable -> Muted("Status unavailable")
                vpn == null -> Muted("Checking…")
                else -> Muted(SettingsRules.vpnText(vpn), color = if (SettingsRules.vpnIsError(vpn)) EncoreColors.Err else p.muted)
            }
        }
        SmallButton("Test VPN", iconRes = R.drawable.ic_refresh, enabled = !testing) {
            testing = true
            card.launch {
                tested = try {
                    if (!card.save()) throw IllegalStateException("")
                    val r = withContext(Dispatchers.IO) { api.vpnTest() }
                    val note = SettingsRules.vpnTestNote(r)
                    (SettingsRules.vpnText(r) + if (note.isNotEmpty()) " · $note" else "") to (SettingsRules.vpnIsError(r) || note.isNotEmpty())
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    (e.message?.ifEmpty { null } ?: "Test failed") to true
                }
                testing = false
            }
        }
    }
}

// ---- Cadenza: the pre-render cache and the engine's own settings --------------------------------------------------

/** The engine settings block of the Cadenza card: what the engine offers, what is saved and what was changed. */
@Stable
class EngineState {
    var loaded by mutableStateOf(false)
    var settings by mutableStateOf<List<SettingsRules.EngineSetting>>(emptyList())
    var saved by mutableStateOf<JsonObject?>(null)
    val edits = mutableStateMapOf<String, String>()

    /** What the last [extra] gave Save, and the edits it was made from (for [committed]). */
    private var sent: String? = null
    private var sentEdits: Map<String, String> = emptyMap()

    /**
     * The extra key for Save: null until the block was drawn, while an environment variable sets it, and when nothing
     * in the block changed (Save and Test send only what changed).
     */
    fun extra(values: JsonObject): JsonObject? {
        if (!loaded || SchemaRules.source(values, "cadenza_engine_settings") == "env") return null
        val v = SettingsRules.engineChange(settings, saved, edits) ?: return null
        sent = v
        sentEdits = edits.toMap()
        return JsonObject(mapOf("cadenza_engine_settings" to JsonPrimitive(v)))
    }

    /** After a Save that sent [extra]'s value worked: that value is what is saved now, and the edits it held are done. */
    fun committed() {
        val v = sent ?: return
        sent = null
        saved = parseJsonObject(v) ?: saved
        for ((k, t) in sentEdits) if (edits[k] == t) edits.remove(k)
        sentEdits = emptyMap()
    }
}

/** Settings → Server processing: the pre-render cache (how full, how fast, Clear cache) and the engine's settings. */
@Composable
fun CadenzaBlocks(api: SettingsApi, state: SchemaFormState, engine: EngineState) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var dsp by remember { mutableStateOf<JsonObject?>(null) }
    var failed by remember { mutableStateOf(false) }
    var clearing by remember { mutableStateOf(false) }
    val locked = SchemaRules.source(state.values, "cadenza_engine_settings") == "env"
    suspend fun refresh() {
        try {
            val d = withContext(Dispatchers.IO) { api.dsp() }
            dsp = d
            failed = false
            if (!engine.loaded) {
                val e = d.obj("engine")
                engine.settings = SettingsRules.engineSettings(e)
                engine.saved = e?.obj("values")
                engine.loaded = true
            }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            failed = true
        }
    }
    LaunchedEffect(Unit) { refresh() }
    SetCard(Modifier.padding(top = 8.dp), flat = true) {
        CardHead("Pre-render cache") {
            SmallButton("Clear cache", iconRes = R.drawable.ic_delete, enabled = !clearing) {
                clearing = true
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) { api.dspCacheClear() }
                        Toasts.show("Cache cleared", "ok")
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "Something went wrong", "err")
                    }
                    clearing = false
                    refresh()
                }
            }
        }
        val d = dsp
        Muted(if (failed) "Status unavailable" else if (d == null) "Checking…" else SettingsRules.cadenzaCacheText(d.obj("cache")))
        Muted(if (d == null) "Engine: …" else SettingsRules.cadenzaEngineText(d.obj("engine")))
    }
    SetCard(Modifier.padding(top = 8.dp), flat = true) {
        Text("Engine settings", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold)
        when {
            !engine.loaded -> Muted("Checking…")
            engine.settings.isEmpty() -> Muted("This engine has no settings of its own.")
            else -> {
                Muted(
                    "The settings of the engine in use. People’s own filter and dither choices under Settings → Playback still win for their streams." +
                        if (locked) " Set by an environment variable." else "",
                )
                for (s in engine.settings) EngineField(s, engine, locked)
            }
        }
    }
}

@Composable
private fun EngineField(s: SettingsRules.EngineSetting, engine: EngineState, locked: Boolean) {
    val value = engine.edits[s.key] ?: SettingsRules.engineInitial(s, engine.saved)
    val help = (if (s.help.isNotEmpty()) "${s.help} · " else "") + "Default: ${SettingsRules.engineDefaultText(s)}"
    when (s.type) {
        "bool" -> SwitchRow(s.label, value == "true", enabled = !locked, help = help) { engine.edits[s.key] = if (it) "true" else "false" }
        "select" -> {
            Dropdown(s.label, s.options.map { SettingsRules.text(it.first) to it.second }, value, enabled = !locked) { engine.edits[s.key] = it }
            Muted(help)
        }
        else -> {
            OutlinedTextField(
                value = value,
                onValueChange = { engine.edits[s.key] = it },
                label = { Text(s.label) },
                enabled = !locked,
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = if ((s.step ?: 1.0) % 1.0 != 0.0 || (s.min ?: 0.0) < 0) KeyboardType.Decimal else KeyboardType.Number),
                modifier = Modifier.fillMaxWidth(),
            )
            Muted(help)
        }
    }
}

// ---- streamrip's config file ------------------------------------------------------------------------------------------

/** Under streamrip's card: write a fresh config.toml from streamrip's defaults when it does not load. */
@Composable
fun StreamripConfigBlock(api: SettingsApi) {
    val scope = rememberCoroutineScope()
    var asking by remember { mutableStateOf(false) }
    var working by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf<Pair<Boolean, String>?>(null) }
    SetCard(Modifier.padding(top = 8.dp), flat = true) {
        CardHead("streamrip config file") {
            SmallButton("Reset streamrip config", iconRes = R.drawable.ic_refresh, enabled = !working) { asking = true }
        }
        Muted("When requests fail with \"Error loading config\", write a fresh config.toml from streamrip's defaults. The old file is kept as a backup; the Tidal sign-in, Deezer ARL and Qobuz login are carried over.")
        result?.let { (ok, msg) -> ResultBox(ok, msg) }
    }
    if (asking) {
        ConfirmDialog(
            "Reset streamrip config",
            "Replace streamrip's config.toml with a fresh one from its defaults? The current file is kept as a backup, and the Tidal sign-in, Deezer ARL and Qobuz login are carried over.",
            "Reset",
            onDismiss = { asking = false },
        ) {
            working = true
            scope.launch {
                try {
                    val r = withContext(Dispatchers.IO) { api.streamripResetConfig() }
                    val ok = SchemaRules.truthy(r["ok"])
                    result = ok to r.str("message").ifEmpty { if (ok) "Connected" else "Failed" }
                    Toasts.show(if (ok) "streamrip config reset" else "streamrip config still doesn't load", if (ok) "ok" else "err")
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    result = false to (e.message ?: "Failed")
                }
                working = false
            }
        }
    }
}

// ---- How requests are fulfilled (the end of Apps & sources) ------------------------------------------------------------

@Composable
fun PipelineHelp() {
    val steps = listOf(
        "Library check" to "if the album/track is already in your library (by MusicBrainz id or name) it's marked available instantly.",
        "streamrip" to "lossless from Qobuz, Tidal or Deezer with your own accounts, matched by ISRC.",
        "Torrents & Usenet" to "Prowlarr searches your indexers; the best album release goes to your torrent or Usenet client and is imported when it finishes. If it stalls past the timeout, the next release or source is tried.",
        "Soulseek" to "Encore searches the network, results are ranked by track count, format, bitrate, folder name and the uploader's free slots/speed. Stalled or failed users are skipped automatically.",
        "yt-dlp" to "each track from the MusicBrainz tracklist is matched on YouTube (preferring official \"Topic\" uploads with matching length).",
        "Import" to "files are matched to the MusicBrainz release, tagged (incl. MBIDs, ISRCs, cover art), renamed with your template, and your media servers are refreshed.",
    )
    SetCard(Modifier.padding(top = 4.dp)) {
        Text("How requests are fulfilled", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        steps.forEachIndexed { i, (name, text) ->
            Row {
                Muted("${i + 1}. ")
                Column(Modifier.weight(1f)) {
                    Text(
                        androidx.compose.ui.text.buildAnnotatedString {
                            pushStyle(androidx.compose.ui.text.SpanStyle(fontWeight = FontWeight.Bold))
                            append(name)
                            pop()
                            append(" – ")
                            append(text)
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.palette.muted,
                    )
                }
            }
        }
    }
}

// ---- General: the API key and About ---------------------------------------------------------------------------------

/** Encore's own API key (only the owner may see it; nothing shows for anyone else): copy and regenerate. */
@Composable
fun ApiKeyCard(api: SettingsApi) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val scope = rememberCoroutineScope()
    var key by remember { mutableStateOf<String?>(null) }
    var shown by remember { mutableStateOf(false) }
    var asking by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        key = try { withContext(Dispatchers.IO) { api.apiKey() }.str("api_key") } catch (e: CancellationException) { throw e } catch (_: Exception) { null }
    }
    val k = key ?: return
    // the key in plain text stays out of screenshots and the Recents thumbnail
    SecureWindow(shown)
    SetCard(Modifier.padding(top = 12.dp)) {
        Text("Encore API key", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        Muted("Send it in an X-Api-Key header to automate Encore (scripts, Home Assistant…). It acts as the owner account – keep it private.")
        Mono(if (shown) k else "•".repeat(k.length.coerceIn(8, 32)))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SmallButton(if (shown) "Hide" else "Show") { shown = !shown }
            SmallButton("Copy", iconRes = R.drawable.ic_content_copy) { copyToClipboard(context, k, sensitive = true) }
            SmallButton("Regenerate", iconRes = R.drawable.ic_refresh) { asking = true }
        }
    }
    if (asking) {
        ConfirmDialog("Encore API key", "Make a new API key? Scripts using the old one stop working.", "Regenerate", danger = true, onDismiss = { asking = false }) {
            scope.serverCall(ok = "New API key generated", after = { r: JsonObject -> key = r.str("api_key") }) { api.regenerateApiKey() }
        }
    }
}

/** Version, build and the apps (General for admins, the bottom of Account for members). */
@Composable
fun AboutCard(info: JsonObject?) {
    val context = androidx.compose.ui.platform.LocalContext.current
    SetCard(Modifier.padding(top = 12.dp)) {
        Text("About Encore", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
            Column {
                Muted("Version")
                Text("v" + (info?.str("version")?.ifEmpty { null } ?: "?"), style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
            }
            Column {
                Muted("Server")
                Text("Encore", style = MaterialTheme.typography.bodyMedium)
            }
            Column {
                Muted("Build")
                Mono(info?.str("build")?.ifEmpty { null } ?: "dev")
            }
        }
        Muted("Apps for Android, Windows, macOS and Linux:")
        TextLink("github.com/Krifmaz/Encore/releases") { openSignInTab(context, "https://github.com/Krifmaz/Encore/releases") }
    }
}

/** The About card with the server's version and build, asked for here (Account, for members). */
@Composable
fun AboutEncore(api: SettingsApi) {
    var info by remember { mutableStateOf<JsonObject?>(null) }
    LaunchedEffect(Unit) {
        info = try { withContext(Dispatchers.IO) { api.info() } } catch (e: CancellationException) { throw e } catch (_: Exception) { null }
    }
    if (info != null) AboutCard(info)
}

/** The account's `prefs` object of GET /api/auth/me ("{}" when it has none). */
fun prefsOf(me: JsonObject): JsonObject = me.obj("prefs") ?: parseJsonObject(me.str("prefs")) ?: JsonObject(emptyMap())

/** A JSON value as an element (for bodies built from typed text). */
fun jsonText(v: String): JsonElement = JsonPrimitive(v)
