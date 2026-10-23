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
import androidx.compose.foundation.lazy.grid.GridItemSpan
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
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
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
import app.encore.android.features.library.CollabInvites
import app.encore.android.features.library.LinkedPlaylistMenu
import app.encore.android.features.library.PlaylistCollabRules
import app.encore.android.features.library.PlaylistLinkRules
import app.encore.android.features.library.PlaylistShareRules
import app.encore.android.R
import app.encore.android.data.api.Playlist
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Collage
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
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
    // the playlists, and the invites to collaborate waiting for the user (none when that ask fails); kept in memory,
    // so the tab opens again at once and refreshes quietly
    val loader = rememberLoad("playlists", keep = true) {
        api.playlists() to try { nav.features.collabInvites() } catch (e: Exception) { emptyList() }
    }
    val p = MaterialTheme.palette
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("Playlists", scrollBehavior = scrollBehavior, actions = { PlaylistsActions(nav) }) },
    ) { padding ->
        LoadableContent(loader) { (playlists, invites) ->
            val sorted = playlists.sortedByDescending { it.isLiked }
            if (sorted.isEmpty() && invites.isEmpty()) {
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
                if (invites.isNotEmpty()) {
                    item(key = "collab-invites", span = { GridItemSpan(maxLineSpan) }) {
                        CollabInvites(nav, invites) { loader.refresh() }
                    }
                }
                items(sorted, key = { it.id }) { pl ->
                    Tile(
                        title = pl.name,
                        // someone else's playlist added from a link: "12 tracks · by anna", or that the link ended
                        subtitle = if (pl.isLinked) PlaylistLinkRules.tileLine(pl.total, pl.linkedOwner, pl.linkedActive)
                        else PlaylistShareRules.tileSubtitle(pl.total, pl.matched, pl.isShared) +
                            PlaylistCollabRules.tileSuffix(pl.collab.collaborative, pl.isMember, pl.collab.owner),
                        onClick = { nav.open(Screen.Playlist(pl.id, pl.name)) },
                    ) { m ->
                        if (pl.isLiked) LikedArt(m, 12.dp, 48.dp) else PlaylistArt(nav, pl, m)
                    }
                }
            }
        }
    }
}

/**
 * One playlist (web/js/pages/playlists.js `renderPlaylist`, read-only here; editing is the features part).
 *
 * Someone else's playlist added from its share link (`kind: linked`, features/library/PlaylistLinkRules) is read-only:
 * no Share, no editing ⋯ (Save a copy and Remove from my playlists instead), "Playlist · by anna" and the owner's hint
 * instead of the library counts; once the link ended, the "stopped sharing" line and Remove.
 */
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
    // the loaded playlist (its current name, and its link while it is shared); the last one stays while it loads
    // again, so the top bar keeps its actions
    val ready = (loader.state.value as? Loadable.Ready)?.value?.playlist
    var lastLoaded by remember(id) { mutableStateOf<Playlist?>(null) }
    SideEffect { if (ready != null) lastLoaded = ready }
    val loaded = ready ?: lastLoaded
    // an added playlist's Remove confirm: from its ⋯, or from the page's button once the link ended
    val confirmRemove = remember(id) { mutableStateOf(false) }
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            EncoreTopBar("", scrollBehavior = scrollBehavior, onBack = nav.back, actions = {
                when {
                    // not known yet (first load, or it failed): an added playlist must not offer the own actions,
                    // Delete included, so there are none until it has loaded
                    loaded == null -> {}
                    // someone else's playlist: Save a copy, Remove from my playlists
                    loaded.isLinked -> LinkedPlaylistMenu(nav, id, loaded.name.ifEmpty { name }, loaded.linkedActive, confirmRemove)
                    // someone else's collaborative playlist: its songs, Members and Leave (no Share, no Rename / Delete)
                    loaded.isMember -> PlaylistMenu(nav, id, loaded.name.ifEmpty { name }, isLiked = false, onChanged = { loader.retry() }, member = true)
                    else -> {
                        // the playlist's public link (Liked Songs too): made, copied, then the system share sheet
                        IconButton(onClick = {
                            val title = loaded.name.ifEmpty { name }
                            shareLinkOf(context, scope, subject = title, text = title, toast = PlaylistShareRules.LINK_COPIED, after = { loader.refresh() }) {
                                api.sharePlaylistLink(id)
                            }
                        }) {
                            Icon(painterResource(R.drawable.ic_share), contentDescription = "Share link")
                        }
                        PlaylistMenu(
                            nav, id, name, isLiked = name == "Liked Songs", onChanged = { loader.retry() },
                            shareUrl = loaded.takeIf { it.isShared }?.let { api.url(it.sharePath) },
                            collaborative = loaded.collab.collaborative,
                        )
                    }
                }
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
                            when {
                                pl.isLinked -> PlaylistLinkRules.kicker(pl.linkedLiked, pl.linkedOwner).uppercase()
                                pl.isMember -> PlaylistCollabRules.kicker(pl.collab.owner).uppercase()
                                pl.isLiked -> "LIKED SONGS · SYNCED EVERYWHERE"
                                else -> "PLAYLIST"
                            },
                            style = MaterialTheme.typography.labelSmall,
                            color = p.muted,
                            letterSpacing = 1.2.sp,
                            textAlign = TextAlign.Center,
                        )
                        Text(pl.name.ifEmpty { name }, style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 6.dp))
                        if (pl.isLinked) {
                            LinkedHead(pl, rows.size) { confirmRemove.value = true }
                        } else {
                            Row(Modifier.padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text("${rows.size} tracks · ", style = MaterialTheme.typography.bodyMedium, color = p.muted)
                                Text("${tracks.size}", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold, color = EncoreColors.Ok)
                                Text(" in library · $missing missing", style = MaterialTheme.typography.bodyMedium, color = p.muted)
                            }
                            // the web's Shared mark: anyone with the link can play these songs; how many added it, never who
                            if (pl.isShared) {
                                Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    Badge("Shared", "accent")
                                    if (pl.addedBy > 0) Text(PlaylistLinkRules.addedBy(pl.addedBy), style = MaterialTheme.typography.bodySmall, color = p.muted)
                                }
                            }
                            // Collaborative: the owner's count of members, or what a member may do
                            if (pl.isMember || pl.collab.collaborative) {
                                Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    Badge(PlaylistCollabRules.TOGGLE, "accent")
                                    val line = if (pl.isMember) PlaylistCollabRules.MEMBER_HINT else PlaylistCollabRules.membersText(pl.collab.members)
                                    if (line.isNotEmpty()) Text(line, style = MaterialTheme.typography.bodySmall, color = p.muted)
                                }
                            }
                            // .progress
                            val frac = if (rows.isNotEmpty()) tracks.size.toFloat() / rows.size else 0f
                            Box(Modifier.padding(top = 12.dp).width(240.dp).height(6.dp).clip(RoundedCornerShape(99.dp)).background(p.panel2)) {
                                Box(Modifier.fillMaxWidth(frac).fillMaxHeight().background(p.gradient))
                            }
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
                    when {
                        // an added playlist whose link ended: the head says so, nothing to list
                        pl.isLinked && !pl.linkedActive -> {}
                        pl.isLinked -> item { EmptyState(R.drawable.ic_note, PlaylistLinkRules.EMPTY) }
                        else -> item { EmptyState(R.drawable.ic_note, "Empty playlist", "Add tracks from an album page or from a link.") }
                    }
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

/**
 * The head of an added playlist under its name: "12 tracks" and the owner's hint while the link works; once it ended,
 * "<owner> stopped sharing this playlist" and Remove from my playlists (no songs, nothing to copy).
 */
@Composable
private fun LinkedHead(pl: Playlist, count: Int, onRemove: () -> Unit) {
    val p = MaterialTheme.palette
    if (pl.linkedActive) {
        Text("$count tracks", style = MaterialTheme.typography.bodyMedium, color = p.muted, modifier = Modifier.padding(top = 4.dp))
        Text(
            PlaylistLinkRules.hint(pl.linkedOwner),
            style = MaterialTheme.typography.bodySmall,
            color = p.muted,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(top = 6.dp),
        )
    } else {
        Text(
            PlaylistLinkRules.stopped(pl.linkedOwner),
            style = MaterialTheme.typography.bodyMedium,
            color = p.muted,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(top = 4.dp),
        )
        EncoreButton(PlaylistLinkRules.REMOVE, modifier = Modifier.padding(top = 14.dp), iconRes = R.drawable.ic_delete, onClick = onRemove)
    }
}
