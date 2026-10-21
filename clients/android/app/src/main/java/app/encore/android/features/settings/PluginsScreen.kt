package app.encore.android.features.settings

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.obj
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.core.strings
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.SettingsApi
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.CopyIconButton
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.screens.openUrl
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody
import okio.BufferedSink
import okio.source

// Settings → Server → Plugins (0.32), the web's web-rs/src/settings/plugins.rs on the phone: the head card with Check
// for updates and the sandbox line; each installed plugin as a card (kind, version, badges, status line, Update,
// Test, Logs, Disable or Enable, Remove, the auto-update switch, the logs drawer and its settings drawn by SchemaForm
// from GET /api/plugins/{id}/settings); the plugins not installed under Available with Install; and Add a plugin:
// Upload plugin through the system's file picker (multipart) and Install from URL (https or owner/repo@ref), each
// after the third-party warning.
//
// The source cards of Downloads are plugin cards here since 0.32 (streamrip with Tidal sign-in and its config file,
// Torrents & Usenet, Soulseek with its VPN block and status, yt-dlp with YouTube sign-in as a part). A built-in
// plugin's settings are Encore's own keys: they are read from and saved with GET/PUT /api/admin/settings through the
// Server pages' shared SchemaFormState (environment locks, Show and Copy, the folder warnings, clear). A plugin's own
// settings (one added from a zip or an address) are saved with PUT /api/plugins/{id}/settings. What a plugin says
// (its name, description, status, log, homepage) is shown as text; only an http(s) homepage is opened.

/** One plugin's log as the drawer shows it. */
private data class LogView(val text: String, val file: String = "")

/** A zip picked for Upload plugin, waiting for the third-party warning to be answered. */
private class PickedZip(val uri: Uri, val name: String)

/** What the Plugins page holds while it is open: the list, the settings answers, what each plugin is doing. */
@Stable
private class PluginsPage(private val api: SettingsApi, private val schemaApi: SchemaApi) {
    /** GET /api/plugins without its list: the sandbox line and the upload limit. */
    var head by mutableStateOf<JsonObject?>(null)
    var plugins by mutableStateOf<List<JsonObject>>(emptyList())
    var loadError by mutableStateOf<String?>(null)
    var loaded by mutableStateOf(false)

    /** The settings answers of the cards drawn with a form, by plugin (`_error` when it could not be read). */
    val forms = mutableStateMapOf<String, JsonObject>()

    /** The form states of plugins' own settings (a built-in plugin uses the Server pages' shared state). */
    val own = HashMap<String, SchemaFormState>()

    /** What this page is doing to a plugin: install, update, remove, enable, disable, test. */
    val acting = mutableStateMapOf<String, String>()

    /** The line under a card's buttons: a Test's answer, or a failure in full. */
    val outs = mutableStateMapOf<String, List<ResultLine>>()

    /** The lines of a part's Test (YouTube sign-in's Test cookies), by "<plugin>/<part>"; true while it runs. */
    val partOuts = mutableStateMapOf<String, List<ResultLine>>()
    val partTesting = mutableStateMapOf<String, Boolean>()

    val open = mutableStateMapOf<String, Boolean>()
    val logs = mutableStateMapOf<String, LogView>()

    var checking by mutableStateOf(false)

    /** Add a plugin: what it is doing ("Uploading and checking…") and its failure. */
    var adding by mutableStateOf<String?>(null)
    var addError by mutableStateOf<String?>(null)

    /** The card (and part) to scroll to once drawn: a `?s=` link's, a new install's. */
    var scrollTo by mutableStateOf<String?>(null)
    var scrollPart by mutableStateOf<String?>(null)

    /** The Server pages' form state (built-in plugins' settings). */
    var shared: SchemaFormState? = null

    fun card(id: String): JsonObject =
        plugins.firstOrNull { it.str("id") == id } ?: JsonObject(mapOf("id" to JsonPrimitive(id)))

    fun setCard(c: JsonObject) {
        val id = c.str("id")
        if (id.isEmpty()) return
        plugins = plugins.map { if (it.str("id") == id) c else it }
    }

    val installedIds: Set<String> get() = plugins.filter { it.bool("installed") }.map { it.str("id") }.toSet()

    val sandboxFull: Boolean get() = PluginRules.sandbox(head?.obj("sandbox")).full

    /** The form state of a plugin's card: the shared one for a built-in plugin, its own otherwise. */
    fun stateFor(id: String): SchemaFormState? {
        val answer = forms[id] ?: return null
        if (answer.str("_error").isNotEmpty()) return null
        return if (answer.bool("builtin")) shared else own[id]
    }

    /** GET /api/plugins, then the settings of every installed plugin with a card of its own, then each one's status. */
    suspend fun load() {
        try {
            val l = withContext(Dispatchers.IO) { api.plugins() }
            val cards = l.arr("plugins").objectsOf()
            val ids = cards.filter { PluginRules.hasCard(it) }.map { it.str("id") }
            val answers = coroutineScope {
                ids.map { id ->
                    async(Dispatchers.IO) {
                        try {
                            api.pluginSettings(id)
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            JsonObject(mapOf("_error" to JsonPrimitive(PluginRules.errorText(e.message, (e as? ApiException)?.body))))
                        }
                    }
                }.awaitAll()
            }
            forms.clear()
            own.clear()
            for ((id, answer) in ids.zip(answers)) {
                forms[id] = answer
                if (answer.str("_error").isEmpty() && !answer.bool("builtin")) own[id] = ownState(id, answer)
            }
            head = JsonObject(l.filterKeys { it != "plugins" })
            plugins = cards
            loadError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            loadError = PluginRules.errorText(e.message, (e as? ApiException)?.body)
        }
        loaded = true
        coroutineScope {
            for (c in plugins) if (c.bool("installed") && c.bool("enabled")) launch { statusOf(c.str("id")) }
        }
    }

    /** The form state of a plugin's own settings: Save and clear go to PUT /api/plugins/{id}/settings. */
    private fun ownState(id: String, answer: JsonObject): SchemaFormState =
        SchemaFormState(shared?.schema ?: SettingsSchema(), PluginRules.cardValues(answer, null), canReveal = false) { body ->
            PluginRules.cardValues(api.pluginSaveSettings(id, body), null)
        }

    /** One plugin's card with its status asked now (GET /api/plugins/{id}). */
    suspend fun statusOf(id: String) {
        try {
            val c = withContext(Dispatchers.IO) { api.plugin(id) }
            if (acting[id] == null) setCard(c)
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
        }
    }

    /** Install, Update, Enable, Disable and Remove (Remove's question is asked before). */
    suspend fun act(id: String, act: String) {
        if (acting[id] != null) return
        val before = card(id)
        acting[id] = act
        outs.remove(id)
        try {
            val c = withContext(Dispatchers.IO) { api.pluginAction(id, act) }
            acting.remove(id)
            Toasts.show(PluginRules.doneWords(act, before, c), "ok")
            when (act) {
                "install" -> {
                    // the new card, open, where it now is (Installed, above the list it came from)
                    open[id] = true
                    scrollTo = id
                    load()
                }
                "remove" -> {
                    open.remove(id)
                    logs.remove(id)
                    load()
                }
                else -> {
                    setCard(c)
                    if (act != "disable") statusOf(id)
                }
            }
        } catch (e: CancellationException) {
            acting.remove(id)
            throw e
        } catch (e: Exception) {
            acting.remove(id)
            outs[id] = listOf(ResultLine(false, PluginRules.errorText(e.message, (e as? ApiException)?.body)))
            // the card as the server has it now (its last error)
            statusOf(id)
        }
    }

    /** Test: the card's settings saved quietly first (as the cards always did), then POST /api/plugins/{id}/test. */
    suspend fun test(id: String) {
        if (acting[id] != null) return
        acting[id] = "test"
        outs.remove(id)
        try {
            val answer = forms[id]
            val state = stateFor(id)
            val failed = if (answer != null && state != null) state.save(schemaApi, PluginRules.groupOf(answer, PluginRules.nameOf(card(id))), quiet = true) else null
            val line = if (failed != null) {
                ResultLine(false, failed)
            } else {
                try {
                    PluginRules.testAnswer(withContext(Dispatchers.IO) { api.pluginTest(id) })
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    ResultLine(false, PluginRules.errorText(e.message, (e as? ApiException)?.body))
                }
            }
            outs[id] = listOf(line)
        } finally {
            acting.remove(id)
        }
        statusOf(id)
    }

    /** A part's Test (YouTube sign-in's Test cookies): the card saved quietly, then POST /api/system/test/<test>. */
    suspend fun partTest(id: String, part: String, test: String) {
        val k = "$id/$part"
        if (partTesting[k] == true) return
        partTesting[k] = true
        partOuts.remove(k)
        try {
            val answer = forms[id]
            val state = stateFor(id)
            val failed = if (answer != null && state != null) state.save(schemaApi, PluginRules.groupOf(answer, PluginRules.nameOf(card(id))), quiet = true) else null
            partOuts[k] = listOf(
                if (failed != null) {
                    SchemaRules.testFailed(failed)
                } else {
                    try {
                        SchemaRules.testLine(withContext(Dispatchers.IO) { schemaApi.test(test) })
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        SchemaRules.testFailed(e.message)
                    }
                },
            )
        } finally {
            partTesting.remove(k)
        }
    }

    /** Save of a card: a built-in plugin's keys with PUT /api/admin/settings, a plugin's own with PUT /api/plugins/{id}/settings. */
    suspend fun save(id: String) {
        val answer = forms[id] ?: return
        val state = stateFor(id) ?: return
        val group = PluginRules.groupOf(answer, PluginRules.nameOf(card(id)))
        if (!answer.bool("builtin")) {
            state.save(schemaApi, group, quiet = false)
            return
        }
        val model = SchemaRules.textOf(state.values["translator_model"])
        if (state.save(schemaApi, group, quiet = true) != null) return
        val changedModel = id == "translator" && SchemaRules.textOf(state.values["translator_model"]) != model
        Toasts.show(if (changedModel) "Saved – Install or Update the Translator to download this model" else "Settings saved", "ok")
    }

    /** The auto-update switch. */
    suspend fun autoUpdate(id: String, on: Boolean) {
        try {
            val c = withContext(Dispatchers.IO) { api.pluginAutoUpdate(id, on) }
            setCard(c)
            Toasts.show("Saved", "ok")
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Toasts.show(e.message ?: "Something went wrong", "err")
        }
    }

    /** The logs drawer: [toggle] opens or closes it (reading the log when it opens), else the log is read again. */
    suspend fun readLog(id: String, toggle: Boolean) {
        if (toggle && logs.containsKey(id)) {
            logs.remove(id)
            return
        }
        logs[id] = LogView("Loading…", logs[id]?.file.orEmpty())
        logs[id] = try {
            val r = withContext(Dispatchers.IO) { api.pluginLogs(id) }
            val lines = r.arr("lines").strings()
            LogView(if (lines.isEmpty()) "The log is empty." else lines.joinToString("\n"), r.str("file"))
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            LogView(PluginRules.errorText(e.message, (e as? ApiException)?.body))
        }
    }

    /** Check for updates: every installed plugin that has a place to update from. */
    suspend fun checkAll() {
        if (checking) return
        checking = true
        try {
            val l = withContext(Dispatchers.IO) { api.pluginsCheck() }
            for (c in l.arr("plugins").objectsOf()) if (acting[c.str("id")] == null) setCard(c)
            Toasts.show(PluginRules.checkWords(PluginRules.updates(l)), "ok")
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Toasts.show(PluginRules.errorText(e.message, (e as? ApiException)?.body), "err")
        } finally {
            checking = false
        }
    }

    /**
     * While a plugin is being installed, updated or removed elsewhere (another device, the scheduled task, the first
     * start after an upgrade), the list is asked again every 3 s; once that is done the page shows the result.
     */
    suspend fun pollBusy() {
        while (true) {
            delay(3000)
            val l = try {
                withContext(Dispatchers.IO) { api.plugins() }
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                continue
            }
            if (PluginRules.changedShape(l, installedIds)) {
                load()
                return
            }
            val cards = l.arr("plugins").objectsOf()
            for (c in cards) if (acting[c.str("id")] == null) setCard(c)
            if (cards.none { it.str("busy").isNotEmpty() }) return
        }
    }

    /** Upload plugin, after the warning: the zip streamed as multipart. */
    suspend fun upload(context: Context, zip: PickedZip) {
        adding = "Uploading and checking ${zip.name}…"
        addError = null
        try {
            val c = withContext(Dispatchers.IO) { api.pluginUpload(zip.name, zipBody(context, zip.uri)) }
            added(c)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            addError = PluginRules.errorText(e.message, (e as? ApiException)?.body)
        } finally {
            adding = null
        }
    }

    /** Install from URL or GitHub, after the warning. True when it installed. */
    suspend fun installUrl(input: String): Boolean {
        adding = "Downloading and checking…"
        addError = null
        try {
            val c = withContext(Dispatchers.IO) { api.pluginInstallUrl(input.trim()) }
            added(c)
            return true
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            addError = PluginRules.errorText(e.message, (e as? ApiException)?.body)
            return false
        } finally {
            adding = null
        }
    }

    /** After an upload or a URL install: the new card, open. */
    private suspend fun added(c: JsonObject) {
        Toasts.show(PluginRules.addedWords(c), "ok")
        val id = c.str("id")
        open[id] = true
        scrollTo = id
        load()
    }
}

/**
 * Settings → Server → Plugins. [focus]: the card a `#/settings?s=…` link named (a moved card's name opens its
 * plugin's card; `youtube` the YouTube sign-in part of yt-dlp's).
 */
@Composable
fun PluginsScreen(
    schema: ServerSchema,
    schemaApi: SchemaApi,
    api: SettingsApi,
    focus: String?,
    onBack: () -> Unit,
    onOpen: (String) -> Unit,
) {
    val page = remember { PluginsPage(api, schemaApi) }
    val state = schema.state
    page.shared = state
    val scope = rememberCoroutineScope()
    var version by remember { mutableStateOf(0) }
    LaunchedEffect(Unit) {
        if (focus != null) {
            val id = PluginRules.cardFor(focus)
            page.open[id] = true
            page.scrollTo = id
            if (focus == "youtube") page.scrollPart = "YouTube sign-in"
        }
    }
    // the built-in plugins' forms need the Server pages' schema and values: the list waits for them
    LaunchedEffect(state != null, version) { if (state != null) page.load() }
    val anyBusy = page.plugins.any { it.str("busy").isNotEmpty() }
    LaunchedEffect(anyBusy) { if (anyBusy) page.pollBusy() }

    SettingsPage("Plugins", onBack) {
        if (state == null) {
            if (schema.error != null) ErrorBox(schema.error) { schema.retry() } else Spinner(24.dp)
            return@SettingsPage
        }
        val err = page.loadError
        if (!page.loaded) {
            Spinner(24.dp)
            return@SettingsPage
        }
        if (err != null && page.plugins.isEmpty()) {
            SetCard {
                CardHead("Plugins")
                ResultLines(false, listOf(ResultLine(false, err)))
                SmallButton("Try again", iconRes = R.drawable.ic_refresh) { version += 1 }
            }
            return@SettingsPage
        }
        HeadCard(page, onOpen) { scope.launch { page.checkAll() } }
        Label("Installed")
        val installed = page.plugins.filter { it.bool("installed") }
        if (installed.isEmpty()) {
            SetCard { Muted("No plugins installed yet. Install the ones you want to use from the list below.") }
        }
        for (c in installed) {
            key(c.str("id")) {
                InstalledCard(page, c, state, schemaApi, api, onOpen)
                Spacer(Modifier.height(12.dp))
            }
        }
        val available = page.plugins.filter { !it.bool("installed") }
        if (available.isNotEmpty()) {
            Label("Available")
            SetCard(flat = true) {
                available.forEachIndexed { i, c ->
                    key(c.str("id")) { AvailableRow(page, c, first = i == 0) }
                }
            }
        }
        AddCard(page)
    }
}

/** A heading over a run of cards (the web's `.nav-label`). */
@Composable
private fun Label(text: String) {
    Text(
        text.uppercase(),
        style = MaterialTheme.typography.labelSmall,
        fontWeight = FontWeight.SemiBold,
        color = MaterialTheme.palette.muted,
        modifier = Modifier.padding(start = 4.dp, end = 4.dp, top = 18.dp, bottom = 8.dp),
    )
}

/** The page's first card: Check for updates and the sandbox line. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun HeadCard(page: PluginsPage, onOpen: (String) -> Unit, onCheck: () -> Unit) {
    val p = MaterialTheme.palette
    val sb = PluginRules.sandbox(page.head?.obj("sandbox"))
    var reasonOpen by remember { mutableStateOf(false) }
    SetCard {
        CardHead("Plugins") {
            SmallButton(if (page.checking) "Checking…" else "Check for updates", iconRes = R.drawable.ic_refresh, enabled = !page.checking) { onCheck() }
        }
        Muted("Download sources and clients, server tools and connections to other music servers. Each one installs, updates and is removed on its own and is kept in the config folder, so no new Encore image is needed. Installed plugins are checked for updates daily; change the schedule under")
        TextLink("Scheduled tasks") { onOpen("tasks") }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text("Sandbox", style = MaterialTheme.typography.bodySmall, fontWeight = FontWeight.Bold, modifier = Modifier.align(Alignment.CenterVertically))
            Badge(sb.text, kind = if (sb.full) "ok" else "warn", modifier = Modifier.align(Alignment.CenterVertically))
        }
        Muted(sb.what)
        if (sb.reason.isNotEmpty()) {
            // the probe's reason: one line, the whole of it (selectable, with Copy) when tapped
            Row(verticalAlignment = Alignment.Top) {
                if (reasonOpen) {
                    SelectionContainer(Modifier.weight(1f)) { Text(sb.reason, style = MaterialTheme.typography.bodySmall, color = p.faint) }
                    CopyIconButton(sb.reason, size = 24.dp)
                } else {
                    Text(
                        sb.reason,
                        style = MaterialTheme.typography.bodySmall,
                        color = p.faint,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f).clickable { reasonOpen = true },
                    )
                }
            }
        }
    }
}

/** The dot beside an installed plugin's name. */
@Composable
private fun StateDot(tone: String) {
    val color = when (tone) {
        "ok" -> EncoreColors.Ok
        "err" -> EncoreColors.Err
        "warn" -> EncoreColors.Warn
        else -> MaterialTheme.palette.faint
    }
    Box(Modifier.size(9.dp).clip(CircleShape).background(color))
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Badges(c: JsonObject) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        for (b in PluginRules.badges(c)) Badge(b.text, kind = b.kind)
    }
}

/** The status line: what the plugin is doing, whether it works (a failure selectable, with Copy), and its last error. */
@Composable
private fun StatusLine(c: JsonObject, acting: String?) {
    val p = MaterialTheme.palette
    val st = PluginRules.status(c, acting) ?: return
    Row(verticalAlignment = Alignment.CenterVertically) {
        if (st.busy) {
            Spinner(12.dp)
            Spacer(Modifier.width(6.dp))
        }
        if (st.error) {
            SelectionContainer(Modifier.weight(1f)) { Text(st.text, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err) }
            CopyIconButton(st.text, size = 24.dp)
        } else {
            Text(st.text, style = MaterialTheme.typography.bodySmall, color = if (st.muted) p.muted else p.text)
        }
    }
    if (st.lastError.isNotEmpty()) {
        Row(verticalAlignment = Alignment.Top) {
            SelectionContainer(Modifier.weight(1f)) { Text(st.lastError, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err) }
            CopyIconButton(st.lastError, size = 24.dp)
        }
    }
}

/** What a source finds, the sandbox and permissions of a third-party plugin, its licence and homepage. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AboutLine(c: JsonObject) {
    val context = LocalContext.current
    val parts = PluginRules.aboutParts(c)
    val home = PluginRules.homepage(c)
    if (parts.isEmpty() && home.isEmpty()) return
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        if (parts.isNotEmpty()) Muted(parts.joinToString(" · "), Modifier.align(Alignment.CenterVertically))
        if (home.isNotEmpty()) TextLink("Homepage") { openUrl(context, home) }
    }
}

/** The buttons of a card or row; the one this page is running shows its spinner and words. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Actions(c: JsonObject, acting: String?, testLabel: String, onAct: (String) -> Unit) {
    val busy = acting != null || c.str("busy").isNotEmpty()
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (a in PluginRules.actions(c, testLabel)) {
            if (acting == a.act) {
                Row(Modifier.align(Alignment.CenterVertically), verticalAlignment = Alignment.CenterVertically) {
                    Spinner(12.dp)
                    Spacer(Modifier.width(6.dp))
                    Muted(PluginRules.busyWords(a.act))
                }
            } else {
                val icon = when (a.act) {
                    "install", "update" -> R.drawable.ic_download
                    "test" -> R.drawable.ic_refresh
                    "logs" -> R.drawable.ic_note
                    "enable" -> R.drawable.ic_check
                    "remove" -> R.drawable.ic_delete
                    else -> null
                }
                SmallButton(a.label, iconRes = icon, primary = a.primary, danger = a.danger, enabled = !busy || a.act == "logs") { onAct(a.act) }
            }
        }
    }
}

/** An installed plugin: folded to its name, version, badges and description; open, everything else. */
@Composable
private fun InstalledCard(page: PluginsPage, c: JsonObject, state: SchemaFormState, schemaApi: SchemaApi, api: SettingsApi, onOpen: (String) -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val id = c.str("id")
    val open = page.open[id] == true
    val acting = page.acting[id]
    val shape = RoundedCornerShape(14.dp)
    val requester = remember { BringIntoViewRequester() }
    var removing by remember { mutableStateOf(false) }
    LaunchedEffect(page.scrollTo, page.scrollPart) {
        if (page.scrollTo != id || page.scrollPart != null) return@LaunchedEffect
        withFrameNanos { }
        requester.bringIntoView()
        page.scrollTo = null
    }
    Column(Modifier.fillMaxWidth().bringIntoViewRequester(requester).clip(shape).background(p.panel).border(1.dp, p.border, shape)) {
        Row(
            Modifier.fillMaxWidth().clickable { page.open[id] = !open }.padding(14.dp),
            verticalAlignment = Alignment.Top,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    PluginRules.dotTone(c)?.let { StateDot(it); Spacer(Modifier.width(8.dp)) }
                    Text(PluginRules.nameOf(c), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f, fill = false))
                    Spacer(Modifier.width(8.dp))
                    Mono(PluginRules.versionText(c), color = p.muted)
                }
                Badges(c)
                Muted(c.str("description"))
            }
            Icon(
                painterResource(R.drawable.ic_expand_more),
                contentDescription = null,
                tint = p.muted,
                modifier = Modifier.padding(top = 2.dp).rotate(if (open) 180f else 0f),
            )
        }
        if (!open) return@Column
        Column(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, bottom = 14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (c.str("warning").isNotEmpty()) {
                Row(verticalAlignment = Alignment.Top) {
                    Icon(painterResource(R.drawable.ic_warning), contentDescription = null, tint = EncoreColors.Warn, modifier = Modifier.size(16.dp).padding(top = 1.dp))
                    Spacer(Modifier.width(8.dp))
                    Text(c.str("warning"), style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn)
                }
            }
            StatusLine(c, acting)
            AboutLine(c)
            Actions(c, acting, PluginRules.testLabel(state.schema, id)) { act ->
                when (act) {
                    "logs" -> scope.launch { page.readLog(id, toggle = true) }
                    "test" -> scope.launch { page.test(id) }
                    "remove" -> removing = true
                    else -> scope.launch { page.act(id, act) }
                }
            }
            if (c.bool("can_update")) {
                SwitchRow(PluginRules.autoUpdateWords(id), c.bool("auto_update"), enabled = acting == null && c.str("busy").isEmpty()) { on ->
                    scope.launch { page.autoUpdate(id, on) }
                }
            }
            ResultLines(acting == "test", page.outs[id].orEmpty())
            page.logs[id]?.let { LogsDrawer(it, onRefresh = { scope.launch { page.readLog(id, toggle = false) } }) }
            val on = c.str("settings_on")
            when {
                on.isNotEmpty() -> Row(verticalAlignment = Alignment.CenterVertically) {
                    Muted("Its settings are on")
                    Spacer(Modifier.width(4.dp))
                    TextLink("Settings → ${PluginRules.settingsOnTitle(state.schema, on)}") { onOpen(on) }
                }
                PluginRules.hasCard(c) -> {
                    val answer = page.forms[id]
                    val error = answer?.str("_error").orEmpty()
                    val formState = page.stateFor(id)
                    when {
                        answer == null -> Spinner(16.dp)
                        error.isNotEmpty() -> ResultLines(false, listOf(ResultLine(false, error)))
                        formState != null -> PluginForm(page, c, answer, formState, schemaApi, api)
                    }
                }
            }
        }
    }
    if (removing) {
        ConfirmDialog("Remove ${PluginRules.nameOf(c)}", PluginRules.removeText(c), "Remove", danger = true, onDismiss = { removing = false }) {
            removing = false
            scope.launch { page.act(id, "remove") }
        }
    }
}

/** The logs drawer: the last 200 lines, selectable, scrolled to the end, with Refresh, Copy and the file. */
@Composable
private fun LogsDrawer(log: LogView, onRefresh: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scroll = rememberScrollState()
    LaunchedEffect(log.text) {
        withFrameNanos { }
        scroll.scrollTo(scroll.maxValue)
    }
    SetCard(flat = true) {
        CardHead("Log") {
            SmallButton("Refresh", iconRes = R.drawable.ic_refresh, onClick = onRefresh)
            SmallButton("Copy", iconRes = R.drawable.ic_content_copy) { copyToClipboard(context, log.text) }
        }
        if (log.file.isNotEmpty()) Mono(log.file, color = p.faint)
        Box(Modifier.fillMaxWidth().heightIn(max = 320.dp).verticalScroll(scroll)) {
            SelectionContainer { Text(log.text, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace) }
        }
    }
}

/**
 * The settings of a plugin card: its fields run by run, a part with its heading (a part that was a card before 0.32
 * keeps that card's description and Test; YouTube sign-in its block), the extras of the built-in cards (Tidal sign-in
 * and the streamrip config, the Soulseek status and VPN block), the client notes and Save.
 */
@Composable
private fun PluginForm(page: PluginsPage, c: JsonObject, answer: JsonObject, state: SchemaFormState, schemaApi: SchemaApi, api: SettingsApi) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val id = c.str("id")
    val group = remember(answer, c.str("name")) { PluginRules.groupOf(answer, PluginRules.nameOf(c)) }
    val runs = remember(answer) { PluginRules.runs(answer) }
    val slots = remember(api) { sourceSlots(api) }[id] ?: CardSlots.None
    val cardScope = remember(group, state, schemaApi, scope) { SchemaCardScope(group, state, schemaApi, scope, { null }) }
    Column(Modifier.fillMaxWidth().padding(top = 8.dp)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(p.border))
        Spacer(Modifier.height(12.dp))
        // the warnings a Save came back with (a folder Encore can't write to)
        ResultLines(false, state.results[group.id].orEmpty())
        if (c.str("kind") == "client") {
            PluginRules.clientNote(c, answer, state.values, state.schema)?.let { n ->
                Text(n.text, style = MaterialTheme.typography.bodySmall, color = if (n.inUse) p.muted else EncoreColors.Warn, modifier = Modifier.padding(bottom = 6.dp))
            }
        }
        for (run in runs) {
            if (run.part.isNotEmpty()) PartHead(page, api, id, run.part, state.schema)
            SchemaFields(state, schemaApi, group, run.fields, slots, cardScope)
        }
        AdvancedToggle(state, group, group.fields)
        slots.below?.invoke(this, cardScope)
        if (id == "torznab") ChosenClients(page, state)
        Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.End) {
            EncoreButton("Save", iconRes = R.drawable.ic_check, primary = true, enabled = state.saving[group.id] != true && page.acting[id] != "test") {
                scope.launch { page.save(id) }
            }
        }
    }
}

/** A part's heading: its title, the description and Test of the card it was before 0.32, and YouTube sign-in's block. */
@Composable
private fun PartHead(page: PluginsPage, api: SettingsApi, id: String, part: String, schema: SettingsSchema) {
    val scope = rememberCoroutineScope()
    val info = remember(schema, part) { PluginRules.partInfo(schema, part) }
    val requester = remember { BringIntoViewRequester() }
    val k = "$id/$part"
    LaunchedEffect(page.scrollTo, page.scrollPart) {
        if (page.scrollTo != id || page.scrollPart != part) return@LaunchedEffect
        withFrameNanos { }
        requester.bringIntoView()
        page.scrollTo = null
        page.scrollPart = null
    }
    Column(Modifier.fillMaxWidth().bringIntoViewRequester(requester).padding(top = 18.dp, bottom = 4.dp)) {
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Column(Modifier.weight(1f)) {
                Text(part, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
                if (info.desc.isNotEmpty()) Muted(info.desc, Modifier.padding(top = 4.dp))
            }
            if (info.test.isNotEmpty()) {
                SmallButton(info.testLabel, iconRes = R.drawable.ic_refresh, enabled = page.partTesting[k] != true) {
                    scope.launch { page.partTest(id, part, info.test) }
                }
            }
        }
        Spacer(Modifier.height(8.dp))
        ResultLines(page.partTesting[k] == true, page.partOuts[k].orEmpty())
        if (info.card == "youtube") YouTubeCard(api)
    }
}

/** The Torrents & Usenet card: a chosen client whose plugin is not installed or is off, with Install or Enable. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ChosenClients(page: PluginsPage, state: SchemaFormState) {
    val scope = rememberCoroutineScope()
    for (cc in PluginRules.chosenClients(state.values, page.plugins, state.schema)) {
        FlowRow(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(cc.text, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn, modifier = Modifier.align(Alignment.CenterVertically))
            val busy = page.acting[cc.plugin]
            if (busy != null) {
                Row(Modifier.align(Alignment.CenterVertically), verticalAlignment = Alignment.CenterVertically) {
                    Spinner(12.dp)
                    Spacer(Modifier.width(6.dp))
                    Muted(PluginRules.busyWords(busy))
                }
            } else if (cc.act == "install") {
                SmallButton("Install", iconRes = R.drawable.ic_download) { scope.launch { page.act(cc.plugin, "install") } }
            } else {
                SmallButton("Enable") { scope.launch { page.act(cc.plugin, "enable") } }
            }
        }
        page.outs[cc.plugin]?.let { ResultLines(false, it) }
    }
}

/** A plugin that is not installed: a row of the Available list with Install. */
@Composable
private fun AvailableRow(page: PluginsPage, c: JsonObject, first: Boolean) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val id = c.str("id")
    if (!first) Box(Modifier.fillMaxWidth().height(1.dp).background(p.border))
    Column(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(PluginRules.nameOf(c), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f, fill = false))
            Spacer(Modifier.width(8.dp))
            Mono(c.str("version"), color = p.muted)
        }
        Badges(c)
        Muted(c.str("description"))
        AboutLine(c)
        StatusLine(c, page.acting[id])
        Actions(c, page.acting[id], "Test") { act -> scope.launch { page.act(id, act) } }
        page.outs[id]?.let { ResultLines(false, it) }
    }
}

/** Upload plugin and Install from URL or GitHub, with the third-party warning. */
@Composable
private fun AddCard(page: PluginsPage) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var url by remember { mutableStateOf("") }
    // the warning before installing: a picked zip, or the address typed
    var askZip by remember { mutableStateOf<PickedZip?>(null) }
    var askUrl by remember { mutableStateOf<String?>(null) }
    val max = PluginRules.uploadMaxMb(page.head)
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        val (name, size) = fileInfo(context, uri)
        val problem = PluginRules.fileProblem(name, size, max)
        if (problem != null) {
            page.addError = problem
        } else {
            page.addError = null
            askZip = PickedZip(uri, name)
        }
    }
    val busy = page.adding != null
    SetCard(Modifier.padding(top = 18.dp)) {
        Text("Add a plugin", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        Muted("A plugin is a zip with a plugin.toml and its program, for example one made with Encore's plugin SDK (docs/plugins in Encore's repository). Plugins you add are not made by Encore: install only plugins you trust.")
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            EncoreButton("Upload plugin", iconRes = R.drawable.ic_upload, enabled = !busy) {
                pick.launch(arrayOf("application/zip", "application/x-zip-compressed", "application/octet-stream", "*/*"))
            }
            Muted("A .zip of at most $max MB", Modifier.weight(1f))
        }
        OutlinedTextField(
            value = url,
            onValueChange = { url = it },
            placeholder = { Text("https://… or owner/repo@ref") },
            singleLine = true,
            enabled = !busy,
            keyboardOptions = KeyboardOptions(
                capitalization = KeyboardCapitalization.None,
                autoCorrectEnabled = false,
                keyboardType = KeyboardType.Uri,
                imeAction = ImeAction.Done,
            ),
            modifier = Modifier.fillMaxWidth(),
        )
        EncoreButton("Install from URL", iconRes = R.drawable.ic_link, enabled = !busy) {
            val problem = PluginRules.urlProblem(url)
            if (problem != null) {
                page.addError = problem
            } else {
                page.addError = null
                askUrl = url
            }
        }
        Muted("The address of a plugin zip or of its plugin.toml, or a GitHub repository as owner/repo@ref (a branch, tag or commit; owner/repo/folder@ref for a plugin in a folder).")
        page.adding?.let { words ->
            Row(verticalAlignment = Alignment.CenterVertically) {
                Spinner(14.dp)
                Spacer(Modifier.width(8.dp))
                Muted(words)
            }
        }
        page.addError?.let { ResultLines(false, listOf(ResultLine(false, it))) }
    }
    askZip?.let { zip ->
        ConfirmDialog("Install a plugin you add", PluginRules.trustText(page.sandboxFull), "Install", onDismiss = { askZip = null }) {
            askZip = null
            scope.launch { page.upload(context, zip) }
        }
    }
    askUrl?.let { typed ->
        ConfirmDialog("Install a plugin you add", PluginRules.trustText(page.sandboxFull), "Install", onDismiss = { askUrl = null }) {
            askUrl = null
            scope.launch { if (page.installUrl(typed)) url = "" }
        }
    }
}

/** The picked file's name ("plugin.zip" when the provider does not say) and size (-1 when it does not say). */
private fun fileInfo(context: Context, uri: Uri): Pair<String, Long> {
    var name: String? = null
    var size = -1L
    try {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
            if (c.moveToFirst()) {
                name = c.getString(0)
                if (!c.isNull(1)) size = c.getLong(1)
            }
        }
    } catch (_: Exception) {
    }
    return (name?.takeIf { it.isNotBlank() } ?: "plugin.zip") to size
}

/** The picked zip streamed into the upload (never held in memory whole). */
private fun zipBody(context: Context, uri: Uri): RequestBody = object : RequestBody() {
    override fun contentType(): MediaType = "application/zip".toMediaType()
    override fun writeTo(sink: BufferedSink) {
        val input = context.contentResolver.openInputStream(uri) ?: throw java.io.IOException("Couldn't read the file")
        input.source().use { sink.writeAll(it) }
    }
}
