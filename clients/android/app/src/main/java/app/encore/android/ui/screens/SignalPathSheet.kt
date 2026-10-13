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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.data.api.Track
import app.encore.android.data.prefs.Quality
import app.encore.android.playback.DeviceOutput
import app.encore.android.playback.StreamInfo
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
            fun s(k: String) = (o[k] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: ""
            fun n(k: String) = (o[k] as? JsonPrimitive)?.content?.toDoubleOrNull()?.toInt() ?: 0
            val l = (o["lossless"] as? JsonPrimitive)?.content
            return TrackInfo(s("codec"), n("sample_rate"), n("bit_depth"), n("channels"), n("bitrate"), when (l) { "true" -> true; "false" -> false; else -> null })
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
            val resampled = outRate > 0 && src > 0 && outRate != src
            PathRow(
                "This device",
                if (outRate > 0) "Android audio → ${out.label} at ${Quality.khz(outRate)} kHz" else "Android audio → ${out.label}",
                when {
                    resampled -> "Your device resamples ${Quality.khz(src)} → ${Quality.khz(outRate)} kHz. Android mixes every app's sound at one rate; for true bit-perfect output play through a Cadenza device or an exclusive-mode player."
                    outRate > 0 && src > 0 -> "Same rate as the file – no resampling on this device. Whether Android mixes it with other sounds is up to your audio settings."
                    else -> ""
                },
            )
            val effects = prefs.effectsSummary()
            if (effects.isNotEmpty()) PathRow("Effects on this device", effects, s?.effectsEngine?.takeIf { it.isNotEmpty() } ?: "")

            val effected = prefs.fxActive()
            val untouched = kind == "original" && info?.lossless != false
            val pure = untouched && !effected
            val cadenza = kind.startsWith("cadenza")
            Spacer(Modifier.height(8.dp))
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(p.panel2).padding(12.dp)) {
                Text(
                    when {
                        cadenza -> "Processed by Cadenza on the server"
                        pure -> "Pure playback from Encore"
                        else -> "Not the untouched file"
                    },
                    fontWeight = FontWeight.Bold,
                )
                Text(
                    when {
                        cadenza -> "Encore upsampled the file on the server with Cadenza and sent lossless 24-bit FLAC. Turn it off under Settings → Playback → Server processing to get the untouched file."
                        pure -> "Encore sent the file exactly as it is stored. Everything after that – volume, mixing, resampling – is this device and Android."
                        untouched -> "Encore sent the file untouched; this device adds the effects you chose under Settings → Playback → Sound."
                        else -> "Encore converted this stream (see above). Settings → Playback → Quality → Original keeps the file untouched even when the connection is slow."
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    modifier = Modifier.padding(top = 4.dp),
                )
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
