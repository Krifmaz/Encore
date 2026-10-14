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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Track
import app.encore.android.remote.DevicesButton
import app.encore.android.remote.LinkedVolume
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.LikeButton
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.PlayerMenuActions
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.theme.palette
import coil3.compose.AsyncImage

/**
 * The "⋯" menu of the player (web/js/player.js `moreMenu`). Navigation (Go to artist, View album, Up next,
 * Add to playlist) is handled here; Lyrics, Share link and Signal path go to the player part through
 * `PlayerConnection.menuActions`, with a plain fallback until that is wired.
 */
@Composable
fun rememberPlayerMenuActions(
    nav: Nav,
    onClose: () -> Unit,
    onUpNext: () -> Unit,
    onAddToPlaylist: (List<Long>) -> Unit,
): PlayerMenuActions {
    return remember(nav) {
        object : PlayerMenuActions {
            private fun current(): Track? = nav.player.nowPlaying.value
            private fun delegate(): PlayerMenuActions? = nav.player.menuActions

            override fun goToArtist() {
                val t = current() ?: return
                val artist = t.albumArtist.ifEmpty { t.artist }
                if (artist.isEmpty()) return
                onClose()
                nav.open(Screen.Artist(artist))
            }

            override fun viewAlbum() {
                val t = current() ?: return
                if (t.albumId.isEmpty()) return
                onClose()
                nav.open(Screen.Album(t.albumId))
            }

            override fun addToPlaylist() {
                val t = current() ?: return
                onAddToPlaylist(listOf(t.id))
            }

            override fun lyrics() {
                val d = delegate()
                if (d != null) d.lyrics() else Toasts.show("Lyrics aren't available yet", "err")
            }

            override fun shareLink() {
                val d = delegate()
                if (d != null) d.shareLink() else Toasts.show("Sharing isn't available yet", "err")
            }

            override fun signalPath() {
                val d = delegate()
                if (d != null) d.signalPath() else Toasts.show(nav.player.qualityTitle.value ?: "Signal path isn't available yet", "info")
            }

            override fun upNext() { onUpNext() }
        }
    }
}

/** The bar above the tabs (the web's `.player` on phones): cover, title, play/pause, next and a thin progress line. */
@Composable
fun MiniPlayer(nav: Nav, onOpen: () -> Unit) {
    val t by nav.player.nowPlaying.collectAsState()
    val track = t ?: return
    val isPlaying by nav.player.isPlaying.collectAsState()
    val position by nav.player.positionMs.collectAsState()
    val duration by nav.player.durationMs.collectAsState()
    val queue by nav.player.queue.collectAsState()
    val index by nav.player.queueIndex.collectAsState()
    val p = MaterialTheme.palette
    val fraction = if (duration > 0) (position.toFloat() / duration).coerceIn(0f, 1f) else 0f
    Column(Modifier.fillMaxWidth().background(p.bg2)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(p.border))
        Box(Modifier.fillMaxWidth().height(3.dp).background(p.border)) {
            Box(Modifier.fillMaxWidth(fraction).fillMaxHeight().background(p.gradient))
        }
        Row(
            Modifier.fillMaxWidth().clickable(onClick = onOpen).padding(start = 12.dp, end = 4.dp, top = 8.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Cover(
                if (track.albumId.isNotEmpty()) nav.graph.api.coverUrl(track.albumId, 96) else null,
                nav.graph.imageLoader,
                size = 48.dp,
                corner = 8.dp,
                label = track.album.ifEmpty { track.title },
            )
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(track.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(track.artist, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            DevicesButton(nav, tint = p.muted)
            IconButton(onClick = { nav.player.togglePlay() }) {
                Icon(
                    painterResource(if (isPlaying) R.drawable.ic_pause else R.drawable.ic_play),
                    contentDescription = if (isPlaying) "Pause" else "Play",
                    tint = p.text,
                )
            }
            IconButton(onClick = { nav.player.next() }, enabled = index < queue.lastIndex) {
                Icon(painterResource(R.drawable.ic_skip_next), contentDescription = "Next", tint = if (index < queue.lastIndex) p.text else p.faint)
            }
        }
    }
}

/**
 * The full-screen player (the web's `.np-full`): the cover blurred as a backdrop under a dark wash, the big art,
 * title / artist · album, the quality badge, the seek bar, prev / play / next, and like, share and "⋯".
 */
@Composable
fun FullPlayer(nav: Nav, actions: PlayerMenuActions, onClose: () -> Unit) {
    val t by nav.player.nowPlaying.collectAsState()
    val track = t ?: return
    val isPlaying by nav.player.isPlaying.collectAsState()
    val position by nav.player.positionMs.collectAsState()
    val duration by nav.player.durationMs.collectAsState()
    val queue by nav.player.queue.collectAsState()
    val index by nav.player.queueIndex.collectAsState()
    val badge by nav.player.qualityBadge.collectAsState()
    val liked by nav.graph.likes.liked.collectAsState()
    val link by nav.player.remote.link.collectAsState()
    val p = MaterialTheme.palette
    var dragging by remember { mutableStateOf<Float?>(null) }
    val coverUrl = if (track.albumId.isNotEmpty()) nav.graph.api.coverUrl(track.albumId, 1000) else null

    Box(
        Modifier
            .fillMaxSize()
            .background(p.bg)
            // swallow taps so they don't reach the screen underneath
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = {}),
    ) {
        // .np-backdrop: the cover, blurred and washed out, under a gradient that keeps the text readable
        if (coverUrl != null) {
            AsyncImage(
                model = coverUrl,
                contentDescription = null,
                imageLoader = nav.graph.imageLoader,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize().blur(60.dp).alpha(if (p.dark) 0.45f else 0.25f),
            )
        }
        Box(
            Modifier.fillMaxSize().background(
                Brush.verticalGradient(listOf(p.bg.copy(alpha = 0.55f), p.bg.copy(alpha = 0.80f), p.bg.copy(alpha = 0.97f))),
            ),
        )

        Column(Modifier.fillMaxSize().safeDrawingPadding()) {
            // .np-top
            Row(Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = onClose) { Icon(painterResource(R.drawable.ic_expand_more), contentDescription = "Close", tint = p.text) }
                Text(
                    link?.let { "Controlling ${it.name}" + if (!it.online) " · offline" else "" } ?: "Now playing",
                    style = MaterialTheme.typography.titleSmall,
                    color = p.muted,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.weight(1f),
                )
                DevicesButton(nav, tint = p.text)
                IconButton(onClick = { actions.upNext() }) {
                    Icon(painterResource(R.drawable.ic_queue_music), contentDescription = "Up next", tint = p.text)
                }
            }

            // .np-art
            Box(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 32.dp, vertical = 8.dp), contentAlignment = Alignment.Center) {
                Box(Modifier.fillMaxWidth().aspectRatio(1f).shadow(30.dp, RoundedCornerShape(20.dp), clip = false)) {
                    Cover(coverUrl, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 20.dp, label = track.album.ifEmpty { track.title })
                }
            }

            // .np-meta
            Column(Modifier.fillMaxWidth().padding(horizontal = 28.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                Text(
                    track.title,
                    style = MaterialTheme.typography.headlineSmall,
                    textAlign = TextAlign.Center,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    listOf(track.artist, track.album).filter { it.isNotEmpty() }.joinToString(" · "),
                    style = MaterialTheme.typography.bodyMedium,
                    color = p.muted,
                    textAlign = TextAlign.Center,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(top = 4.dp).clickable { if (track.albumId.isNotEmpty()) actions.viewAlbum() else actions.goToArtist() },
                )
                val q = badge
                if (!q.isNullOrEmpty()) {
                    Spacer(Modifier.height(10.dp))
                    Badge(q, onClick = { actions.signalPath() })
                }
            }

            // .np-seek
            val dur = duration.coerceAtLeast(0L)
            val shown = dragging ?: if (dur > 0) (position.toFloat() / dur).coerceIn(0f, 1f) else 0f
            Slider(
                value = shown,
                onValueChange = { dragging = it },
                onValueChangeFinished = {
                    val d = dragging
                    if (d != null && dur > 0) nav.player.seekTo((d * dur).toLong())
                    dragging = null
                },
                enabled = dur > 0,
                colors = SliderDefaults.colors(
                    thumbColor = p.text,
                    activeTrackColor = p.accent,
                    inactiveTrackColor = p.border,
                    disabledThumbColor = p.faint,
                    disabledActiveTrackColor = p.border,
                    disabledInactiveTrackColor = p.border,
                ),
                modifier = Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 12.dp),
            )
            Row(Modifier.fillMaxWidth().padding(horizontal = 28.dp)) {
                val cur = dragging?.let { (it * dur).toLong() } ?: position
                Text(formatDuration(cur).ifEmpty { "0:00" }, style = MaterialTheme.typography.bodySmall, color = p.faint)
                Spacer(Modifier.weight(1f))
                Text(formatDuration(dur).ifEmpty { "0:00" }, style = MaterialTheme.typography.bodySmall, color = p.faint)
            }

            // .np-ctrls
            Row(
                Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 4.dp),
                horizontalArrangement = Arrangement.Center,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = { nav.player.previous() }, modifier = Modifier.size(64.dp)) {
                    Icon(painterResource(R.drawable.ic_skip_previous), contentDescription = "Previous", tint = p.text, modifier = Modifier.size(36.dp))
                }
                Spacer(Modifier.width(20.dp))
                Box(
                    Modifier
                        .size(72.dp)
                        .clip(CircleShape)
                        .background(p.gradient)
                        .clickable { nav.player.togglePlay() },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        painterResource(if (isPlaying) R.drawable.ic_pause else R.drawable.ic_play),
                        contentDescription = if (isPlaying) "Pause" else "Play",
                        tint = p.onAccent,
                        modifier = Modifier.size(36.dp),
                    )
                }
                Spacer(Modifier.width(20.dp))
                val hasNext = index < queue.lastIndex
                IconButton(onClick = { nav.player.next() }, enabled = hasNext, modifier = Modifier.size(64.dp)) {
                    Icon(painterResource(R.drawable.ic_skip_next), contentDescription = "Next", tint = if (hasNext) p.text else p.faint, modifier = Modifier.size(36.dp))
                }
            }

            // linked: the other device's volume
            LinkedVolume(nav, Modifier.fillMaxWidth().padding(horizontal = 48.dp))

            // .np-actions: like, share, ⋯
            Row(
                Modifier.fillMaxWidth().padding(bottom = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(18.dp, Alignment.CenterHorizontally),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                LikeButton(track.id in liked, { nav.graph.likes.toggle(track.id) })
                IconButton(onClick = { actions.shareLink() }) {
                    Icon(painterResource(R.drawable.ic_share), contentDescription = "Share link", tint = p.text)
                }
                MoreButton(
                    entries = listOf(
                        if (track.artist.isNotEmpty() || track.albumArtist.isNotEmpty()) MenuEntry("Go to artist", R.drawable.ic_person) { actions.goToArtist() } else null,
                        if (track.albumId.isNotEmpty()) MenuEntry("View album", R.drawable.ic_album) { actions.viewAlbum() } else null,
                        MenuEntry("Add to playlist", R.drawable.ic_playlist_add) { actions.addToPlaylist() },
                        MenuEntry("Lyrics", R.drawable.ic_lyrics) { actions.lyrics() },
                        MenuEntry("Share link", R.drawable.ic_share) { actions.shareLink() },
                        MenuEntry("Signal path", R.drawable.ic_signal) { actions.signalPath() },
                    ),
                    tint = p.text,
                )
            }
        }
    }
}

/** The "Up next" sheet (the web's `.np-queue`): what plays after this song, with "Then shuffle library". */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun UpNextSheet(nav: Nav, onDismiss: () -> Unit) {
    val queue by nav.player.queue.collectAsState()
    val index by nav.player.queueIndex.collectAsState()
    val settings by nav.graph.store.settings.collectAsState()
    val p = MaterialTheme.palette
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val upNext = if (index + 1 <= queue.lastIndex) queue.subList(index + 1, queue.size) else emptyList()
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Row(Modifier.fillMaxWidth().padding(start = 22.dp, end = 12.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Up next", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            Row(
                Modifier.clickable { nav.graph.store.setAutoplay(!settings.autoplay) },
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Checkbox(
                    checked = settings.autoplay,
                    onCheckedChange = { nav.graph.store.setAutoplay(it) },
                    colors = CheckboxDefaults.colors(checkedColor = p.accent, checkmarkColor = p.onAccent, uncheckedColor = p.muted),
                )
                Text("Then shuffle library", style = MaterialTheme.typography.bodySmall, color = p.muted)
            }
        }
        LazyColumn(Modifier.fillMaxWidth().fillMaxHeight(0.7f)) {
            itemsIndexed(upNext) { k, q ->
                val i = index + 1 + k
                Row(
                    Modifier.fillMaxWidth().clickable { nav.player.skipTo(i) }.padding(start = 16.dp, end = 4.dp, top = 6.dp, bottom = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Cover(
                        if (q.albumId.isNotEmpty()) nav.graph.api.coverUrl(q.albumId, 96) else null,
                        nav.graph.imageLoader,
                        size = 42.dp,
                        corner = 8.dp,
                        label = q.album.ifEmpty { q.title },
                    )
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f)) {
                        Text(q.title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(q.artist, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    IconButton(onClick = { nav.player.removeAt(i) }) {
                        Icon(painterResource(R.drawable.ic_close), contentDescription = "Remove", tint = p.muted)
                    }
                }
            }
            if (upNext.isEmpty()) {
                item {
                    Text(
                        if (settings.autoplay) "Nothing queued – a shuffle of your library plays next." else "Nothing queued.",
                        style = MaterialTheme.typography.bodySmall,
                        color = p.muted,
                        modifier = Modifier.padding(horizontal = 22.dp, vertical = 16.dp),
                    )
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }
}
