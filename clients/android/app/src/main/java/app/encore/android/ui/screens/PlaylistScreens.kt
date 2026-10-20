package app.encore.android.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.features.library.OnSongFixed
import app.encore.android.features.library.PlaylistShareRules
import app.encore.android.R
import app.encore.android.data.api.Playlist
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Collage
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.Loadable
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.MoreButton
import app.encore.android.features.offline.rememberOfflineMenu
import app.encore.android.ui.components.Tile
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.components.shareLinkOf
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette

/** The web's Liked Songs art: a heart on the accent gradient. */
@Composable
private fun LikedArt(modifier: Modifier, corner: Dp, iconSize: Dp) {
    val p = MaterialTheme.palette
    Box(modifier.clip(RoundedCornerShape(corner)).background(p.gradient), contentAlignment = Alignment.Center) {
        Icon(painterResource(R.drawable.ic_favorite), contentDescription = null, tint = p.onAccent, modifier = Modifier.size(iconSize))
    }
}

/**
 * A playlist's cover: the 2×2 collage of its first four albums (the web's `.collage`), the saved cover when
 * there are fewer, else the lettered placeholder. The albums come from the playlist's rows, loaded lazily.
 */
@Composable
private fun PlaylistArt(nav: Nav, pl: Playlist, modifier: Modifier, corner: Dp = 12.dp) {
    val api = nav.graph.api
    val loader = rememberLoad("playlist-art", pl.id) {
        api.playlist(pl.id).rows.mapNotNull { it.track?.albumId?.takeIf { id -> id.isNotEmpty() } }.distinct().take(4)
    }
    val covers = when (val s = loader.state.value) {
        is Loadable.Ready -> s.value.map { api.coverUrl(it, 400) }
        else -> emptyList()
    }
    if (covers.size >= 4) {
        Collage(covers, nav.graph.imageLoader, modifier, corner = corner, label = pl.name)
    } else {
        Cover(pl.coverUrl.ifEmpty { covers.firstOrNull() }, nav.graph.imageLoader, modifier, corner = corner, label = pl.name)
    }
}

/** Playlists, Liked Songs first (web/js/pages/playlists.js `plCard` tiles). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlaylistsScreen(nav: Nav) {
    val api = nav.graph.api
    val loader = rememberLoad("playlists") { api.playlists() }
    val p = MaterialTheme.palette
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("Playlists", scrollBehavior = scrollBehavior, actions = { PlaylistsActions(nav) }) },
    ) { padding ->
        LoadableContent(loader) { playlists ->
            val sorted = playlists.sortedByDescending { it.isLiked }
            if (sorted.isEmpty()) {
                EmptyState(R.drawable.ic_queue_music, "No playlists yet", modifier = Modifier.padding(padding))
                return@LoadableContent
            }
            LazyVerticalGrid(
                columns = GridCells.Adaptive(140.dp),
                modifier = Modifier.fillMaxSize().padding(padding),
                contentPadding = PaddingValues(start = Gutter, end = Gutter, top = 4.dp, bottom = 24.dp),
                horizontalArrangement = Arrangement.spacedBy(14.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                items(sorted, key = { it.id }) { pl ->
                    Tile(
                        title = pl.name,
                        subtitle = PlaylistShareRules.tileSubtitle(pl.total, pl.matched, pl.isShared),
                        onClick = { nav.open(Screen.Playlist(pl.id, pl.name)) },
                    ) { m ->
                        if (pl.isLiked) LikedArt(m, 12.dp, 48.dp) else PlaylistArt(nav, pl, m)
                    }
                }
            }
        }
    }
}

/** One playlist (web/js/pages/playlists.js `renderPlaylist`, read-only here; editing is the features part). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlaylistScreen(nav: Nav, id: Long, name: String) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    // Liked Songs changes as you like things: reload when the set changes
    val loader = rememberLoad(id, if (name == "Liked Songs") liked.size else 0) { api.playlist(id) }
    // a song corrected or deleted from its ⋯: the playlist loads again in place
    OnSongFixed { loader.refresh() }
    // "Download for offline" / "Remove download" for the playlist's songs (features/offline)
    val offlineMenu = rememberOfflineMenu(nav)
    val p = MaterialTheme.palette
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    // the loaded playlist (its current name, and its link while it is shared)
    val loaded = (loader.state.value as? Loadable.Ready)?.value?.playlist
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            EncoreTopBar("", scrollBehavior = scrollBehavior, onBack = nav.back, actions = {
                // the playlist's public link (Liked Songs too): made, copied, then the system share sheet
                IconButton(onClick = {
                    val title = loaded?.name?.ifEmpty { null } ?: name
                    shareLinkOf(context, scope, subject = title, text = title, toast = PlaylistShareRules.LINK_COPIED, after = { loader.refresh() }) {
                        api.sharePlaylistLink(id)
                    }
                }) {
                    Icon(painterResource(R.drawable.ic_share), contentDescription = "Share link")
                }
                PlaylistMenu(
                    nav, id, name, isLiked = name == "Liked Songs", onChanged = { loader.retry() },
                    shareUrl = loaded?.takeIf { it.isShared }?.let { api.url(it.sharePath) },
                )
            })
        },
    ) { padding ->
        LoadableContent(loader) { detail ->
            val pl = detail.playlist
            val tracks = detail.tracks
            val rows = detail.rows
            val missing = rows.size - tracks.size
            val covers = rows.mapNotNull { it.track?.albumId?.takeIf { a -> a.isNotEmpty() } }.distinct().take(4).map { api.coverUrl(it, 400) }
            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                // .detail-head
                item {
                    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter), horizontalAlignment = Alignment.CenterHorizontally) {
                        Box(Modifier.size(180.dp).shadow(24.dp, RoundedCornerShape(16.dp), clip = false)) {
                            when {
                                pl.isLiked -> LikedArt(Modifier.fillMaxSize(), 16.dp, 72.dp)
                                covers.size >= 4 -> Collage(covers, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 16.dp, label = pl.name)
                                else -> Cover(pl.coverUrl.ifEmpty { covers.firstOrNull() }, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 16.dp, label = pl.name)
                            }
                        }
                        Spacer(Modifier.height(18.dp))
                        Text(
                            if (pl.isLiked) "LIKED SONGS · SYNCED EVERYWHERE" else "PLAYLIST",
                            style = MaterialTheme.typography.labelSmall,
                            color = p.muted,
                            letterSpacing = 1.2.sp,
                        )
                        Text(pl.name.ifEmpty { name }, style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 6.dp))
                        Row(Modifier.padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text("${rows.size} tracks · ", style = MaterialTheme.typography.bodyMedium, color = p.muted)
                            Text("${tracks.size}", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold, color = EncoreColors.Ok)
                            Text(" in library · $missing missing", style = MaterialTheme.typography.bodyMedium, color = p.muted)
                        }
                        // the web's Shared mark: anyone with the link can play these songs
                        if (pl.isShared) Badge("Shared", "accent", Modifier.padding(top = 8.dp))
                        // .progress
                        val frac = if (rows.isNotEmpty()) tracks.size.toFloat() / rows.size else 0f
                        Box(Modifier.padding(top = 12.dp).width(240.dp).height(6.dp).clip(RoundedCornerShape(99.dp)).background(p.panel2)) {
                            Box(Modifier.fillMaxWidth(frac).fillMaxHeight().background(p.gradient))
                        }
                    }
                }
                item {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                        if (tracks.isNotEmpty()) {
                            PlayButtons(nav, tracks) {
                                val entries = offlineMenu.entries(tracks)
                                if (entries.isNotEmpty()) MoreButton(entries = entries)
                            }
                        } else Spacer(Modifier.height(16.dp))
                    }
                }
                if (rows.isEmpty()) {
                    item { EmptyState(R.drawable.ic_note, "Empty playlist", "Add tracks from an album page or from a link.") }
                }
                items(rows.size) { i ->
                    val row = rows[i]
                    val t = row.track
                    if (t != null) {
                        val at = rows.subList(0, i).count { it.track != null }
                        TrackRow(
                            track = t,
                            imageLoader = nav.graph.imageLoader,
                            coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                            liked = t.id in liked,
                            playing = nowPlaying?.id == t.id,
                            onClick = { nav.player.play(tracks, at) },
                            onLike = { nav.graph.likes.toggle(t.id) },
                            subtitle = listOf(t.artist, t.album).filter { it.isNotEmpty() }.joinToString(" · "),
                            subtitleLinks = rememberSongLinks(nav, t),
                            onMore = { nav.trackMenu(t) },
                        )
                    } else {
                        // not in the library: greyed, with its request's status or the web's "missing" badge
                        Row(
                            Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 10.dp, bottom = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                (i + 1).toString(),
                                style = MaterialTheme.typography.bodyMedium,
                                color = p.faint,
                                textAlign = TextAlign.End,
                                modifier = Modifier.width(28.dp),
                            )
                            Spacer(Modifier.width(14.dp))
                            Column(Modifier.weight(1f)) {
                                Text(row.title, fontWeight = FontWeight.SemiBold, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text(row.artist, style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                            if (row.requestStatus.isNotEmpty()) StatusBadge(row.requestStatus) else Badge("missing")
                        }
                    }
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
}
