package app.encore.android.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.lerp
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
import androidx.compose.runtime.LaunchedEffect
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
import app.encore.android.features.library.AlbumTypesRules
import app.encore.android.features.group.GroupListen
import app.encore.android.features.group.GroupPill
import app.encore.android.features.group.GroupRules
import app.encore.android.features.group.GroupUpNextSheet
import app.encore.android.remote.LinkedVolume
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.features.offline.OfflineMode
import app.encore.android.features.offline.rememberOfflineMenu
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.LikeButton
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.PlayerMenuActions
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDay
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.components.rememberMotionOn
import app.encore.android.ui.components.scrollingTitle
import app.encore.android.ui.theme.palette
import coil3.compose.AsyncImage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

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
                if (t.albumArtist.isEmpty() && t.artist.isEmpty() && t.artists.isEmpty()) return
                onClose()
                openTrackArtist(nav, t)
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
                val motion = rememberMotionOn()
                Text(track.shownTitle(), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.scrollingTitle(motion))
                Text(track.shownArtist(), style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.scrollingTitle(motion))
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

/** The thumbnail the cover becomes in lyrics mode. */
private val THUMB = 56.dp

/**
 * The full-screen player (the web's `.np-full`): the cover blurred as a backdrop under a dark wash, the big art,
 * title / artist · album, the quality badge, the seek bar, prev / play / next, and like, share and "⋯".
 *
 * Lyrics mode (the Lyrics button, ⋯ → Lyrics; remembered as [LyricsMode]): the cover glides into a 56 dp thumbnail
 * beside the title, artist, play / pause and a thin seek line, and the lyrics take the rest of the screen ([LyricsPane]).
 * "Back to the cover", a tap on the thumbnail or Back return to the big art.
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
    val shuffle by nav.player.shuffle.collectAsState()
    val repeat by nav.player.repeat.collectAsState()
    // a group listen: the pill in the top bar, and the ⋯ menu's "Start a group listen" / "Group listen"
    val group by GroupListen.state.collectAsState()
    val groupsHere by GroupListen.available.collectAsState()
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val lyricsMode by remember { LyricsMode.state(context) }.collectAsState()
    val toLyrics = { LyricsMode.set(context, true) }
    val toCover = { LyricsMode.set(context, false) }
    // Back in lyrics mode goes back to the cover first
    BackHandler(enabled = lyricsMode) { toCover() }
    val prog by animateFloatAsState(if (lyricsMode) 1f else 0f, tween(420, easing = FastOutSlowInEasing), label = "lyricsMode")
    val lyrics = rememberLyrics(nav, track.id, active = lyricsMode)
    // "Download for offline" / "Remove download" for the song (features/offline); not while this phone is a remote
    val offlineMenu = rememberOfflineMenu(nav)
    // who added the song (0.36, GET /api/library/tracks/{id}/added): the ⋯ menu's first line, asked when the menu
    // opens as in every song list; none until the server said, when it has nothing to say, offline, or from an older server
    var askAdded by remember(track.id) { mutableStateOf(false) }
    var added by remember(track.id) { mutableStateOf("") }
    LaunchedEffect(track.id, askAdded) {
        if (!askAdded || track.id <= 0 || OfflineMode.active.value) return@LaunchedEffect
        added = try {
            withContext(Dispatchers.IO) { AlbumTypesRules.addedOf(nav.graph.api.trackAdded(track.id), ::formatDay) }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            ""
        }
    }
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
                GroupPill(Modifier.padding(end = 4.dp))
                DevicesButton(nav, tint = p.text)
                IconButton(onClick = { actions.upNext() }) {
                    Icon(painterResource(R.drawable.ic_queue_music), contentDescription = "Up next", tint = p.text)
                }
            }

            // .np-art, and in lyrics mode the compact bar and the lyrics: the cover moves between the two places
            BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
                val big = maxOf(minOf(maxWidth - 64.dp, maxHeight - 16.dp), THUMB)
                val size = lerp(big, THUMB, prog)
                val x = lerp((maxWidth - big) / 2, 20.dp, prog)
                val y = lerp((maxHeight - big) / 2, 4.dp, prog)
                val corner = lerp(20.dp, 8.dp, prog)
                if (prog > 0f) {
                    Column(Modifier.fillMaxSize().alpha(prog)) {
                        // the compact bar: the thumbnail's place, title / artist, play / pause
                        Row(
                            Modifier.fillMaxWidth().padding(start = 20.dp + THUMB + 12.dp, end = 8.dp, top = 4.dp).height(THUMB),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f)) {
                                val motion = rememberMotionOn()
                                Text(track.shownTitle(), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.scrollingTitle(motion))
                                Text(track.shownArtist(), style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.scrollingTitle(motion))
                            }
                            IconButton(onClick = { nav.player.togglePlay() }) {
                                Icon(
                                    painterResource(if (isPlaying) R.drawable.ic_pause else R.drawable.ic_play),
                                    contentDescription = if (isPlaying) "Pause" else "Play",
                                    tint = p.text,
                                )
                            }
                        }
                        ThinSeek(position, duration, onSeek = { ms -> nav.player.seekTo(ms) }, Modifier.padding(start = 20.dp, end = 20.dp, top = 6.dp))
                        LyricsPane(nav, lyrics, position, Modifier.weight(1f).fillMaxWidth())
                    }
                }
                Box(
                    Modifier
                        .offset(x, y)
                        .size(size)
                        .shadow(lerp(30.dp, 6.dp, prog), RoundedCornerShape(corner), clip = false)
                        .clickable(enabled = lyricsMode, onClickLabel = "Back to the cover") { toCover() },
                ) {
                    Cover(coverUrl, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = corner, label = track.album.ifEmpty { track.title })
                }
            }

            // lyrics mode: the way back to the big art
            AnimatedVisibility(visible = lyricsMode, enter = fadeIn(tween(300)), exit = fadeOut(tween(150))) {
                Box(Modifier.fillMaxWidth().padding(top = 6.dp, bottom = 14.dp), contentAlignment = Alignment.Center) {
                    Text(
                        "Back to the cover",
                        style = MaterialTheme.typography.labelLarge,
                        color = p.text,
                        modifier = Modifier
                            .clip(RoundedCornerShape(50))
                            .background(p.panel2.copy(alpha = 0.85f))
                            .clickable { toCover() }
                            .padding(horizontal = 18.dp, vertical = 10.dp),
                    )
                }
            }

            AnimatedVisibility(
                visible = !lyricsMode,
                enter = expandVertically(tween(420, easing = FastOutSlowInEasing)) + fadeIn(tween(420)),
                exit = shrinkVertically(tween(420, easing = FastOutSlowInEasing)) + fadeOut(tween(200)),
            ) {
                Column(Modifier.fillMaxWidth()) {
                    // .np-meta
                    Column(Modifier.fillMaxWidth().padding(horizontal = 28.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                        // a title too long for the line scrolls sideways instead of ending in "…"
                        val motion = rememberMotionOn()
                        Text(
                            track.shownTitle(),
                            style = MaterialTheme.typography.headlineSmall,
                            textAlign = TextAlign.Center,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.scrollingTitle(motion),
                        )
                        track.titleUnder()?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = p.muted, textAlign = TextAlign.Center, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.scrollingTitle(motion)) }
                        // each artist and the album open their pages (closing the player)
                        Text(
                            rememberSongLinks(nav, track, before = onClose),
                            style = MaterialTheme.typography.bodyMedium,
                            color = p.muted,
                            textAlign = TextAlign.Center,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.padding(top = 4.dp).scrollingTitle(motion),
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

                    // .np-ctrls: shuffle, prev, play / pause, next, repeat (the same shuffle and repeat as the car and lock screen)
                    Row(
                        Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 4.dp),
                        horizontalArrangement = Arrangement.Center,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        IconButton(onClick = { nav.player.setShuffle(!shuffle) }, modifier = Modifier.size(48.dp)) {
                            Icon(
                                painterResource(R.drawable.ic_shuffle),
                                contentDescription = if (shuffle) "Shuffle: on" else "Shuffle: off",
                                tint = if (shuffle) p.accent else p.muted,
                            )
                        }
                        Spacer(Modifier.width(4.dp))
                        IconButton(onClick = { nav.player.previous() }, modifier = Modifier.size(64.dp)) {
                            Icon(painterResource(R.drawable.ic_skip_previous), contentDescription = "Previous", tint = p.text, modifier = Modifier.size(36.dp))
                        }
                        Spacer(Modifier.width(12.dp))
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
                        Spacer(Modifier.width(12.dp))
                        val hasNext = index < queue.lastIndex
                        IconButton(onClick = { nav.player.next() }, enabled = hasNext, modifier = Modifier.size(64.dp)) {
                            Icon(painterResource(R.drawable.ic_skip_next), contentDescription = "Next", tint = if (hasNext) p.text else p.faint, modifier = Modifier.size(36.dp))
                        }
                        Spacer(Modifier.width(4.dp))
                        IconButton(onClick = { nav.player.cycleRepeat() }, modifier = Modifier.size(48.dp)) {
                            Icon(
                                painterResource(if (repeat == "one") R.drawable.ic_repeat_one else R.drawable.ic_repeat),
                                contentDescription = when (repeat) { "one" -> "Repeat: this song"; "all" -> "Repeat: the queue"; else -> "Repeat: off" },
                                tint = if (repeat != "off") p.accent else p.muted,
                            )
                        }
                    }

                    // linked: the other device's volume
                    LinkedVolume(nav, Modifier.fillMaxWidth().padding(horizontal = 48.dp))

                    // .np-actions: like, share, lyrics, ⋯ (a Surround song has no like and no playlists)
                    Row(
                        Modifier.fillMaxWidth().padding(bottom = 12.dp),
                        horizontalArrangement = Arrangement.spacedBy(18.dp, Alignment.CenterHorizontally),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        if (!track.isSurround) LikeButton(track.id in liked, { nav.graph.likes.toggle(track.id) })
                        IconButton(onClick = { actions.shareLink() }) {
                            Icon(painterResource(R.drawable.ic_share), contentDescription = "Share link", tint = p.text)
                        }
                        IconButton(onClick = toLyrics) {
                            Icon(painterResource(R.drawable.ic_lyrics), contentDescription = "Lyrics", tint = p.text)
                        }
                        MoreButton(
                            onOpen = { askAdded = true },
                            entries = listOf(
                                if (added.isNotEmpty()) MenuEntry(added, info = true, onClick = {}) else null,
                                if (track.artist.isNotEmpty() || track.albumArtist.isNotEmpty()) MenuEntry("Go to artist", R.drawable.ic_person) { actions.goToArtist() } else null,
                                if (track.albumId.isNotEmpty()) MenuEntry("View album", R.drawable.ic_album) { actions.viewAlbum() } else null,
                                if (!track.isSurround) MenuEntry("Add to playlist", R.drawable.ic_playlist_add) { actions.addToPlaylist() } else null,
                                MenuEntry("Lyrics", R.drawable.ic_lyrics) { toLyrics() },
                                MenuEntry("Share link", R.drawable.ic_share) { actions.shareLink() },
                                MenuEntry("Signal path", R.drawable.ic_signal) { actions.signalPath() },
                                when {
                                    group != null -> MenuEntry("Group listen", R.drawable.ic_group) { GroupListen.sheetOpen.value = true }
                                    groupsHere == true && link == null -> MenuEntry("Start a group listen", R.drawable.ic_group) { GroupListen.startOpen.value = true }
                                    else -> null
                                },
                                // the group's own pause (Pause pauses only this phone), for those who control it while it plays
                                group?.takeIf { GroupRules.offersPauseForEveryone(it) }?.let {
                                    MenuEntry(GroupRules.PAUSE_FOR_EVERYONE, R.drawable.ic_pause) { GroupListen.pauseForEveryone() }
                                },
                                *(if (link == null && !track.isSurround) offlineMenu.entries(listOf(track)) else emptyList()).toTypedArray(),
                            ),
                            tint = p.text,
                        )
                    }
                }
            }
        }
    }
}

/**
 * The thin seek line of lyrics mode: the played part in the accent colour; a tap or a drag along it seeks (the
 * position follows the finger while dragging and is sent when you let go).
 */
@Composable
private fun ThinSeek(positionMs: Long, durationMs: Long, onSeek: (Long) -> Unit, modifier: Modifier = Modifier) {
    val p = MaterialTheme.palette
    val dur = durationMs.coerceAtLeast(0L)
    var drag by remember { mutableStateOf<Float?>(null) }
    val frac = drag ?: if (dur > 0) (positionMs.toFloat() / dur).coerceIn(0f, 1f) else 0f
    Box(
        modifier
            .fillMaxWidth()
            .height(20.dp)
            .pointerInput(dur) {
                detectTapGestures { o -> if (dur > 0 && size.width > 0) onSeek(((o.x / size.width).coerceIn(0f, 1f) * dur).toLong()) }
            }
            .pointerInput(dur) {
                detectHorizontalDragGestures(
                    onDragStart = { o -> if (dur > 0 && size.width > 0) drag = (o.x / size.width).coerceIn(0f, 1f) },
                    onDragEnd = {
                        drag?.let { f -> onSeek((f * dur).toLong()) }
                        drag = null
                    },
                    onDragCancel = { drag = null },
                ) { change, _ ->
                    if (dur > 0 && size.width > 0) drag = (change.position.x / size.width).coerceIn(0f, 1f)
                }
            },
        contentAlignment = Alignment.CenterStart,
    ) {
        Box(Modifier.fillMaxWidth().height(if (drag != null) 5.dp else 3.dp).clip(RoundedCornerShape(3.dp)).background(p.border)) {
            Box(Modifier.fillMaxWidth(frac).fillMaxHeight().background(p.accent))
        }
    }
}

/**
 * The "Up next" sheet (the web's `.np-queue`): what plays after this song, with Clear (every song after this one, with
 * Undo for 6 s) and "Then shuffle library".
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun UpNextSheet(nav: Nav, onDismiss: () -> Unit) {
    // in a group listen: the group's queue, with who added each song
    val group by GroupListen.state.collectAsState()
    group?.let { GroupUpNextSheet(nav, it, onDismiss); return }
    val queue by nav.player.queue.collectAsState()
    val index by nav.player.queueIndex.collectAsState()
    val settings by nav.graph.store.settings.collectAsState()
    val p = MaterialTheme.palette
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val upNext = if (index + 1 <= queue.lastIndex) queue.subList(index + 1, queue.size) else emptyList()
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Row(Modifier.fillMaxWidth().padding(start = 22.dp, end = 12.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Up next", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            SmallButton("Clear", iconRes = R.drawable.ic_delete, enabled = upNext.isNotEmpty()) { nav.player.clearUpNext() }
            Spacer(Modifier.width(4.dp))
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
                        Text(q.shownTitle(), fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(rememberSongLinks(nav, q, withAlbum = false, before = onDismiss), style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
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
