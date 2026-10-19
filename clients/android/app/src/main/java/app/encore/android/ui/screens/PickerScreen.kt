package app.encore.android.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.str
import app.encore.android.data.api.ApiException
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.PickResult
import app.encore.android.data.repo.PickSearch
import app.encore.android.data.repo.PickerInfo
import app.encore.android.data.repo.PickerOpts
import app.encore.android.data.repo.PickerSource
import app.encore.android.features.picker.PickerRules
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.components.formatSize
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

private fun isUrl(v: String): Boolean = Regex("^https?://\\S+$", RegexOption.IGNORE_CASE).matches(v.trim())

/** A failed search on one source: the message and, for a 409 (signed out, stopped, off), the settings link. */
private class PickErr(val message: String, val settings: String = "", val label: String = "")

private fun pickErr(e: Exception): PickErr {
    val body = (e as? ApiException)?.body
    return PickErr(e.message ?: "Search failed", body?.str("settings") ?: "", body?.str("settings_label") ?: "")
}

/** The web UI's settings page for a `#/settings?s=…` link, on this server (for people without the phone's server settings). */
private fun settingsUrl(nav: Nav, link: String): String = nav.graph.api.base.trimEnd('/') + "/" + link.trimStart('/')

/** The page or card a `#/settings?s=slskd` link names ("" for Settings itself). */
private fun settingsSection(link: String): String = Regex("[?&]s=([^&]+)").find(link)?.groupValues?.get(1).orEmpty()

/**
 * "Sign in to Tidal – Settings → streamrip": a hint with its settings link. Admins go to that page of the phone's own
 * Settings; everyone else gets the web UI's page.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun HintLine(nav: Nav, admin: Boolean, hint: String, settings: String, label: String, error: Boolean = false) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    FlowRow(Modifier.padding(horizontal = Gutter, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        if (hint.isNotEmpty()) Text(hint, style = MaterialTheme.typography.bodySmall, color = if (error) EncoreColors.Err else p.muted)
        if (settings.isNotEmpty()) {
            Text(
                (if (hint.isNotEmpty()) "– " else "") + label.ifEmpty { "Settings" },
                style = MaterialTheme.typography.bodySmall, color = p.accent,
                modifier = Modifier.clickable(role = Role.Button) {
                    val section = settingsSection(settings)
                    when {
                        !admin -> openUrl(context, settingsUrl(nav, settings))
                        section.isEmpty() -> nav.open(Screen.Settings)
                        else -> nav.open(Screen.SettingsAt(section))
                    }
                },
            )
        }
    }
}

/**
 * The line of a kept answer (req-slsk-cache §5): "Found at 14:02 · Search again", while it runs "Searching Soulseek
 * again…" (or the wait for another Soulseek search), and the error of a "Search again" that failed (the rows stay).
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun FoundRow(text: String, again: Boolean, running: String?, error: String?, onAgain: () -> Unit) {
    val p = MaterialTheme.palette
    if (running != null) {
        Row(Modifier.padding(horizontal = Gutter, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Spinner(); Spacer(Modifier.width(8.dp))
            Text(running, style = MaterialTheme.typography.bodySmall, color = p.muted)
        }
    } else if (text.isNotEmpty() || again) {
        FlowRow(Modifier.padding(horizontal = Gutter, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            if (text.isNotEmpty()) Text(text, style = MaterialTheme.typography.bodySmall, color = p.muted)
            if (again) {
                Text(
                    (if (text.isNotEmpty()) "· " else "") + "Search again",
                    style = MaterialTheme.typography.bodySmall, color = p.accent,
                    modifier = Modifier.clickable(role = Role.Button, onClick = onAgain),
                )
            }
        }
    }
    if (error != null && running == null) {
        Text(error, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err, modifier = Modifier.padding(horizontal = Gutter, vertical = 2.dp))
    }
}

/**
 * The manual download picker (web/js/pages/picker.js): search every source, see quality / size / uploader and
 * download the exact result you choose. Opened for an album or a song, or for an existing request.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PickerScreen(nav: Nav, opts: PickerOpts) {
    val features = nav.features
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val info = rememberLoad("picker-sources") { features.pickerSources() }
    var query by remember { mutableStateOf(listOf(opts.artist, opts.title.ifEmpty { opts.album }).filter { it.isNotEmpty() }.joinToString(" ")) }
    var linkUrl by remember { mutableStateOf("") }
    var tab by remember { mutableStateOf(0) }
    var lossless by remember { mutableStateOf(false) }
    val results = remember { mutableStateMapOf<String, List<PickResult>>() }
    val errors = remember { mutableStateMapOf<String, PickErr>() }
    val loading = remember { mutableStateMapOf<String, Boolean>() }
    // a kept answer's fields per source (a server that keeps Soulseek and indexer searches; never on today's)
    val kept = remember { mutableStateMapOf<String, PickSearch>() }
    // "Search again" running for a source (its rows stay), and why one failed
    val again = remember { mutableStateMapOf<String, Boolean>() }
    val againErrors = remember { mutableStateMapOf<String, String>() }
    // another Soulseek search holds the turn and this one waits for it
    var slskWaiting by remember { mutableStateOf(false) }
    var searchGen by remember { mutableStateOf(0) }
    var busy by remember { mutableStateOf(false) }
    var me by remember { mutableStateOf<Me?>(null) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    LaunchedEffect(Unit) { me = try { withContext(Dispatchers.IO) { features.me() } } catch (e: CancellationException) { throw e } catch (_: Exception) { null } }

    /** One answer in place: its rows, and its kept-search fields when it came from a kept search. */
    fun land(id: String, r: PickSearch) {
        results[id] = r.items
        if (r.kept) kept[id] = r else kept.remove(id)
    }

    /**
     * Runs a Soulseek search after asking whether another one holds the turn (asked first, so it can't be this one);
     * the waiting line shows only when the answer takes more than a moment (a kept answer comes at once).
     */
    suspend fun <T> soulseekTurn(id: String, block: suspend () -> T): T = coroutineScope {
        val running = id == "slskd" && withContext(Dispatchers.IO) { features.soulseekSearchRunning() } == true
        val shown = if (running) launch { delay(1000); slskWaiting = true } else null
        try {
            block()
        } finally {
            shown?.cancel()
            if (id == "slskd") slskWaiting = false
        }
    }

    /** "Search again": a new search of this one source (`fresh`); the kept rows stay until it answers. */
    fun searchAgain(s: PickerSource) {
        if (again[s.id] == true) return
        again[s.id] = true
        againErrors.remove(s.id)
        val q = query.trim()
        val gen = searchGen
        scope.launch {
            try {
                val r = soulseekTurn(s.id) { withContext(Dispatchers.IO) { features.pickerSearch(s.id, opts, q, fresh = true) } }
                // a new search started meanwhile: its answer wins
                if (gen == searchGen) {
                    land(s.id, r)
                    errors.remove(s.id)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (gen == searchGen) againErrors[s.id] = e.message ?: "Search failed"
            }
            again[s.id] = false
        }
    }

    fun download(source: String, payload: JsonElement, title: String) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                val status = withContext(Dispatchers.IO) { features.pick(source, payload, title, opts) }
                Toasts.show(if (status == "pending") "Sent for approval – your pick will be used" else "Downloading your pick", "ok")
                nav.back()
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
                busy = false
            }
        }
    }

    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("Pick a download · ${opts.title.ifEmpty { opts.album }}", scrollBehavior = scrollBehavior, onBack = nav.back) },
    ) { padding ->
        LoadableContent(info) { inf: PickerInfo ->
            if (!inf.allowed) {
                LaunchedEffect(Unit) { Toasts.show("Manual search is limited to admins", "err"); nav.back() }
                return@LoadableContent
            }
            // every source the server knows has a tab; only the ready ones search, the others show why not
            val sources = inf.sources
            LaunchedEffect(searchGen) {
                if (sources.isEmpty()) return@LaunchedEffect
                results.clear(); errors.clear(); kept.clear(); againErrors.clear()
                val q = query.trim()
                for (s in sources) {
                    // a ready source; and Soulseek while it is not connected, for the searches a server kept
                    if (!PickerRules.searches(s)) continue
                    if (s.ready) loading[s.id] = true
                    launch {
                        try {
                            land(s.id, soulseekTurn(s.id) { withContext(Dispatchers.IO) { features.pickerSearch(s.id, opts, q) } })
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            // a 409 carries the source's state and settings link ("Soulseek isn't connected yet");
                            // a source that was not ready with nothing kept shows its hint, as before
                            if (s.ready) errors[s.id] = pickErr(e)
                        }
                        loading[s.id] = false
                    }
                }
            }
            val tabs = listOf("all" to "All") + sources.map { it.id to it.name }
            val current = tabs.getOrNull(tab)?.first ?: "all"
            val showing = if (current == "all") sources.map { it.id } else listOf(current)
            fun tabLabel(id: String, name: String): String {
                if (id == "all") return "All"
                val s = sources.firstOrNull { it.id == id }
                if (loading[id] == true) return name
                return if (s == null || s.ready || results[id]?.isNotEmpty() == true) name + " " + (results[id]?.size ?: 0) else name + " · " + PickerRules.stateWord(s.state)
            }
            // a spinner in the tab of each source that searches, and in All while any does (the web's tabs)
            val busyTabs = tabs.indices.filter { i ->
                val id = tabs[i].first
                if (id == "all") loading.values.any { it } else loading[id] == true || again[id] == true
            }.toSet()

            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                item {
                    Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        EncoreField(
                            value = query,
                            onValueChange = { query = it },
                            placeholder = "Search, or paste a YouTube Music / Bandcamp / any link",
                            leadingIcon = R.drawable.ic_search,
                            imeAction = ImeAction.Search,
                            onAction = { if (isUrl(query)) download("link", buildJsonObject { put("url", query.trim()) }, query.trim()) else searchGen += 1 },
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(Modifier.width(8.dp))
                        EncoreButton("Search", primary = true, enabled = !busy) {
                            if (isUrl(query)) download("link", buildJsonObject { put("url", query.trim()) }, query.trim()) else searchGen += 1
                        }
                    }
                }
                if (isUrl(query)) {
                    item {
                        Row(
                            Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp).clip(RoundedCornerShape(12.dp)).border(1.dp, p.accent, RoundedCornerShape(12.dp)).padding(horizontal = 12.dp, vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f)) {
                                Text("Download this link", fontWeight = FontWeight.SemiBold)
                                Text(query.trim(), style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                            SmallButton("Get", iconRes = R.drawable.ic_download, primary = true, enabled = !busy) { download("link", buildJsonObject { put("url", query.trim()) }, query.trim()) }
                        }
                    }
                }
                item {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        ChoiceRow(tabs.map { (id, name) -> tabLabel(id, name) }, tab, onSelect = { tab = it }, modifier = Modifier.weight(1f), busy = busyTabs)
                    }
                }
                item {
                    Row(Modifier.padding(horizontal = Gutter), verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = lossless, onCheckedChange = { lossless = it })
                        Text("Lossless only", style = MaterialTheme.typography.bodyMedium)
                    }
                }
                if (sources.isEmpty()) {
                    item { Text("No download sources are enabled – set them up in Settings.", color = p.muted, modifier = Modifier.padding(Gutter)) }
                }
                for (src in showing) {
                    val source: PickerSource? = sources.firstOrNull { it.id == src }
                    val name = source?.name?.ifEmpty { null } ?: src
                    var list = results[src] ?: emptyList()
                    if (lossless) list = list.filter { it.lossless }
                    // the count only for a source that was searched (the web's section head does the same)
                    if (current == "all") item(key = "head-$src") { SectionHead(name, subtitle = if (source?.ready != false || kept[src] != null) "${list.size}" else null) }
                    val err = errors[src]
                    val keptAnswer = kept[src]
                    val admin = me?.isAdmin == true
                    when {
                        // signed out, not connected or off: "Sign in to Tidal – Settings → streamrip" (unless a kept
                        // search answered for it)
                        source != null && !source.ready && keptAnswer == null -> item(key = "state-$src") { HintLine(nav, admin, source.hint, source.settings, source.settingsLabel) }
                        err != null -> item(key = "err-$src") { HintLine(nav, admin, err.message, err.settings, err.label, error = true) }
                        loading[src] == true -> item(key = "load-$src") {
                            Row(Modifier.padding(horizontal = Gutter, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                                Spinner(); Spacer(Modifier.width(8.dp))
                                Text(
                                    if (src == "slskd" && slskWaiting) PickerRules.WAITING else PickerRules.searchingText(src, name),
                                    style = MaterialTheme.typography.bodySmall, color = p.muted,
                                )
                            }
                        }
                        else -> {
                            // a kept answer: "Found at 14:02 · Search again" (with "No results." when it found nothing)
                            val line = PickerRules.foundLine(keptAnswer, query, list.isEmpty(), System.currentTimeMillis() / 1000.0)
                            val running = if (again[src] == true) (if (src == "slskd" && slskWaiting) PickerRules.WAITING else PickerRules.searchingAgainText(name)) else null
                            if (line != null || running != null || againErrors[src] != null) {
                                item(key = "found-$src") {
                                    FoundRow(line?.text.orEmpty(), line?.again == true, running, againErrors[src]) { source?.let { searchAgain(it) } }
                                }
                            }
                            if (list.isEmpty() && line == null) item(key = "none-$src") { Text("No results.", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(horizontal = Gutter, vertical = 6.dp)) }
                            val shown = if (current == "all") list.take(8) else list.take(150)
                            items(shown.size, key = { i -> "$src-$i" }) { i -> PickRow(nav, shown[i], busy) { x -> download(x.source, x.payload, x.title) } }
                            if (current == "all" && list.size > shown.size) {
                                item(key = "more-$src") {
                                    Row(Modifier.padding(horizontal = Gutter, vertical = 6.dp)) {
                                        SmallButton("Show all ${list.size} from $name") { tab = tabs.indexOfFirst { it.first == src }.coerceAtLeast(0) }
                                    }
                                }
                            }
                        }
                    }
                }
                item {
                    Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        EncoreField(
                            value = linkUrl,
                            onValueChange = { linkUrl = it },
                            placeholder = "…or paste a link: Qobuz, Tidal, Deezer, YouTube, Bandcamp, SoundCloud…",
                            leadingIcon = R.drawable.ic_link,
                            keyboardType = KeyboardType.Uri,
                            imeAction = ImeAction.Go,
                            onAction = { if (linkUrl.isNotBlank()) download("link", buildJsonObject { put("url", linkUrl.trim()) }, linkUrl.trim()) },
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(Modifier.width(8.dp))
                        EncoreButton("Download link", iconRes = R.drawable.ic_download, enabled = !busy && linkUrl.isNotBlank()) {
                            download("link", buildJsonObject { put("url", linkUrl.trim()) }, linkUrl.trim())
                        }
                    }
                }
                item {
                    Text(
                        "The file you pick is still fingerprint-checked, tagged with MusicBrainz data and de-duplicated. If it fails, the request fails instead of trying other sources.",
                        style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(horizontal = Gutter),
                    )
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
}

/** picker.js `row`: title, subtitle, quality badge · files · duration · size · info, a warning, and "Get". */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun PickRow(nav: Nav, x: PickResult, busy: Boolean, onPick: (PickResult) -> Unit) {
    val p = MaterialTheme.palette
    Row(
        Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 5.dp).clip(RoundedCornerShape(12.dp)).background(p.panel).border(1.dp, p.border, RoundedCornerShape(12.dp)).padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (x.cover.isNotEmpty()) {
            Cover(x.cover, nav.graph.imageLoader, size = 44.dp, corner = 8.dp, label = x.title)
            Spacer(Modifier.width(10.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(x.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
            if (x.subtitle.isNotEmpty() && x.subtitle != x.title) Text(x.subtitle, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            FlowRow(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Badge(x.quality.ifEmpty { "?" }, kind = if (x.lossless) "ok" else "")
                val bits = ArrayList<String>()
                if (x.tracks > 0) bits.add("${x.tracks} file" + if (x.tracks == 1) "" else "s")
                if (x.durationSec > 0) bits.add(formatDuration((x.durationSec * 1000).toLong()))
                if (x.size > 0) bits.add(formatSize(x.size))
                if (x.info.isNotEmpty()) bits.add(x.info)
                for (b in bits) Text(b, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.align(Alignment.CenterVertically))
            }
            if (x.warning.isNotEmpty()) Text(x.warning, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn)
        }
        Spacer(Modifier.width(8.dp))
        SmallButton("Get", iconRes = R.drawable.ic_download, primary = true, enabled = !busy) { onPick(x) }
    }
}
