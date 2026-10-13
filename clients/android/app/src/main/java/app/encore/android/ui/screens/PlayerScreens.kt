package app.encore.android.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.ui.Nav
import app.encore.android.ui.PlayerConnection
import app.encore.android.ui.PlayerUiState
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.LikeButton
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.theme.EncoreColors
import kotlinx.coroutines.delay

/** The playback position, polled from the controller twice a second while playing. */
@Composable
private fun rememberPosition(player: PlayerConnection, state: PlayerUiState): Long {
    var position by remember { mutableStateOf(0L) }
    LaunchedEffect(state.isPlaying, state.current?.id, state.index) {
        while (true) {
            position = player.positionMs()
            if (!state.isPlaying) break
            delay(500L)
        }
    }
    return position
}

/** The bar above the tabs: cover, title, play/pause, next, and a thin progress line. */
@Composable
fun MiniPlayer(nav: Nav, state: PlayerUiState, onOpen: () -> Unit) {
    val t = state.current ?: return
    val position = rememberPosition(nav.player, state)
    val fraction = if (state.durationMs > 0) (position.toFloat() / state.durationMs).coerceIn(0f, 1f) else 0f
    Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainer)) {
        Box(Modifier.fillMaxWidth().height(2.dp).background(MaterialTheme.colorScheme.surfaceContainerHighest)) {
            Box(Modifier.fillMaxWidth(fraction).fillMaxHeight().background(EncoreColors.AccentGradient))
        }
        Row(
            Modifier.fillMaxWidth().clickable(onClick = onOpen).padding(start = 12.dp, end = 4.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Cover(if (t.albumId.isNotEmpty()) nav.graph.api.coverUrl(t.albumId, 96) else null, nav.graph.imageLoader, size = 44.dp, corner = 6.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(t.title, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(t.artist, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            IconButton(onClick = { nav.player.togglePlay() }) {
                if (state.buffering && !state.isPlaying) {
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                } else {
                    Icon(painterResource(if (state.isPlaying) R.drawable.ic_pause else R.drawable.ic_play), contentDescription = if (state.isPlaying) "Pause" else "Play")
                }
            }
            IconButton(onClick = { nav.player.next() }, enabled = state.hasNext) {
                Icon(painterResource(R.drawable.ic_skip_next), contentDescription = "Next")
            }
        }
    }
}

/** The full-screen player: big cover, scrubber, transport, like, and the queue. */
@Composable
fun FullPlayer(nav: Nav, state: PlayerUiState, onClose: () -> Unit) {
    val t = state.current ?: return
    val liked by nav.graph.likes.liked.collectAsState()
    var showQueue by remember { mutableStateOf(false) }
    val position = rememberPosition(nav.player, state)
    var dragging by remember { mutableStateOf<Float?>(null) }

    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            // swallow taps so they don't reach the screen underneath
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = {})
            .safeDrawingPadding(),
    ) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onClose) { Icon(painterResource(R.drawable.ic_expand_more), contentDescription = "Close") }
            Text(
                if (showQueue) "Up next" else "Now playing",
                style = MaterialTheme.typography.titleSmall,
                textAlign = TextAlign.Center,
                modifier = Modifier.weight(1f),
            )
            IconButton(onClick = { showQueue = !showQueue }) {
                Icon(
                    painterResource(R.drawable.ic_queue_music),
                    contentDescription = "Queue",
                    tint = if (showQueue) MaterialTheme.colorScheme.secondary else MaterialTheme.colorScheme.onSurface,
                )
            }
        }

        if (showQueue) {
            QueueList(nav, state, Modifier.weight(1f))
        } else {
            Box(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 28.dp, vertical = 12.dp), contentAlignment = Alignment.Center) {
                Cover(
                    if (t.albumId.isNotEmpty()) nav.graph.api.coverUrl(t.albumId, 1200) else null,
                    nav.graph.imageLoader,
                    Modifier.fillMaxWidth().aspectRatio(1f),
                    corner = 14.dp,
                )
            }
        }

        Row(Modifier.fillMaxWidth().padding(start = 24.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(t.title, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    t.artist,
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.secondary,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.clickable {
                        if (t.albumArtist.isNotEmpty()) { onClose(); nav.open(Screen.Artist(t.albumArtist)) }
                    },
                )
                Text(
                    listOf(t.album, t.format.uppercase()).filter { it.isNotEmpty() }.joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.clickable {
                        if (t.albumId.isNotEmpty()) { onClose(); nav.open(Screen.Album(t.albumId)) }
                    },
                )
            }
            LikeButton(t.id in liked, { nav.graph.likes.toggle(t.id) })
        }

        val duration = state.durationMs.coerceAtLeast(0L)
        val shown = dragging ?: if (duration > 0) (position.toFloat() / duration).coerceIn(0f, 1f) else 0f
        Slider(
            value = shown,
            onValueChange = { dragging = it },
            onValueChangeFinished = {
                val d = dragging
                if (d != null && duration > 0) nav.player.seekTo((d * duration).toLong())
                dragging = null
            },
            enabled = duration > 0,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
        )
        Row(Modifier.fillMaxWidth().padding(horizontal = 24.dp)) {
            Text(formatDuration(((dragging ?: -1f).let { if (it >= 0f) (it * duration).toLong() else position })).ifEmpty { "0:00" },
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.weight(1f))
            Text(formatDuration(duration), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }

        Row(
            Modifier.fillMaxWidth().padding(vertical = 16.dp),
            horizontalArrangement = Arrangement.SpaceEvenly,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = { nav.player.previous() }, modifier = Modifier.size(56.dp)) {
                Icon(painterResource(R.drawable.ic_skip_previous), contentDescription = "Previous", modifier = Modifier.size(32.dp))
            }
            Box(
                Modifier
                    .size(72.dp)
                    .clip(CircleShape)
                    .background(EncoreColors.AccentGradient)
                    .clickable { nav.player.togglePlay() },
                contentAlignment = Alignment.Center,
            ) {
                if (state.buffering && !state.isPlaying) {
                    CircularProgressIndicator(Modifier.size(28.dp), color = EncoreColors.Background, strokeWidth = 3.dp)
                } else {
                    Icon(
                        painterResource(if (state.isPlaying) R.drawable.ic_pause else R.drawable.ic_play),
                        contentDescription = if (state.isPlaying) "Pause" else "Play",
                        tint = EncoreColors.Background,
                        modifier = Modifier.size(36.dp),
                    )
                }
            }
            IconButton(onClick = { nav.player.next() }, enabled = state.hasNext, modifier = Modifier.size(56.dp)) {
                Icon(painterResource(R.drawable.ic_skip_next), contentDescription = "Next", modifier = Modifier.size(32.dp))
            }
        }
        state.error?.let {
            Text(
                it,
                color = MaterialTheme.colorScheme.error,
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 4.dp),
                textAlign = TextAlign.Center,
            )
        }
        Spacer(Modifier.height(8.dp))
    }
}

@Composable
private fun QueueList(nav: Nav, state: PlayerUiState, modifier: Modifier) {
    val listState = rememberLazyListState(initialFirstVisibleItemIndex = state.index.coerceAtLeast(0))
    LazyColumn(modifier.fillMaxWidth(), state = listState) {
        itemsIndexed(state.queue) { i, q ->
            val current = i == state.index
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(if (current) MaterialTheme.colorScheme.surfaceContainerHigh else MaterialTheme.colorScheme.background)
                    .clickable { nav.player.skipTo(i) }
                    .padding(start = 16.dp, end = 4.dp, top = 6.dp, bottom = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Cover(if (q.albumId.isNotEmpty()) nav.graph.api.coverUrl(q.albumId, 96) else null, nav.graph.imageLoader, size = 40.dp, corner = 6.dp)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(
                        q.title,
                        color = if (current) MaterialTheme.colorScheme.secondary else MaterialTheme.colorScheme.onSurface,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(q.artist, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                if (!current) {
                    IconButton(onClick = { nav.player.removeAt(i) }) {
                        Icon(painterResource(R.drawable.ic_close), contentDescription = "Remove from queue", tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
        if (state.queue.isEmpty()) {
            item {
                TextButton(onClick = {}) { Text("Nothing queued") }
            }
        }
    }
}
