package app.encore.android.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
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
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
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
import app.encore.android.data.prefs.PlaybackPrefs
import app.encore.android.data.prefs.Quality
import app.encore.android.data.session.AuthKind
import app.encore.android.data.session.Session
import app.encore.android.data.session.ThemeChoice
import app.encore.android.cadenza.CadenzaEngine
import app.encore.android.playback.CarPrefs
import app.encore.android.playback.DeviceOutput
import app.encore.android.playback.ResumeStore
import app.encore.android.playback.UsbOutputManager
import app.encore.android.remote.RemoteDevice
import app.encore.android.remote.RemoteUi
import app.encore.android.ui.Nav
import app.encore.android.ui.components.SectionTitle
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

@Composable
fun SettingsScreen(nav: Nav, session: Session) {
    val store = nav.graph.store
    val settings by store.settings.collectAsState()
    val prefs by nav.graph.prefs.playback.collectAsState()
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val p = MaterialTheme.palette
    var showPlayback by remember { mutableStateOf(false) }

    if (showPlayback) {
        PlaybackSettingsScreen(nav, onBack = { showPlayback = false })
        return
    }

    Column(Modifier.fillMaxSize()) {
        BackBar("Settings", nav.back)
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            SectionTitle("Account")
            Column(Modifier.padding(horizontal = 16.dp)) {
                Text(session.username, style = MaterialTheme.typography.bodyLarge)
                Text(session.server, style = MaterialTheme.typography.bodySmall, color = p.muted)
                Text(
                    if (session.kind == AuthKind.BEARER) "Signed in with an app token" else "Signed in with a web session (expires after 30 days)",
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                )
                Spacer(Modifier.height(12.dp))
                OutlinedButton(onClick = {
                    nav.player.stopAndClear()
                    ResumeStore(context).clear()
                    scope.launch {
                        withContext(Dispatchers.IO) { nav.graph.api.logout() }
                        nav.graph.likes.clear()
                        nav.graph.prefs.clear()
                        store.signOut()
                    }
                }) { Text("Sign out") }
            }

            SectionTitle("Appearance")
            ThemeOption("Encore (charcoal and cream)", settings.theme == ThemeChoice.ENCORE) { store.setTheme(ThemeChoice.ENCORE) }
            ThemeOption("OLED black", settings.theme == ThemeChoice.OLED) { store.setTheme(ThemeChoice.OLED) }
            ThemeOption("Light", settings.theme == ThemeChoice.LIGHT) { store.setTheme(ThemeChoice.LIGHT) }

            SectionTitle("Playback")
            Row(
                Modifier.fillMaxWidth().clickable { showPlayback = true }.padding(horizontal = 16.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text("Playback")
                    Text(
                        playbackSummary(prefs),
                        style = MaterialTheme.typography.bodySmall,
                        color = p.muted,
                    )
                }
                Icon(painterResource(R.drawable.ic_expand_more), contentDescription = null, tint = p.muted)
            }

            SectionTitle("This device")
            DeviceNameRow(nav)

            SectionTitle("About")
            Text(
                "Encore for Android ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})",
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
                modifier = Modifier.padding(horizontal = 16.dp),
            )
            Spacer(Modifier.height(32.dp))
        }
    }
}

/**
 * The name your other devices see this phone under (web/js/remote.js `encore.device.name`; empty = the default
 * "Android app (<model>)"), and the Devices sheet.
 */
@Composable
private fun DeviceNameRow(nav: Nav) {
    val remote = nav.player.remote
    val custom by remote.customName.collectAsState()
    val p = MaterialTheme.palette
    var text by remember { mutableStateOf(custom) }
    Column(Modifier.padding(horizontal = 16.dp)) {
        OutlinedTextField(
            value = text,
            onValueChange = { v -> text = v.take(60); remote.setName(text) },
            label = { Text("Device name") },
            placeholder = { Text(RemoteDevice.defaultName()) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Text(
            "Shown on your other devices, and in \"was playing on\" when another device offers to continue your music",
            style = MaterialTheme.typography.bodySmall,
            color = p.muted,
            modifier = Modifier.padding(top = 4.dp),
        )
    }
    Row(
        Modifier.fillMaxWidth().clickable { RemoteUi.openDevices() }.padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text("Your devices")
            Text("Play here, send your music to another device, or use this phone as a remote", style = MaterialTheme.typography.bodySmall, color = p.muted)
        }
        Icon(painterResource(R.drawable.ic_devices), contentDescription = null, tint = p.muted)
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

@Composable
private fun ThemeOption(label: String, selected: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = selected, onClick = onClick)
        Text(label)
    }
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
    BackHandler(onBack = onBack)
    LaunchedEffect(Unit) { repo.refreshDsp() }
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
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
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
            Help("Android Auto shows the current line of a song's synced lyrics in place of its title (the Lyrics button next to Repeat turns it on and off). Only on this phone.")

            H3("Sound (on this device)")
            SoundSection(prefs, bitPerfect, set)

            H3("Song transitions")
            TransitionsSection(prefs, bitPerfect, set)

            H3("Pure playback")
            Help("Encore streams the original file untouched – the quality badge on the player opens the Signal path, which shows what the file is, what was sent and what this device does with it. The app hands the sound to Android, which mixes it with other sounds and may resample it; for bit-perfect output use a USB DAC in bit-perfect mode (Cadenza engine below).")

            H3("Server processing (Cadenza)")
            val d = dsp
            if (d == null || !d.enabled) {
                GreyedDsp()
                Help("Turned off on this server. Cadenza upsampling for streams uses the server's processor, so an admin has to switch it on (Settings → Apps & sources → Server processing) and allow your account.")
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
    val catalog = remember { CadenzaCatalog.load() }
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

/** [CadenzaEngine.catalog] parsed: the filters and dithers the phone engine offers, as (id, label) pairs. */
internal class CadenzaCatalog(val filters: List<Pair<String, String>>, val dithers: List<Pair<String, String>>) {
    fun filterLabel(id: String): String = filters.firstOrNull { it.first == id }?.second ?: id
    fun ditherLabel(id: String): String = dithers.firstOrNull { it.first == id }?.second ?: id

    companion object {
        val EMPTY = CadenzaCatalog(emptyList(), emptyList())

        fun load(): CadenzaCatalog {
            val json = try { CadenzaEngine.catalog() } catch (_: Throwable) { "" }
            if (json.isEmpty()) return EMPTY
            return try {
                val o = Json.parseToJsonElement(json) as? JsonObject ?: return EMPTY
                fun pairs(k: String): List<Pair<String, String>> = (o[k] as? JsonArray)?.mapNotNull { e ->
                    val a = e as? JsonArray ?: return@mapNotNull null
                    val id = (a.getOrNull(0) as? JsonPrimitive)?.content ?: return@mapNotNull null
                    val label = (a.getOrNull(1) as? JsonPrimitive)?.content ?: id
                    id to label
                } ?: emptyList()
                CadenzaCatalog(pairs("filters"), pairs("dithers"))
            } catch (_: Exception) {
                EMPTY
            }
        }
    }
}

@Composable
private fun SoundSection(prefs: PlaybackPrefs, bitPerfect: Boolean, set: ((PlaybackPrefs) -> PlaybackPrefs) -> Unit) {
    val p = MaterialTheme.palette
    CheckRow("Use sound effects", prefs.fxOn) { on -> set { it.copy(fxOn = on) } }
    val on = prefs.fxOn
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
    Help("Runs in this app – it costs your server nothing. Works with every quality setting. Turning it on means the sound is no longer the untouched file (the Signal path says so).")
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
