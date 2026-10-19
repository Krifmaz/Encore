package app.encore.android.remote

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.dbl
import app.encore.android.core.str
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** Whether the Devices sheet is open; any devices button sets it, the root (EncoreRoot) draws the sheet. */
object RemoteUi {
    val devicesOpen = MutableStateFlow(false)
    fun openDevices() { devicesOpen.value = true }
}

/** The devices / cast-style button of the mini and full player (the web's player-bar devices button). */
@Composable
fun DevicesButton(nav: Nav, tint: Color = Color.Unspecified, modifier: Modifier = Modifier) {
    val link by nav.player.remote.link.collectAsState()
    val p = MaterialTheme.palette
    IconButton(onClick = { RemoteUi.openDevices() }, modifier = modifier) {
        Icon(
            painterResource(R.drawable.ic_devices),
            contentDescription = if (link != null) "Devices (controlling ${link?.name})" else "Devices",
            tint = if (link != null) p.accent else if (tint == Color.Unspecified) p.text else tint,
        )
    }
}

/**
 * While this phone is a remote: "Controlling <name>" above the mini player (instead of the pick-up bar), with the
 * Devices sheet and Unlink. "offline" once the device missed two check-ins.
 */
@Composable
fun LinkedBar(nav: Nav) {
    val remote = nav.player.remote
    val link by remote.link.collectAsState()
    val l = link ?: return
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(12.dp)
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .clip(shape)
            .background(p.bg2)
            .border(1.dp, p.border, shape)
            .padding(start = 12.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(R.drawable.ic_link), contentDescription = null, tint = p.accent, modifier = Modifier.size(18.dp))
        Spacer(Modifier.width(8.dp))
        val text = buildAnnotatedString {
            append("Controlling ")
            withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(l.name) }
            if (!l.online) append(" · offline")
        }
        Text(text, style = MaterialTheme.typography.bodySmall, color = p.text, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        Spacer(Modifier.width(8.dp))
        SmallButton("Devices", iconRes = R.drawable.ic_devices) { RemoteUi.openDevices() }
        Spacer(Modifier.width(6.dp))
        SmallButton("Unlink") { remote.unlink() }
    }
}

/** The linked device's volume in the full player (nothing when this phone plays itself). */
@Composable
fun LinkedVolume(nav: Nav, modifier: Modifier = Modifier) {
    val link by nav.player.remote.link.collectAsState()
    val l = link ?: return
    val p = MaterialTheme.palette
    VolumeSlider(l.volume.toFloat(), p.accent, p.border, modifier) { v -> nav.player.setVolume(v) }
}

@Composable
private fun VolumeSlider(value: Float, active: Color, inactive: Color, modifier: Modifier = Modifier, onCommit: (Float) -> Unit) {
    var dragging by remember { mutableStateOf<Float?>(null) }
    Slider(
        value = dragging ?: value,
        onValueChange = { dragging = it },
        onValueChangeFinished = {
            dragging?.let(onCommit)
            dragging = null
        },
        valueRange = 0f..1f,
        colors = SliderDefaults.colors(thumbColor = active, activeTrackColor = active, inactiveTrackColor = inactive),
        modifier = modifier,
    )
}

/**
 * "Your devices" (web/js/remote.js `open()`): this phone's name, the link, and every other signed-in device with
 * what it plays and its controls – Play here (its queue moves to this phone and it pauses), Send my music there
 * (this phone's queue moves to it and this phone pauses) and Link as remote.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DevicesSheet(nav: Nav, onDismiss: () -> Unit) {
    val remote = nav.player.remote
    val devices by remote.devices.collectAsState()
    val link by remote.link.collectAsState()
    val custom by remote.customName.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val p = MaterialTheme.palette
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var renaming by remember { mutableStateOf(false) }

    // pushed updates repaint the list; this is the fallback while the event stream is down
    LaunchedEffect(Unit) {
        remote.refresh()
        while (true) {
            delay(2000L)
            if (!remote.streaming.value) remote.refresh()
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp)) {
            Text("Your devices", style = MaterialTheme.typography.titleMedium)
            Spacer(Modifier.height(8.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                val me = buildAnnotatedString {
                    append("This device: ")
                    withStyle(SpanStyle(fontWeight = FontWeight.Bold, color = p.text)) { append(custom.ifEmpty { RemoteDevice.defaultName() }) }
                }
                Text(me, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.weight(1f))
                TextButton(onClick = { renaming = true }) { Text("Rename", color = p.accent) }
            }
            val l = link
            if (l != null) {
                val text = buildAnnotatedString {
                    append("Linked: everything you play here goes to ")
                    withStyle(SpanStyle(fontWeight = FontWeight.Bold, color = p.text)) { append(l.name) }
                    append(". Unlink to listen on this device.")
                }
                Text(text, style = MaterialTheme.typography.bodySmall, color = p.muted)
                Spacer(Modifier.height(6.dp))
                SmallButton("Unlink from ${l.name}", iconRes = R.drawable.ic_link) { remote.unlink() }
            }
            Spacer(Modifier.height(12.dp))
        }
        LazyColumn(Modifier.fillMaxWidth().fillMaxHeight(0.75f)) {
            if (devices.isEmpty()) {
                item {
                    Text(
                        "No other devices right now. Open Encore on your phone, computer or the apps (signed in as you) and they show up here.",
                        style = MaterialTheme.typography.bodySmall,
                        color = p.muted,
                        modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
                    )
                }
            }
            items(devices, key = { it.id }) { dev ->
                DeviceCard(
                    nav = nav,
                    dev = dev,
                    others = devices,
                    linked = link?.id == dev.id,
                    canSend = link == null && nowPlaying != null,
                    onDone = onDismiss,
                )
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }

    if (renaming) {
        var text by remember { mutableStateOf(remote.name()) }
        AlertDialog(
            onDismissRequest = { renaming = false },
            containerColor = p.panel,
            titleContentColor = p.text,
            textContentColor = p.muted,
            title = { Text("Name this device") },
            text = {
                Column {
                    Text("Shown on your other devices", style = MaterialTheme.typography.bodySmall)
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(value = text, onValueChange = { text = it.take(60) }, singleLine = true)
                }
            },
            confirmButton = {
                TextButton(onClick = { remote.setName(text); renaming = false }) { Text("Save", color = p.text, fontWeight = FontWeight.SemiBold) }
            },
            dismissButton = { TextButton(onClick = { renaming = false }) { Text("Cancel", color = p.muted) } },
        )
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun DeviceCard(nav: Nav, dev: DeviceInfo, others: List<DeviceInfo>, linked: Boolean, canSend: Boolean, onDone: () -> Unit) {
    val remote = nav.player.remote
    val p = MaterialTheme.palette
    val s = dev.state
    val shape = RoundedCornerShape(12.dp)
    fun send(action: String, build: kotlinx.serialization.json.JsonObjectBuilder.() -> Unit = {}) {
        val body: JsonObject = buildJsonObject { put("action", action); build() }
        if (linked) remote.sendToLink(body) else remote.send(dev.id, body)
    }
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 14.dp, vertical = 5.dp)
            .clip(shape)
            .background(p.bg2)
            .border(1.dp, if (linked) p.accent else p.border, shape)
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(painterResource(R.drawable.ic_devices), contentDescription = null, tint = p.muted, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(6.dp))
            Text(dev.name, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            if (dev.remoteOf.isEmpty()) {
                if (linked) SmallButton("Linked · Unlink", iconRes = R.drawable.ic_link, primary = true) { remote.unlink() }
                else SmallButton("Link as remote", iconRes = R.drawable.ic_link) { remote.linkTo(dev) }
            }
        }
        val title = dev.title
        when {
            dev.remoteOf.isNotEmpty() -> {
                val of = if (dev.remoteOf == remote.me) "this device" else (others.firstOrNull { it.id == dev.remoteOf }?.name ?: "another device")
                Text("Remote for $of", style = MaterialTheme.typography.bodySmall, color = p.muted)
            }
            title.isNotEmpty() -> Row(verticalAlignment = Alignment.CenterVertically) {
                val albumId = s.str("album_id")
                Cover(
                    if (albumId.isNotEmpty()) nav.graph.api.coverUrl(albumId, 96) else null,
                    nav.graph.imageLoader,
                    size = 52.dp,
                    corner = 8.dp,
                    label = s.str("album").ifEmpty { title },
                )
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        listOf(s.str("artist"), s.str("album")).filter { it.isNotEmpty() }.joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                        color = p.muted,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    val pos = formatDuration(Math.round(s.dbl("position") * 1000)).ifEmpty { "0:00" }
                    val dur = formatDuration(Math.round(s.dbl("duration") * 1000)).ifEmpty { "0:00" }
                    val label = (s["signal"] as? JsonObject)?.str("label") ?: ""
                    Text(
                        (if (dev.playing) "Playing" else "Paused") + " · $pos / $dur" + (if (label.isNotEmpty()) " · $label" else ""),
                        style = MaterialTheme.typography.bodySmall,
                        color = p.faint,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            else -> Text("Nothing playing", style = MaterialTheme.typography.bodySmall, color = p.muted)
        }
        if (dev.remoteOf.isEmpty()) {
            if (title.isNotEmpty()) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = { send("prev") }) {
                        Icon(painterResource(R.drawable.ic_skip_previous), contentDescription = "Previous", tint = p.text)
                    }
                    IconButton(onClick = { send("toggle") }) {
                        Icon(
                            painterResource(if (dev.playing) R.drawable.ic_pause else R.drawable.ic_play),
                            contentDescription = if (dev.playing) "Pause" else "Play",
                            tint = p.text,
                        )
                    }
                    IconButton(onClick = { send("next") }) {
                        Icon(painterResource(R.drawable.ic_skip_next), contentDescription = "Next", tint = p.text)
                    }
                    val vol = if (s.containsKey("volume")) s.dbl("volume").toFloat().coerceIn(0f, 1f) else 0.9f
                    VolumeSlider(vol, p.accent, p.border, Modifier.weight(1f).padding(start = 6.dp)) { v -> send("volume") { put("value", v.toDouble()) } }
                }
            }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (title.isNotEmpty() && !linked) {
                    SmallButton("Play here") { playHere(nav, dev); onDone() }
                }
                if (canSend) {
                    SmallButton("Send my music there", primary = true) { sendThere(nav, dev) }
                }
            }
        }
    }
}

/** Play here: the other device's queue, at its song and position, on this phone – and that device pauses. */
private fun playHere(nav: Nav, dev: DeviceInfo) {
    val remote = nav.player.remote
    val s = dev.state
    val queue = RemoteDevice.tracksOf(s["queue"])
    val current = RemoteDevice.trackOf(s)
    val list = if (queue.isNotEmpty()) queue else listOfNotNull(current?.takeIf { it.id > 0L })
    if (list.isEmpty()) {
        Toasts.show("That song is no longer in your library", "err")
        return
    }
    var index = s.dbl("index").toInt()
    if (current != null && (index !in list.indices || list[index].id != current.id)) index = list.indexOfFirst { it.id == current.id }
    if (index < 0) index = 0
    // this phone plays itself again
    if (remote.link.value != null) remote.unlink()
    var at = s.dbl("position")
    val dur = list[index].durationSec
    if (dur > 0 && at > dur - 2) at = 0.0
    nav.player.playFrom(list, index, Math.round(at * 1000))
    remote.send(dev.id, buildJsonObject { put("action", "pause") })
}

/** Send my music there: this phone's queue, at its song and position, plays on that device – and this phone pauses. */
private fun sendThere(nav: Nav, dev: DeviceInfo) {
    val full = nav.player.queue.value
    val queue = full.filter { it.id > 0L }
    val index = nav.player.queueIndex.value
    if (queue.isEmpty() || index < 0) {
        Toasts.show("Only songs from your library can be sent", "err")
        return
    }
    // the place of the song in the library-only list (songs not in the library before it are left out)
    val idx = full.take(index).count { it.id > 0L }.coerceIn(0, queue.size - 1)
    val (win, at) = RemoteDevice.window(queue, idx)
    nav.player.remote.send(dev.id, buildJsonObject {
        put("action", "play_queue")
        put("tracks", RemoteDevice.slimList(win))
        put("index", at)
        put("position", nav.player.positionMs() / 1000.0)
    })
    if (nav.player.isPlaying.value) nav.player.togglePlay()
}
