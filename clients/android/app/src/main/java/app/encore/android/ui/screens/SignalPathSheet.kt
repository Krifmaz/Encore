package app.encore.android.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.cadenza.CadenzaCatalog
import app.encore.android.core.int
import app.encore.android.core.stringOnly
import app.encore.android.data.api.Track
import app.encore.android.data.prefs.Quality
import app.encore.android.playback.StreamInfo
import app.encore.android.playback.output.DeviceOutput
import app.encore.android.ui.Nav
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** `GET /api/library/tracks/{id}/info`: what's in the file. */
class TrackInfo(val codec: String, val sampleRate: Int, val bitDepth: Int, val channels: Int, val bitrate: Int, val lossless: Boolean?) {
    companion object {
        fun from(o: JsonObject): TrackInfo {
            val l = (o["lossless"] as? JsonPrimitive)?.content
            return TrackInfo(o.stringOnly("codec"), o.int("sample_rate"), o.int("bit_depth"), o.int("channels"), o.int("bitrate"), when (l) { "true" -> true; "false" -> false; else -> null })
        }
    }
}

/** player.js sourceLine(): "FLAC · 16-bit / 44.1 kHz · stereo · 1411 kbps". */
fun sourceLine(t: Track, info: TrackInfo?): String {
    if (info == null) return t.format.uppercase() + (if (t.bitrate > 0) " · ${t.bitrate} kbps" else "")
    val parts = ArrayList<String>()
    parts.add((info.codec.ifEmpty { t.format }).uppercase())
    if (info.lossless == true && info.bitDepth > 0 && info.sampleRate > 0) parts.add("${info.bitDepth}-bit / ${Quality.khz(info.sampleRate)} kHz")
    else if (info.sampleRate > 0) parts.add("${Quality.khz(info.sampleRate)} kHz")
    if (info.channels > 0) parts.add(when (info.channels) { 2 -> "stereo"; 1 -> "mono"; else -> "${info.channels} channels" })
    if (info.bitrate > 0) parts.add("${info.bitrate} kbps")
    return parts.joinToString(" · ")
}

/**
 * The Signal path sheet (web/js/player.js signalPath()): what the file is, what Encore sent, what this device does
 * with it, the effects, and a plain verdict on whether what you hear is the file untouched.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SignalPathSheet(nav: Nav, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val track by nav.player.nowPlaying.collectAsState()
    val stream by nav.player.streamInfo.collectAsState()
    val prefs by nav.graph.prefs.playback.collectAsState()
    val phone by nav.graph.prefs.phone.collectAsState()
    val t = track
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)

    var info by remember { mutableStateOf<TrackInfo?>(null) }
    var loading by remember { mutableStateOf(true) }
    LaunchedEffect(t?.id) {
        val id = t?.id ?: return@LaunchedEffect
        loading = true
        info = try { withContext(Dispatchers.IO) { TrackInfo.from(nav.graph.api.trackInfo(id)) } } catch (e: CancellationException) { throw e } catch (_: Exception) { null }
        loading = false
    }
    val out = remember { DeviceOutput.of(context) }
    val catalog = CadenzaCatalog.phone

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 22.dp, end = 22.dp, bottom = 28.dp)) {
            Text("Signal path", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(bottom = 12.dp))
            if (t == null) {
                Text("Nothing playing.", color = p.muted)
            } else {
            val s: StreamInfo? = stream?.takeIf { it.trackId == t.id }
            val kind = s?.transportKind ?: "original"
            val transport = s?.transportText ?: "The original file, byte for byte – nothing was converted or resampled on the way"
            val net = if ((s?.connectionKbps ?: 0.0) > 0) " · connection ≈ ${Quality.mbit(s!!.connectionKbps)} Mbit/s" else ""

            PathRow("Source", if (loading && info == null) sourceLine(t, null) else sourceLine(t, info),
                if (info?.lossless == false) "A lossy file: what the file lost is gone before Encore touches it" else "")
            PathRow("Encore → this device", transport + net, "")

            val outRate = out.rateHz
            val src = info?.sampleRate ?: 0
            val cadenza = kind.startsWith("cadenza")
            val phoneCadenza = s?.phoneCadenza == true
            val phoneRate = s?.phoneRate ?: 0
            val bitPerfect = s?.usbBitPerfect == true
            if (phoneCadenza) {
                val filter = s?.phoneFilterLabel?.takeIf { it.isNotEmpty() }
                    ?: (if (phone.filter.isEmpty()) "Cadenza default filter" else catalog.filterLabel(phone.filter))
                val dither = s?.phoneDitherLabel?.takeIf { it.isNotEmpty() }
                    ?: (if (phone.dither.isEmpty()) "Cadenza default dither" else catalog.ditherLabel(phone.dither))
                PathRow(
                    "Cadenza on this phone",
                    if (phoneRate > 0) "$filter → ${Quality.khz(phoneRate)} kHz, $dither" else "$filter, $dither",
                    "",
                )
            } else if (phone.cadenza && cadenza) {
                PathRow("Cadenza on this phone", "Passed through", "The server upsampled this song, so the phone engine passed it through.")
            }
            // the rate that reaches Android: Cadenza's on the phone, else the song's own
            val inRate = if (phoneCadenza && phoneRate > 0) phoneRate else src
            val resampled = !bitPerfect && outRate > 0 && inRate > 0 && outRate != inRate
            val dacRate = if (phoneCadenza && phoneRate > 0) phoneRate else s?.processorRate?.takeIf { it > 0 } ?: src
            val dacName = s?.usbDevice?.takeIf { it.isNotEmpty() }?.let { " ($it)" } ?: ""
            PathRow(
                "This device",
                when {
                    bitPerfect && dacRate > 0 -> "USB DAC$dacName · bit-perfect at ${Quality.khz(dacRate)} kHz"
                    bitPerfect -> "USB DAC$dacName · bit-perfect"
                    outRate > 0 -> "Android audio → ${out.label} at ${Quality.khz(outRate)} kHz"
                    else -> "Android audio → ${out.label}"
                },
                when {
                    bitPerfect -> "Android's mixer is bypassed: the DAC gets these samples exactly, and its own volume sets the level."
                    !s?.usbNote.isNullOrEmpty() -> s?.usbNote ?: ""
                    resampled -> "Your device resamples ${Quality.khz(inRate)} → ${Quality.khz(outRate)} kHz. Android mixes every app's sound at one rate; for true bit-perfect output use a USB DAC in bit-perfect mode (Settings → Playback → Cadenza engine) or a Cadenza device."
                    outRate > 0 && inRate > 0 -> "Same rate as ${if (phoneCadenza) "Cadenza's output" else "the file"} – no resampling on this device. Whether Android mixes it with other sounds is up to your audio settings."
                    else -> ""
                },
            )
            val effects = prefs.effectsSummary()
            if (effects.isNotEmpty()) PathRow("Effects on this device", effects, s?.effectsEngine?.takeIf { it.isNotEmpty() } ?: "")

            val effected = prefs.fxActive()
            val untouched = kind == "original" && info?.lossless != false
            val pure = untouched && !effected
            Spacer(Modifier.height(8.dp))
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(p.panel2).padding(12.dp)) {
                Text(
                    when {
                        phoneCadenza -> "Played by Cadenza on this phone"
                        cadenza -> "Processed by Cadenza on the server"
                        pure -> "Pure playback from Encore"
                        else -> "Not the untouched file"
                    },
                    fontWeight = FontWeight.Bold,
                )
                Text(
                    when {
                        phoneCadenza -> "Cadenza upsampled the song in this app before Android got it. Turn it off under Settings → Playback → Cadenza engine to get the untouched file."
                        cadenza -> "Encore upsampled the file on the server with Cadenza and sent lossless 24-bit FLAC. Turn it off under Settings → Playback → Server processing to get the untouched file."
                        pure -> "Encore sent the file exactly as it is stored. Everything after that – volume, mixing, resampling – is this device and Android."
                        untouched -> "Encore sent the file untouched; this device adds the effects you chose under Settings → Playback → Sound."
                        else -> "Encore converted this stream (see above). Settings → Playback → Quality → Original keeps the file untouched even when the connection is slow."
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    modifier = Modifier.padding(top = 4.dp),
                )
                if (phoneCadenza) {
                    Text(
                        if (bitPerfect) "Bit-perfect to the DAC" else if (outRate > 0) "Mixed by Android at ${Quality.khz(outRate)} kHz" else "Mixed by Android",
                        style = MaterialTheme.typography.bodySmall,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }
            }
            }
        }
    }
}

@Composable
private fun PathRow(label: String, value: String, note: String) {
    val p = MaterialTheme.palette
    Column(Modifier.fillMaxWidth().padding(bottom = 12.dp)) {
        Text(label, style = MaterialTheme.typography.bodySmall, color = p.muted)
        Text(value, fontWeight = FontWeight.SemiBold)
        if (note.isNotEmpty()) Text(note, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(top = 2.dp))
    }
}
