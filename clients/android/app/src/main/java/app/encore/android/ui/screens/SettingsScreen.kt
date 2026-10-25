package app.encore.android.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import android.os.Build
import app.encore.android.BuildConfig
import app.encore.android.R
import app.encore.android.data.prefs.EqBand
import app.encore.android.data.prefs.PhonePrefs
import app.encore.android.data.prefs.NormPrefs
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.playback.Normalization
import app.encore.android.data.prefs.Quality
import app.encore.android.core.long
import app.encore.android.core.str
import app.encore.android.data.api.settingsApi
import app.encore.android.data.session.Session
import app.encore.android.features.files.FilesAccess
import app.encore.android.features.settings.AppearanceScreen
import app.encore.android.features.settings.AutoDownloadScreen
import app.encore.android.features.settings.BlockedScreen
import app.encore.android.features.settings.ConnectionsScreen
import app.encore.android.features.settings.ContributeScreen
import app.encore.android.features.settings.DiagnosticsScreen
import app.encore.android.features.settings.EqGraph
import app.encore.android.features.settings.InvitesScreen
import app.encore.android.features.settings.LanguageScreen
import app.encore.android.features.settings.PluginRules
import app.encore.android.features.settings.PluginsScreen
import app.encore.android.features.settings.SECTION_TITLES
import app.encore.android.features.settings.SchemaFormState
import app.encore.android.features.settings.ServerSchema
import app.encore.android.features.settings.ServerSectionScreen
import app.encore.android.features.settings.SettingsRules
import app.encore.android.features.settings.ThisDeviceScreen
import app.encore.android.features.settings.UsersScreen
import app.encore.android.features.settings.appearanceSummary
import app.encore.android.features.settings.loadState
import app.encore.android.features.settings.schemaApi
import app.encore.android.ui.components.Badge
import app.encore.android.cadenza.CadenzaCatalog
import app.encore.android.cadenza.CadenzaEngine
import app.encore.android.playback.CarPrefs
import app.encore.android.playback.output.DeviceOutput
import app.encore.android.playback.output.UsbOutputManager
import app.encore.android.ui.Nav
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import app.encore.android.ui.tv.TvLayoutMode
import app.encore.android.ui.tv.TvPrefs
import app.encore.android.ui.tv.rememberIsTvDevice
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Settings: the web's sections in the web's order (web-rs/src/settings/mod.rs), as a list that opens one page at a
 * time. You – Account, Appearance, Language, Playback, Connections and This device (the phone's own); for admins
 * also People – Users & permissions, Invites; Server – the pages of the server's settings schema (General, Library,
 * Downloads, Integrations, Server processing, Email & notifications, Auto-download, Contribute, Blocked, Scheduled
 * tasks) with Plugins after Downloads (0.32: the schema has no page for it, the web adds it there too) and Files;
 * System – Diagnostics. The schema and its values are asked for once when Settings opens, so what was typed on a Server page
 * and not saved yet is still there after going to another page and back.
 */
@Composable
fun SettingsScreen(nav: Nav, session: Session, startAt: String? = null) {
    val graph = nav.graph
    val api = graph.settingsApi
    val p = MaterialTheme.palette
    val settings by graph.store.settings.collectAsState()
    val prefs by graph.prefs.playback.collectAsState()
    // the pages opened from the list (and from one page to another: Users → Invites), the top one shown
    val pages = remember { mutableStateListOf<String>() }
    var focus by remember { mutableStateOf<String?>(null) }
    var me by remember { mutableStateOf<JsonObject?>(null) }
    var info by remember { mutableStateOf<JsonObject?>(null) }
    var schemaVersion by remember { mutableIntStateOf(0) }
    var schemaState by remember { mutableStateOf<SchemaFormState?>(null) }
    var schemaError by remember { mutableStateOf<String?>(null) }
    var update by remember { mutableStateOf<JsonObject?>(null) }
    val filesHere by FilesAccess.available.collectAsState()
    // Settings → TV is listed on a TV only
    val isTv = rememberIsTvDevice()
    // whether this server has plugins (0.32): null while not known (taken as yes); `probed` once it was asked
    var plugins by remember { mutableStateOf<Boolean?>(null) }
    var probed by remember { mutableStateOf(false) }

    LaunchedEffect(session) {
        try {
            val (m, i) = withContext(Dispatchers.IO) { api.me() to (try { api.info() } catch (_: Exception) { JsonObject(emptyMap()) }) }
            me = m
            info = i
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            me = JsonObject(emptyMap())
        }
    }
    val role = me?.str("role").orEmpty()
    val admin = role == "admin" || role == "owner"
    val features = info?.get("features")?.let { el -> (el as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content } } ?: emptyList()
    // Show and Copy of saved secrets: the server's route is the owner's (and needs its "reveal" feature)
    val canReveal = "reveal" in features && role == "owner"
    LaunchedEffect(admin, canReveal, schemaVersion) {
        if (!admin) return@LaunchedEffect
        schemaError = null
        try {
            schemaState = withContext(Dispatchers.IO) { graph.schemaApi.loadState(canReveal) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            schemaError = e.message ?: "Something went wrong"
        }
    }
    LaunchedEffect(admin) {
        if (!admin) return@LaunchedEffect
        plugins = withContext(Dispatchers.IO) { api.pluginsProbe() }
        probed = true
    }
    LaunchedEffect(admin) {
        if (!admin) return@LaunchedEffect
        FilesAccess.probe(graph)
        // the banner: the server's kept check for a newer Encore (GitHub is asked only when that check is old)
        update = try { withContext(Dispatchers.IO) { api.serverUpdate(false) } } catch (e: CancellationException) { throw e } catch (_: Exception) { null }
    }
    val schema = schemaState?.schema
    // a key saved outside the schema cards (Invites' sign-up mode, a tool switch, a task's cleanup) shows on its card too
    val mainScope = rememberCoroutineScope()
    DisposableEffect(schemaState) {
        val st = schemaState
        api.onSettingsSaved = if (st == null) null else { v -> mainScope.launch { st.replaceValues(v) } }
        onDispose { api.onSettingsSaved = null }
    }

    fun open(id: String) {
        // the source cards are plugin cards since 0.32: their old links (and the Download tools card's, the client,
        // Translator and connection plugins') open the Plugins page with that plugin's card open
        if (plugins != false && PluginRules.opensPlugins(id)) {
            focus = id.takeIf { it != "plugins" }
            pages.add("plugins")
            return
        }
        // a card's id (old links name cards) opens its page with that card open
        val page = schema?.pageOf(id) ?: id
        focus = if (page != id) id else null
        pages.add(page)
    }
    // opened at one page (a `#/settings?s=…` link): it opens once the schema has said which page holds that card, and
    // going back from it leaves Settings
    var started by remember { mutableStateOf(startAt == null) }
    LaunchedEffect(startAt, me, schemaState, schemaError, probed) {
        if (started || startAt == null || me == null) return@LaunchedEffect
        if (admin && ((schemaState == null && schemaError == null) || !probed)) return@LaunchedEffect
        started = true
        open(startAt)
    }
    val back: () -> Unit = {
        if (startAt != null && pages.size <= 1) nav.back()
        else if (pages.isNotEmpty()) pages.removeAt(pages.lastIndex)
        focus = null
    }
    if (!started) {
        Column(Modifier.fillMaxSize()) {
            BackBar("Settings", nav.back)
            LoadingBox()
        }
        return
    }

    val page = pages.lastOrNull()
    if (page != null) {
        BackHandler(onBack = back)
        val server = ServerSchema(schemaState, schemaError) { schemaVersion += 1 }
        // one page's state never carries over to the next (Users → Invites, Auto-download → Downloads)
        key(page, pages.size) { when (page) {
            "account" -> AccountScreen(nav, session, onBack = back)
            "appearance" -> AppearanceScreen(nav, serverPrefs = "theme" in features, onBack = back)
            "language" -> LanguageScreen(nav, onBack = back)
            "playback" -> PlaybackSettingsScreen(nav, onBack = back)
            "androidauto" -> CarButtonsScreen(onBack = back, title = "Android Auto")
            "connections" -> ConnectionsScreen(nav, admin, onBack = back)
            "device" -> ThisDeviceScreen(nav, onBack = back)
            // this TV's own layout (shown on TVs only)
            "tv" -> app.encore.android.ui.tv.TvSettingsScreen(onBack = back)
            "users" -> UsersScreen(nav, me?.long("id") ?: 0L, onBack = back, onOpenInvites = { open("invites") })
            "invites" -> InvitesScreen(nav, onBack = back)
            "autodownload" -> AutoDownloadScreen(nav, onBack = back, onOpen = { open(it) })
            "contribute" -> ContributeScreen(nav, onBack = back)
            "blocked" -> BlockedScreen(nav, admin, onBack = back)
            "diagnostics" -> DiagnosticsScreen(nav, onBack = back)
            "plugins" -> PluginsScreen(server, graph.schemaApi, api, focus, onBack = back, onOpen = { open(it) })
            else -> ServerSectionScreen(page, server, graph.schemaApi, api, info, focus, plugins, onBack = back, onOpen = { open(it) })
        } }
        return
    }

    Column(Modifier.fillMaxSize()) {
        BackBar("Settings", nav.back)
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            // admins: the server's last check found a newer Encore ("Encore 0.31.0 is available – Diagnostics")
            val u = update
            if (admin && u != null && SettingsRules.truthy(u["is_newer"]) && u.str("latest").isNotEmpty()) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 16.dp, vertical = 6.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(p.panel)
                        .border(1.dp, p.border, RoundedCornerShape(12.dp))
                        .clickable { open("diagnostics") }
                        .padding(horizontal = 14.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Badge("Update", kind = "info")
                    Spacer(Modifier.width(8.dp))
                    Text("Encore ${u.str("latest")} is available – Diagnostics", style = MaterialTheme.typography.bodySmall)
                }
            }
            NavLabel("You")
            // which server this phone is signed in to, at which of its addresses (local network or public) right now
            // (This device has the whole address and how)
            val route by app.encore.android.data.api.ServerRoute.route.collectAsState()
            NavRow("Account", "${session.username} · ${app.encore.android.data.api.RouteRules.summary(route, session.server)}") { open("account") }
            NavRow("Appearance", appearanceSummary(settings)) { open("appearance") }
            NavRow("Language") { open("language") }
            NavRow("Playback", playbackSummary(prefs)) { open("playback") }
            // the car's buttons (the media player's row and the Encore Lyrics screen's strip): one tap from Settings
            NavRow("Android Auto", "Car buttons and the Encore Lyrics screen") { open("androidauto") }
            NavRow("Connections") { open("connections") }
            NavRow("This device") { open("device") }
            // on a TV: the TV layout, the player's side and the zoom (ui/tv)
            if (isTv) {
                val tvMode by TvPrefs.mode.collectAsState()
                val tvSide by TvPrefs.side.collectAsState()
                NavRow("TV", if (tvMode == TvLayoutMode.OFF) tvMode.label else "Player on the ${tvSide.label.lowercase()}") { open("tv") }
            }
            if (admin) {
                NavLabel("People")
                NavRow("Users & permissions") { open("users") }
                NavRow("Invites") { open("invites") }
                NavLabel("Server")
                val sections = schema?.sections?.map { it.id to it.title } ?: SERVER_PAGES.map { it to (SECTION_TITLES[it] ?: it) }
                for ((id, title) in withPlugins(sections, plugins != false)) NavRow(title) { open(id) }
                // the admin file manager (features/files, 0.33; not on older servers)
                if (filesHere == true) NavRow("Files") { nav.open(app.encore.android.ui.Screen.Files()) }
                NavLabel("System")
                NavRow("Diagnostics", badge = if (u != null && SettingsRules.truthy(u["is_newer"]) && u.str("latest").isNotEmpty()) "Update" else null) { open("diagnostics") }
            }
            Text(
                "Encore for Android ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})",
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
                modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 24.dp),
            )
            Spacer(Modifier.height(32.dp))
        }
    }
}

/**
 * The Server pages with Plugins right after Downloads (the web's `server_nav`): the server's schema has no page for
 * it. A server whose schema has one keeps its own; one without plugins (before 0.32) has none.
 */
private fun withPlugins(sections: List<Pair<String, String>>, plugins: Boolean): List<Pair<String, String>> {
    if (!plugins || sections.any { it.first == "plugins" }) return sections
    val at = sections.indexOfFirst { it.first == "downloads" }
    val entry = "plugins" to (SECTION_TITLES["plugins"] ?: "Plugins")
    return if (at < 0) sections + entry else sections.subList(0, at + 1) + entry + sections.subList(at + 1, sections.size)
}

/** The Server pages when the schema has not loaded (an older server, or an error): the web's ids and order. */
private val SERVER_PAGES = listOf(
    "general", "library", "downloads", "integrations", "processing", "accounts", "autodownload", "contribute", "blocked", "tasks",
)

/** A group label of the list (the web's `.nav-label`). */
@Composable
private fun NavLabel(text: String) {
    Text(
        text.uppercase(),
        style = MaterialTheme.typography.labelSmall,
        fontWeight = FontWeight.SemiBold,
        color = MaterialTheme.palette.muted,
        modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 20.dp, bottom = 4.dp),
    )
}

/** One page of the list: its title, a line of what it holds, and the arrow. */
@Composable
private fun NavRow(title: String, summary: String = "", badge: String? = null, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            if (summary.isNotEmpty()) Text(summary, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1)
        }
        if (badge != null) {
            Badge(badge, kind = "info")
            Spacer(Modifier.width(8.dp))
        }
        Icon(painterResource(R.drawable.ic_expand_more), contentDescription = null, tint = p.muted, modifier = Modifier.rotate(-90f))
    }
}

/** "Auto · pre-load · then shuffle library · EQ: Bass boost" – the one-line state of the Playback screen. */
private fun playbackSummary(p: PlaybackPrefs): String {
    val parts = ArrayList<String>()
    parts.add(when (p.quality) { "original" -> "Original"; "saver" -> "Data saver"; else -> "Auto" })
    if (p.preload) parts.add("pre-load")
    if (p.autoplay) parts.add("then shuffle library")
    if (p.dsp) parts.add("server processing")
    val fx = p.effectsSummary(withCrossfade = false)
    if (fx.isNotEmpty()) parts.add(fx)
    return parts.joinToString(" · ")
}

// ---- Settings → Playback (web/js/pages/settings.js renderPlayback, the same sections and words) -------------------

private val QUALITY_HELP = mapOf(
    "auto" to "The original file. Encore measures your connection while it plays and steps the stream down as far as it needs to keep the music playing – down to 32 kbps – then back up as soon as the connection allows.",
    "original" to "Always the original file, even if it stutters on a slow connection. Formats this device can't play are converted to FLAC (lossless).",
    "saver" to "Always a small 96 kbps stream (AAC) – for mobile data and slow links.",
)

private val DSP_RATES = listOf(88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000)

/** Settings → Playback: quality, buffering, what happens when the queue ends, sound effects, server-side Cadenza. */
@Composable
fun PlaybackSettingsScreen(nav: Nav, onBack: () -> Unit) {
    val repo = nav.graph.prefs
    val prefs by repo.playback.collectAsState()
    val dsp by repo.dsp.collectAsState()
    val phone by repo.phone.collectAsState()
    val context = LocalContext.current
    val p = MaterialTheme.palette
    // In the car → Car buttons: its own page over this one (CarButtonsScreen.kt). The scroll position and the DSP
    // refresh sit above the early return, so Back lands where the page was left and does not fetch again.
    val scroll = rememberScrollState()
    LaunchedEffect(Unit) { repo.refreshDsp() }
    var carButtons by androidx.compose.runtime.saveable.rememberSaveable { mutableStateOf(false) }
    if (carButtons) {
        CarButtonsScreen(onBack = { carButtons = false })
        return
    }
    BackHandler(onBack = onBack)
    val set: ((PlaybackPrefs) -> PlaybackPrefs) -> Unit = { change -> repo.update(change) }
    val deviceRate = remember { DeviceOutput.of(context).rateHz }
    // a USB DAC shows up here while nothing plays too (the playback service only watches while it runs; no-op below Android 14)
    DisposableEffect(Unit) {
        val watch = UsbOutputManager.watch(context)
        onDispose { watch.close() }
    }
    val usbDac by UsbOutputManager.state.collectAsState()
    val bitPerfect = phone.bitPerfect && usbDac != null

    Column(Modifier.fillMaxSize()) {
        BackBar("Playback", onBack)
        Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(horizontal = 16.dp)) {
            Help("Saved to your account, so it follows you to every device.")

            H3("Quality")
            Seg(
                listOf("auto" to "Auto", "original" to "Original", "saver" to "Data saver"),
                prefs.quality,
            ) { v -> set { it.copy(quality = v) } }
            Help(QUALITY_HELP[prefs.quality] ?: "")

            H3("Buffering")
            CheckRow("Pre-load the next song while this one plays", prefs.preload) { on -> set { it.copy(preload = on) } }
            Help("The next song is fetched once the current one is safely buffered, so it starts without a gap. Turn it off on a very slow connection to leave all the bandwidth to the song playing. Takes effect the next time playback starts.")

            H3("When the queue ends")
            CheckRow("Keep going with a shuffle of the whole library", prefs.autoplay) { on -> set { it.copy(autoplay = on) } }
            CheckRow("Resume where I left off when the app opens", prefs.resume) { on -> set { it.copy(resume = on) } }

            H3("In the car")
            val carLyrics by remember { CarPrefs.liveLyrics(context) }.collectAsState()
            CheckRow("Live lyrics in the car", carLyrics) { on -> CarPrefs.setLiveLyrics(context, on) }
            val carLyricsMode by remember { CarPrefs.lyricsMode(context) }.collectAsState()
            Seg(
                listOf(CarPrefs.MODE_WIDE to "Wide", CarPrefs.MODE_CARD to "Card", CarPrefs.MODE_TITLE to "Title line"),
                carLyricsMode,
            ) { v -> CarPrefs.setLyricsMode(context, v) }
            Help(
                if (carLyricsMode == CarPrefs.MODE_WIDE) {
                    "Android Auto shows the lyrics across its player: the line playing as large as the title and the next line under it, beside a lyrics card in place of the cover. Android Auto draws the player itself and keeps a gap between the two; Encore Lyrics in the car's app list fills the whole screen. The Lyrics button (Car buttons, below) turns it on and off. Only on this phone."
                } else if (carLyricsMode == CarPrefs.MODE_TITLE) {
                    "Android Auto shows the current line of a song's synced lyrics in place of its title. The Lyrics button (Car buttons, below) turns it on and off. Only on this phone."
                } else {
                    "Android Auto shows a lyrics card in place of the cover: the current line of a song's synced lyrics, large, with the lines around it, over the blurred cover. Title and artist stay as they are. The Lyrics button (Car buttons, below) turns it on and off. Only on this phone."
                },
            )
            Help("For the whole song text on the car screen, open Encore Lyrics in the car's app list.")
            CarButtonsRow { carButtons = true }

            H3("Sound (on this device)")
            val norm by repo.norm.collectAsState()
            NormalizationSection(norm, bitPerfect) { n -> repo.setNorm(n) }
            SoundSection(prefs, bitPerfect, set)

            H3("Song transitions")
            TransitionsSection(prefs, bitPerfect, set)

            H3("Pure playback")
            Help("Encore streams the original file untouched – the quality badge on the player opens the Signal path, which shows what the file is, what was sent and what this device does with it. The app hands the sound to Android, which mixes it with other sounds and may resample it; for bit-perfect output use a USB DAC in bit-perfect mode (Cadenza engine below).")

            H3("Server processing (Cadenza)")
            val d = dsp
            if (d == null || !d.enabled) {
                GreyedDsp()
                Help("Turned off on this server. Cadenza upsampling for streams uses the server's processor, so an admin has to switch it on (Settings → Server processing) and allow your account.")
            } else if (!d.allowed) {
                GreyedDsp()
                Help("Your account isn't allowed to use the server's processing. Ask an admin.")
            } else {
                CheckRow("Upsample my streams on the server", prefs.dsp) { on -> set { it.copy(dsp = on) } }
                val filters = listOf("" to "Server default") + d.filters
                Column(Modifier.alpha(if (prefs.dsp) 1f else 0.45f)) {
                    Picker("Upsampling filter", filters, prefs.dspFilter, enabled = prefs.dsp) { v -> set { it.copy(dspFilter = v) } }
                    Picker("Dither / noise shaping", d.dithers, prefs.dspDither, enabled = prefs.dsp) { v -> set { it.copy(dspDither = v) } }
                    val rates = listOf(
                        "device" to (if (deviceRate > 0) "Match this device (${Quality.khz(deviceRate)} kHz)" else "Match this device"),
                        "auto" to "Highest the server allows",
                    ) + DSP_RATES.filter { it <= d.maxRate }.map { it.toString() to "${Quality.khz(it)} kHz" }
                    Picker("Output rate", rates, prefs.dspRate, enabled = prefs.dsp) { v -> set { it.copy(dspRate = v) } }
                }
                Help("The server's Cadenza upsamples the original file and sends lossless 24-bit FLAC. It uses more data (about 4.6 Mbit/s at 96 kHz, 9.2 Mbit/s at 192 kHz) and the server's processor; Data saver and Auto still step down to a lighter stream when the connection can't keep up.")
            }

            H3("Cadenza engine · this phone")
            PhoneCadenzaSection(phone, deviceRate, serverOn = d != null && d.enabled && d.allowed && prefs.dsp) { change -> repo.setPhone(change(repo.phone.value)) }
            Spacer(Modifier.height(32.dp))
        }
    }
}

/**
 * Settings → Playback → "Cadenza engine · this phone": the engine inside the app ([CadenzaEngine]) upsampling on the
 * phone, and bit-perfect output to a USB DAC on Android 14+ ([UsbOutputManager]). Device-only, so [PhonePrefs]
 * (not the account's playback prefs).
 */
@Composable
private fun PhoneCadenzaSection(phone: PhonePrefs, deviceRate: Int, serverOn: Boolean, set: ((PhonePrefs) -> PhonePrefs) -> Unit) {
    if (!CadenzaEngine.available) {
        Column(Modifier.alpha(0.45f)) {
            CheckRow("Upsample on this phone with Cadenza", false, enabled = false) {}
            Picker("Upsampling filter", listOf("" to "–"), "", enabled = false) {}
            Picker("Dither / noise shaping", listOf("" to "–"), "", enabled = false) {}
            Picker("Output rate", listOf("" to "–"), "", enabled = false) {}
        }
        Help("This build of the app came without the Cadenza library.")
        return
    }
    val catalog = CadenzaCatalog.phone
    CheckRow("Upsample on this phone with Cadenza", phone.cadenza) { on -> set { it.copy(cadenza = on) } }
    val on = phone.cadenza
    Column(Modifier.alpha(if (on) 1f else 0.45f)) {
        Picker("Upsampling filter", listOf("" to "Cadenza default") + catalog.filters, phone.filter, enabled = on) { v -> set { it.copy(filter = v) } }
        Picker("Dither / noise shaping", listOf("" to "Cadenza default") + catalog.dithers, phone.dither, enabled = on) { v -> set { it.copy(dither = v) } }
        val rates = listOf("device" to (if (deviceRate > 0) "Match this device (${Quality.khz(deviceRate)} kHz)" else "Match this device")) +
            PhonePrefs.RATES.map { it.toString() to "${Quality.khz(it)} kHz" }
        Picker("Output rate", rates, phone.rate, enabled = on) { v -> set { it.copy(rate = v) } }
    }
    Help("Cadenza runs inside the app: the song is upsampled on the phone before Android gets it. Matching this device's rate stops Android resampling it again; higher rates only make sense with a USB DAC in bit-perfect mode below. Longer filters use more battery.")
    if (serverOn) Help("Server processing is on, so songs the server upsamples skip the phone engine.")

    if (Build.VERSION.SDK_INT >= 34) {
        val usb by UsbOutputManager.state.collectAsState()
        val dac = usb
        if (dac != null) {
            val name = dac.deviceName.ifEmpty { "USB DAC" }
            CheckRow("Send directly to USB DAC (bit-perfect) · $name", phone.bitPerfect) { b -> set { it.copy(bitPerfect = b) } }
            Help("Android's mixer is bypassed and the DAC gets the exact samples (the song's, or Cadenza's when upsampling is on). Song transitions are off in this mode; volume is the DAC's own.")
        } else {
            Help("Plug in a USB DAC that supports bit-perfect output and the switch appears here.")
        }
    } else {
        Help("Bit-perfect USB output needs Android 14 or newer.")
    }
}

@Composable
private fun SoundSection(prefs: PlaybackPrefs, bitPerfect: Boolean, set: ((PlaybackPrefs) -> PlaybackPrefs) -> Unit) {
    val p = MaterialTheme.palette
    // the effects run in Cadenza: a build without its library plays without them
    val lib = CadenzaEngine.available
    CheckRow("Use sound effects", prefs.fxOn && lib, enabled = lib) { on -> set { it.copy(fxOn = on) } }
    val on = prefs.fxOn && lib
    Column(Modifier.alpha(if (on) 1f else 0.45f)) {
        val presets = PlaybackPrefs.EQ_PRESETS.map { it.id to it.label } + (if (prefs.eqPreset == "custom") listOf("custom" to "Custom") else emptyList())
        Picker("Equalizer preset", presets, prefs.eqPreset, enabled = on) { id ->
            val x = PlaybackPrefs.EQ_PRESETS.firstOrNull { it.id == id }
            if (x != null) set { it.copy(eqPreset = x.id, eqBands = x.bands, eqPreamp = x.preamp) }
        }
        SliderRow("Preamp", signDb(prefs.eqPreamp), prefs.eqPreamp.toFloat(), -24f..12f, steps = 71, enabled = on) { v ->
            set { it.copy(eqPreamp = Math.round(v / 0.5f) * 0.5) }
        }
        // the bands on screen are the preset's own until you edit one (then the preset is Custom)
        val bands = prefs.bands()
        // Cadenza's graph: drag a point (frequency left/right, gain up/down), double-tap for a bell
        EqGraph(bands, enabled = on, maxBands = 10) { nb -> set { it.copy(eqPreset = "custom", eqBands = nb) } }
        Help("Drag a point to move it (left/right = frequency, up/down = gain), double-tap the graph to add a bell there.")
        Row(Modifier.fillMaxWidth().padding(top = 4.dp)) {
            Text("Type", Modifier.weight(1.3f), style = MaterialTheme.typography.bodySmall, color = p.muted)
            Text("Frequency (Hz)", Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = p.muted)
            Text("Gain (dB)", Modifier.weight(0.8f), style = MaterialTheme.typography.bodySmall, color = p.muted)
            Text("Q", Modifier.weight(0.7f), style = MaterialTheme.typography.bodySmall, color = p.muted)
            Spacer(Modifier.width(40.dp))
        }
        if (bands.isEmpty()) Text("No bands.", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(vertical = 6.dp))
        bands.forEachIndexed { i, b ->
            BandRow(
                b,
                enabled = on,
                onChange = { nb -> set { it.copy(eqPreset = "custom", eqBands = bands.toMutableList().also { l -> l[i] = nb }) } },
                onRemove = { set { it.copy(eqPreset = "custom", eqBands = bands.filterIndexed { k, _ -> k != i }) } },
            )
        }
        Row {
            TextButton(
                onClick = { set { it.copy(eqPreset = "custom", eqBands = bands + EqBand("peaking", 1000, 0.0, 1.0)) } },
                enabled = on && bands.size < 10,
            ) { Text("Add band") }
        }
        Picker("Crossfeed", PlaybackPrefs.CROSSFEED, prefs.crossfeed, enabled = on) { v -> set { it.copy(crossfeed = v) } }
        Help("For headphones: mixes a little of each channel into the other, like speakers in a room.")
        SliderRow("Balance", PlaybackPrefs.balanceText(prefs.balance), prefs.balance.toFloat(), -100f..100f, steps = 199, enabled = on) { v ->
            set { it.copy(balance = Math.round(v)) }
        }
        CheckRow("Mono", prefs.mono, enabled = on) { m -> set { it.copy(mono = m) } }
    }
    if (!lib) Help("This build of the app came without the Cadenza library: the sound effects are off.")
    else Help("Runs in this app – it costs your server nothing. Works with every quality setting. Turning it on means the sound is no longer the untouched file (the Signal path says so).")
}

private val NORM_HELP = mapOf(
    "off" to "Every song plays at its own level.",
    "track" to "Every song plays at about the same loudness.",
    "album" to "Every album plays at about the same loudness; the songs of an album keep their differences.",
)

/**
 * Volume normalization (the web's Settings → Playback → Sound, keys `encore.norm.*`): Off / Track / Album, the target
 * loudness and "Prevent clipping". This device only (PrefsRepository.norm); it works with the effects off.
 */
@Composable
private fun NormalizationSection(norm: NormPrefs, bitPerfect: Boolean, set: (NormPrefs) -> Unit) {
    Column(Modifier.alpha(if (bitPerfect) 0.45f else 1f)) {
        Text("Volume normalization", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted, modifier = Modifier.padding(bottom = 4.dp))
        Seg(NormPrefs.MODES, norm.mode) { v -> set(norm.copy(mode = v)) }
        Help(NORM_HELP[norm.mode] ?: "")
        Column(Modifier.alpha(if (norm.on) 1f else 0.45f)) {
            SliderRow(
                "Target level",
                Normalization.lufsText(norm.target),
                norm.target.toFloat(),
                NormPrefs.MIN_TARGET.toFloat()..NormPrefs.MAX_TARGET.toFloat(),
                steps = NormPrefs.MAX_TARGET - NormPrefs.MIN_TARGET - 1,
                enabled = norm.on,
            ) { v -> set(norm.copy(target = Math.round(v))) }
            CheckRow("Prevent clipping", norm.clip, enabled = norm.on) { on -> set(norm.copy(clip = on)) }
        }
        Help("Uses the ReplayGain tags in your files, or Encore's own measurement when a file has none. Higher targets are louder; −14 LUFS is about what streaming services play at. Prevent clipping never turns a song up further than its loudest peak allows. Only on this device.")
    }
    if (bitPerfect) Help("Normalization is off while this phone sends songs bit-perfect to the USB DAC (Cadenza engine below).")
}

private val TRANSITION_HELP = mapOf(
    "off" to "Each song plays to its end, then the next one starts.",
    "crossfade" to "The end of one song fades out while the next one fades in.",
    "overlap" to "The next song starts at full volume a few seconds early while the old one fades out under it.",
    "smart" to "Skips the silence at the end of one song and the start of the next, then overlaps them from the last audible moment, with a short fade.",
)

/** Song transitions: how one song becomes the next (every mode uses the length and the album rule). */
@Composable
private fun TransitionsSection(prefs: PlaybackPrefs, bitPerfect: Boolean, set: ((PlaybackPrefs) -> PlaybackPrefs) -> Unit) {
    // bit-perfect USB output plays one song at a time straight to the DAC, so there is nothing to blend
    val off = prefs.transition == "off"
    val secs = (if (prefs.crossfade > 0) prefs.crossfade else PlaybackPrefs.TRANSITION_DEFAULT_S).coerceIn(1, 12)
    Column(Modifier.alpha(if (bitPerfect) 0.45f else 1f)) {
        Picker(null, PlaybackPrefs.TRANSITIONS, prefs.transition, enabled = !bitPerfect) { v ->
            set { it.copy(transition = v, crossfade = if (v != "off" && it.crossfade < 1) PlaybackPrefs.TRANSITION_DEFAULT_S else it.crossfade) }
        }
        Help(TRANSITION_HELP[prefs.transition] ?: "")
        Column(Modifier.alpha(if (off) 0.45f else 1f)) {
            SliderRow("Length", "$secs s", secs.toFloat(), 1f..12f, steps = 10, enabled = !bitPerfect && !off) { v ->
                set { it.copy(crossfade = Math.round(v).coerceIn(1, 12)) }
            }
        }
        CheckRow("Keep albums gapless (no transition between songs from the same album)", prefs.crossfadeAlbumGapless, enabled = !bitPerfect) { on -> set { it.copy(crossfadeAlbumGapless = on) } }
    }
    if (bitPerfect) Help("Song transitions are off while this phone sends songs bit-perfect to the USB DAC (Cadenza engine below).")
}

private fun signDb(v: Double): String = "${if (v > 0) "+" else ""}${PlaybackPrefs.fmt(v)} dB"

@Composable
private fun BandRow(b: EqBand, enabled: Boolean, onChange: (EqBand) -> Unit, onRemove: () -> Unit) {
    val p = MaterialTheme.palette
    Row(Modifier.fillMaxWidth().padding(vertical = 2.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        Box(Modifier.weight(1.3f)) {
            Picker(null, PlaybackPrefs.EQ_TYPES, b.type, enabled = enabled) { t -> onChange(b.copy(type = t)) }
        }
        NumberField(b.freq.toString(), Modifier.weight(1f), enabled) { s -> s.toIntOrNull()?.let { onChange(b.copy(freq = it.coerceIn(20, 20000))) } }
        NumberField(PlaybackPrefs.fmt(b.gain), Modifier.weight(0.8f), enabled) { s -> s.toDoubleOrNull()?.let { onChange(b.copy(gain = it.coerceIn(-24.0, 24.0))) } }
        NumberField(PlaybackPrefs.fmt(b.q), Modifier.weight(0.7f), enabled) { s -> s.toDoubleOrNull()?.let { onChange(b.copy(q = it.coerceIn(0.1, 18.0))) } }
        IconButton(onClick = onRemove, enabled = enabled, modifier = Modifier.width(40.dp)) {
            Icon(painterResource(R.drawable.ic_close), contentDescription = "Remove band", tint = p.muted)
        }
    }
}

/** A number box that commits what you typed when it parses (keeps the text while you edit). */
@Composable
private fun NumberField(value: String, modifier: Modifier, enabled: Boolean, onCommit: (String) -> Unit) {
    var text by remember(value) { mutableStateOf(value) }
    OutlinedTextField(
        value = text,
        onValueChange = { s -> text = s; if (s.isNotBlank()) onCommit(s) },
        enabled = enabled,
        singleLine = true,
        textStyle = MaterialTheme.typography.bodySmall,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        modifier = modifier,
    )
}

@Composable
private fun H3(text: String) {
    Text(text, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 20.dp, bottom = 6.dp))
}

@Composable
private fun Help(text: String) {
    if (text.isEmpty()) return
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted, modifier = Modifier.padding(top = 4.dp, bottom = 4.dp))
}

@Composable
private fun CheckRow(label: String, checked: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(enabled = enabled) { onChange(!checked) }.padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Checkbox(checked = checked, onCheckedChange = { onChange(it) }, enabled = enabled)
        Text(label, Modifier.weight(1f))
    }
}

@Composable
private fun SliderRow(label: String, valueText: String, value: Float, range: ClosedFloatingPointRange<Float>, steps: Int, enabled: Boolean, onCommit: (Float) -> Unit) {
    var dragging by remember { mutableStateOf<Float?>(null) }
    val shown = dragging ?: value
    Column(Modifier.fillMaxWidth().padding(top = 6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(label, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted)
            Spacer(Modifier.width(6.dp))
            Text(if (dragging != null) previewText(label, shown) else valueText, style = MaterialTheme.typography.bodySmall, fontWeight = FontWeight.SemiBold)
        }
        Slider(
            value = shown,
            onValueChange = { dragging = it },
            onValueChangeFinished = { dragging?.let { onCommit(it) }; dragging = null },
            valueRange = range,
            steps = steps,
            enabled = enabled,
        )
    }
}

private fun previewText(label: String, v: Float): String = when (label) {
    "Preamp" -> signDb(Math.round(v / 0.5f) * 0.5)
    "Balance" -> PlaybackPrefs.balanceText(Math.round(v))
    "Length" -> "${Math.round(v)} s"
    "Target level" -> Normalization.lufsText(Math.round(v))
    else -> v.toString()
}

/** A labelled drop-down (the web's `<select>`). */
@Composable
private fun Picker(label: String?, options: List<Pair<String, String>>, selected: String, enabled: Boolean, onSelect: (String) -> Unit) {
    val p = MaterialTheme.palette
    var open by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
        if (label != null) Text(label, style = MaterialTheme.typography.bodySmall, color = p.muted)
        Box {
            Row(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(8.dp))
                    .background(p.panel2)
                    .clickable(enabled = enabled) { open = true }
                    .padding(horizontal = 12.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(options.firstOrNull { it.first == selected }?.second ?: selected.ifEmpty { "–" }, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                Icon(painterResource(R.drawable.ic_expand_more), contentDescription = null, tint = p.muted)
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                for ((v, l) in options) {
                    DropdownMenuItem(text = { Text(l) }, onClick = { open = false; onSelect(v) })
                }
            }
        }
    }
}

/** The web's `.seg`: a row of choices, one on. */
@Composable
private fun Seg(options: List<Pair<String, String>>, selected: String, onSelect: (String) -> Unit) {
    val p = MaterialTheme.palette
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(p.panel2).padding(3.dp), horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        for ((v, l) in options) {
            val on = v == selected
            Box(
                Modifier
                    .weight(1f)
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (on) p.accent else p.panel2)
                    .clickable { onSelect(v) }
                    .padding(vertical = 8.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(l, style = MaterialTheme.typography.labelLarge, color = if (on) p.onAccent else p.text)
            }
        }
    }
}

@Composable
private fun GreyedDsp() {
    Column(Modifier.alpha(0.45f)) {
        CheckRow("Upsample my streams on the server", false, enabled = false) {}
        Picker("Upsampling filter", listOf("" to "–"), "", enabled = false) {}
        Picker("Dither / noise shaping", listOf("" to "–"), "", enabled = false) {}
        Picker("Output rate", listOf("" to "–"), "", enabled = false) {}
    }
}
