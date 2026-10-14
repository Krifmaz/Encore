package app.encore.android.ui

import android.content.Context
import android.content.ContextWrapper
import androidx.activity.ComponentActivity
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.PlaybackSpot
import app.encore.android.data.api.Track
import app.encore.android.data.session.Session
import app.encore.android.playback.ResumeStore
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.theme.palette
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private const val WEEK_SEC = 7 * 24 * 3600.0

private fun Context.findActivity(): ComponentActivity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is ComponentActivity) return c
        c = c.baseContext
    }
    return null
}

/**
 * Pick up on this phone where another device left off (web/js/remote.js `resume` / `showSpot`): the server keeps
 * the account's last spot; when the app opens with nothing playing and another device played after this phone did,
 * a bar above the mini player offers "Continue <song> from 2:31 · was playing on <device>" with Play here and ×.
 * Not shown when Settings → Playback "Resume where I left off" is off, when the spot is over a week old, or after
 * you closed the bar for this very spot.
 */
@Composable
fun SpotBar(nav: Nav, session: Session) {
    val context = LocalContext.current
    val graph = nav.graph
    val prefs by graph.prefs.playback.collectAsState()
    val isPlaying by nav.player.isPlaying.collectAsState()
    val store = remember(context) { ResumeStore(context) }
    val scope = rememberCoroutineScope()
    var spot by remember(session) { mutableStateOf<PlaybackSpot?>(null) }
    var busy by remember(session) { mutableStateOf(false) }
    // goes up each time the app comes back to the foreground: the app stays composed in the background,
    // so the spot is checked again then (you may have played on another device meanwhile)
    var foreground by remember { mutableIntStateOf(0) }
    val activity = remember(context) { context.findActivity() }
    DisposableEffect(activity) {
        val lifecycle = activity?.lifecycle
        var stopped = false
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) stopped = true
            if (event == Lifecycle.Event.ON_START && stopped) {
                stopped = false
                foreground++
            }
        }
        lifecycle?.addObserver(observer)
        onDispose { lifecycle?.removeObserver(observer) }
    }

    LaunchedEffect(session, prefs.resume, foreground) {
        spot = null
        if (!prefs.resume) return@LaunchedEffect
        val s = try { withContext(Dispatchers.IO) { graph.api.spot() } } catch (_: Exception) { null } ?: return@LaunchedEffect
        val localAt = withContext(Dispatchers.IO) { store.load()?.savedAtSec ?: 0.0 }
        val other = s.title.isNotEmpty() && s.deviceId != graph.store.deviceId &&
            System.currentTimeMillis() / 1000.0 - s.updatedAt < WEEK_SEC
        // another device played after this phone did, and you haven't closed the bar for this spot
        if (other && localAt < s.updatedAt && store.dismissedSpot != s.updatedAtRaw && !nav.player.isPlaying.value) spot = s
    }
    // something plays here now: the offer is gone
    LaunchedEffect(isPlaying) { if (isPlaying) spot = null }

    val s = spot ?: return
    val p = MaterialTheme.palette

    fun playHere() {
        if (busy) return
        busy = true
        scope.launch {
            try {
                var ids = s.queue.take(300)
                if (s.trackId !in ids) ids = listOf(s.trackId)
                val byId = withContext(Dispatchers.IO) { graph.api.tracksByIds(ids) }.associateBy { it.id }
                // back in the queue's own order, repeats included (the server sends each song once)
                val tracks = ArrayList<Track>(ids.size)
                var index = -1
                ids.forEachIndexed { i, id ->
                    val t = byId[id] ?: return@forEachIndexed
                    if (i == s.index && id == s.trackId) index = tracks.size
                    tracks.add(t)
                }
                if (index < 0) index = tracks.indexOfFirst { it.id == s.trackId }
                spot = null
                busy = false
                if (index < 0) {
                    Toasts.show("That song is no longer in your library", "err")
                    return@launch
                }
                var at = s.positionSec
                // still playing there: it has moved on since its last check-in, and it pauses so the music moves here
                if (s.online && s.playing && s.devicePlaying) {
                    at += maxOf(0.0, System.currentTimeMillis() / 1000.0 - s.updatedAt)
                    graph.scope.launch(Dispatchers.IO) { try { graph.api.remoteCommand(s.deviceId, "pause") } catch (_: Exception) {} }
                }
                val dur = tracks[index].durationSec
                if (dur > 0 && at > dur - 2) at = 0.0
                nav.player.playFrom(tracks, index, Math.round(at * 1000))
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                busy = false
                Toasts.show(e.message ?: "Couldn't load that queue", "err")
            }
        }
    }

    val shape = RoundedCornerShape(12.dp)
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .clip(shape)
            .background(p.bg2)
            .border(1.dp, p.border, shape)
            .padding(start = 14.dp, end = 2.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val at = formatDuration(Math.round(s.positionSec) * 1000L).ifEmpty { "0:00" }
        val text = buildAnnotatedString {
            append("Continue ")
            withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(s.title) }
            append(" from $at · was playing on ${s.deviceName.ifEmpty { "another device" }}")
        }
        Text(
            text,
            style = MaterialTheme.typography.bodySmall,
            color = p.text,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        Spacer(Modifier.width(8.dp))
        SmallButton("Play here", primary = true, enabled = !busy) { playHere() }
        IconButton(onClick = {
            store.dismissedSpot = s.updatedAtRaw
            spot = null
        }) {
            Icon(painterResource(R.drawable.ic_close), contentDescription = "Close", tint = p.muted, modifier = Modifier.size(20.dp))
        }
    }
}
