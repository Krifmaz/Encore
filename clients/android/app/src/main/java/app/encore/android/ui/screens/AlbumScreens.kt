package app.encore.android.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.itemsIndexed
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
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.AlbumDetail
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.FastScroller
import app.encore.android.ui.components.rememberLibraryIndex
import app.encore.android.ui.components.rememberScrollerTarget
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.Loadable
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.RoundCover
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.formatDay
import app.encore.android.ui.components.formatLongDuration
import app.encore.android.ui.components.formatSize
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.components.shareLinkOf
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/** A back arrow and a title, for pages stacked on a tab (kept for screens that don't use a scrolling top bar). */
@Composable
fun BackBar(title: String, onBack: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(top = 4.dp, end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onBack) { Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back") }
        Text(title, style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** Play and Shuffle for a list of songs (the web's `.btn.primary` Play and `.btn` Shuffle). */
@Composable
fun PlayButtons(nav: Nav, tracks: List<Track>, modifier: Modifier = Modifier, trailing: @Composable () -> Unit = {}) {
    Row(modifier.padding(horizontal = Gutter, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        EncoreButton("Play", iconRes = R.drawable.ic_play, primary = true, enabled = tracks.isNotEmpty()) { nav.player.play(tracks) }
        EncoreButton("Shuffle", iconRes = R.drawable.ic_shuffle, enabled = tracks.isNotEmpty()) { nav.player.play(tracks, shuffle = true) }
        trailing()
    }
}

// ---- album -----------------------------------------------------------------------------------------------------

/** What the album page shows beyond [AlbumDetail]: size, the history line and per-track "requested by" notes. */
private class AlbumPage(val detail: AlbumDetail, val size: Long, val history: String, val notes: Map<Long, String>)

private fun JsonObject.s(key: String): String {
    val v = this[key] as? JsonPrimitive ?: return ""
    return if (v.isString) v.content else v.content.takeIf { it != "null" } ?: ""
}

private fun JsonObject.d(key: String): Double = (this[key] as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0

/**
 * GET /api/library/albums/{id} (server/src/routes/library.rs `album`): the rows carry `requested_by`,
 * `requested_at` and `added_at`, which web/js/pages/library.js turns into the history line.
 */
private fun loadAlbumPage(nav: Nav, id: String): AlbumPage {
    val o = nav.graph.api.getObject("/api/library/albums/${EncoreApi.enc(id)}")
    val rows = (o["tracks"] as? JsonArray)?.mapNotNull { it as? JsonObject } ?: emptyList()
    val tracks = rows.mapNotNull { Track.from(it) }
    val album = Album(
        id = id,
        title = o.s("album").ifEmpty { "Unknown album" },
        artist = o.s("albumartist"),
        year = o.s("year").take(4),
        trackCount = tracks.size,
        formats = tracks.map { it.format }.distinct().joinToString(","),
    )
    val detail = AlbumDetail(album, tracks, o.d("duration"))

    val requesters = rows.map { it.s("requested_by") }.filter { it.isNotEmpty() }.distinct().joinToString(", ")
    val requestedAt = rows.map { it.d("requested_at") }.filter { it > 0 }.minOrNull()
    val addedAt = rows.map { it.d("added_at") }.filter { it > 0 }.minOrNull()
    val history = listOf(
        if (requesters.isNotEmpty()) "Requested by $requesters" + (if (requestedAt != null) " on ${formatDay(requestedAt)}" else "") else "",
        if (addedAt != null) (if (requesters.isNotEmpty()) "added " else "Added ") + formatDay(addedAt) else "",
    ).filter { it.isNotEmpty() }.joinToString(" · ")

    val notes = HashMap<Long, String>()
    for (r in rows) {
        val who = r.s("requested_by")
        if (who.isEmpty()) continue
        val at = r.d("requested_at")
        val id2 = r.d("id").toLong()
        notes[id2] = "requested by $who" + if (at > 0) " on ${formatDay(at)}" else ""
    }
    return AlbumPage(detail, o.d("size").toLong(), history, notes)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AlbumScreen(nav: Nav, albumId: String) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val loader = rememberLoad(albumId) { loadAlbumPage(nav, albumId) }
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    // the album's public link, like the player's "Share link" for a song
    fun shareAlbum(title: String, artist: String) {
        shareLinkOf(
            context, scope,
            subject = title,
            text = listOf(title, artist).filter { it.isNotEmpty() }.joinToString(" – "),
            toast = "Link copied – anyone with it can play this album",
        ) { api.shareAlbumLink(albumId) }
    }
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            EncoreTopBar("", scrollBehavior = scrollBehavior, onBack = nav.back) {
                IconButton(onClick = {
                    val a = (loader.state.value as? Loadable.Ready)?.value?.detail?.album
                    shareAlbum(a?.title ?: "", a?.artist ?: "")
                }) {
                    Icon(painterResource(R.drawable.ic_share), contentDescription = "Share link")
                }
            }
        },
    ) { padding ->
        LoadableContent(loader) { page ->
            val detail = page.detail
            val album = detail.album
            val tracks = detail.tracks
            val multiDisc = tracks.map { it.disc }.distinct().size > 1
            val formats = tracks.map { it.format.uppercase() }.filter { it.isNotEmpty() }.distinct().joinToString(" / ")
            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                // .detail-head
                item {
                    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter), horizontalAlignment = Alignment.CenterHorizontally) {
                        Box(Modifier.size(220.dp).shadow(24.dp, RoundedCornerShape(16.dp), clip = false)) {
                            Cover(api.coverUrl(albumId, 1000), nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 16.dp, label = album.title)
                        }
                        Spacer(Modifier.height(18.dp))
                        Text("ALBUM · IN YOUR LIBRARY", style = MaterialTheme.typography.labelSmall, color = p.muted, letterSpacing = 1.2.sp)
                        Text(
                            album.title,
                            style = MaterialTheme.typography.headlineMedium,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(top = 6.dp),
                        )
                        Text(
                            album.artist,
                            style = MaterialTheme.typography.bodyLarge,
                            fontWeight = FontWeight.SemiBold,
                            color = p.text,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(top = 6.dp).clickable { if (album.artist.isNotEmpty()) nav.open(Screen.Artist(album.artist)) },
                        )
                        Text(
                            listOf(
                                album.year,
                                "${tracks.size} tracks",
                                formatLongDuration(detail.durationSec),
                                if (page.size > 0) formatSize(page.size) else "",
                            ).filter { it.isNotEmpty() }.joinToString(" · "),
                            style = MaterialTheme.typography.bodyMedium,
                            color = p.muted,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(top = 2.dp),
                        )
                        if (page.history.isNotEmpty()) {
                            Text(page.history, style = MaterialTheme.typography.bodySmall, color = p.muted, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 4.dp))
                        }
                        if (formats.isNotEmpty()) {
                            Row(Modifier.padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) { Badge(formats) }
                        }
                    }
                }
                item {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                        PlayButtons(nav, tracks) {
                            MoreButton(
                                entries = listOf(
                                    MenuEntry("Add to queue", R.drawable.ic_queue_music) { tracks.forEach { nav.player.addToQueue(it) }; Toasts.show("Added to queue", "ok") },
                                    MenuEntry("Play next", R.drawable.ic_skip_next) { tracks.asReversed().forEach { nav.player.playNext(it) }; Toasts.show("Plays next", "ok") },
                                    MenuEntry("Add to playlist", R.drawable.ic_playlist_add) { nav.addToPlaylist(tracks.map { it.id }) },
                                    MenuEntry("Share link", R.drawable.ic_share) { shareAlbum(album.title, album.artist) },
                                ),
                            )
                        }
                    }
                }
                // the track list, with a header per disc when there is more than one
                itemsIndexed(tracks, key = { _, t -> t.id }) { i, t ->
                    if (multiDisc && (i == 0 || tracks[i - 1].disc != t.disc)) {
                        Text(
                            "Disc ${t.disc}",
                            style = MaterialTheme.typography.labelMedium,
                            color = p.faint,
                            modifier = Modifier.padding(start = Gutter, end = Gutter, top = 14.dp, bottom = 4.dp),
                        )
                    }
                    val fmt = (t.format.uppercase() + if (t.bitrate > 0) " ${t.bitrate}k" else "").trim()
                    TrackRow(
                        track = t,
                        imageLoader = null,
                        coverUrl = null,
                        liked = t.id in liked,
                        playing = nowPlaying?.id == t.id,
                        onClick = { nav.player.play(tracks, i) },
                        onLike = { nav.graph.likes.toggle(t.id) },
                        number = t.trackNo,
                        subtitle = if (t.artist.isNotEmpty() && t.artist != album.artist) "${t.artist} · $fmt" else fmt,
                        note = page.notes[t.id],
                        onMore = { nav.trackMenu(t, onAlbumPage = true) },
                    )
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
}

// ---- artist ----------------------------------------------------------------------------------------------------

/** An artist in the library: a round portrait, the counts and their albums (the web's artist page header + grid). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ArtistScreen(nav: Nav, name: String) {
    val api = nav.graph.api
    val loader = rememberLoad(name) { api.albums(sort = "released", artist = name, limit = 200) }
    val grid = rememberLazyGridState()
    val sections = rememberLibraryIndex(api, "albums", "released", artist = name)
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    // GET /api/library/artist-tracks?name= (every spelling, alias and collaboration of this artist), then play
    fun playAll(shuffle: Boolean) {
        scope.launch {
            val songs = try { withContext(Dispatchers.IO) { api.artistTracks(name) } } catch (e: Exception) { Toasts.show(e.message ?: "Couldn't load the songs", "err"); return@launch }
            if (songs.isEmpty()) Toasts.show("Nothing by this artist in your library", "err") else nav.player.play(songs, shuffle = shuffle)
        }
    }
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            EncoreTopBar(name, scrollBehavior = scrollBehavior, onBack = nav.back) {
                IconButton(onClick = {
                    shareLinkOf(
                        context, scope,
                        subject = name,
                        text = name,
                        toast = "Link copied – anyone with it can play this artist",
                    ) { api.shareArtistLink(name) }
                }) {
                    Icon(painterResource(R.drawable.ic_share), contentDescription = "Share link")
                }
            }
        },
    ) { padding ->
        LoadableContent(loader) { albums ->
            val songs = albums.sumOf { it.trackCount }
            Box(Modifier.fillMaxSize().padding(padding)) {
                LazyVerticalGrid(
                    columns = GridCells.Adaptive(140.dp),
                    state = grid,
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(start = Gutter, end = Gutter, bottom = 24.dp),
                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                    verticalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    item(span = { GridItemSpan(maxLineSpan) }) {
                        Column(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                            RoundCover(
                                albums.firstOrNull()?.let { api.coverUrl(it.id, 400) },
                                nav.graph.imageLoader,
                                size = 140.dp,
                                label = name,
                            )
                            Spacer(Modifier.height(14.dp))
                            Text("ARTIST · IN YOUR LIBRARY", style = MaterialTheme.typography.labelSmall, color = p.muted, letterSpacing = 1.2.sp)
                            Text(name, style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 6.dp))
                            Text(
                                "${albums.size} album${if (albums.size == 1) "" else "s"} · $songs tracks",
                                style = MaterialTheme.typography.bodyMedium,
                                color = p.muted,
                                modifier = Modifier.padding(top = 2.dp),
                            )
                            // the catalog artist page's Play / Shuffle
                            Row(Modifier.padding(top = 14.dp, bottom = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                EncoreButton("Play", iconRes = R.drawable.ic_play, primary = true) { playAll(false) }
                                EncoreButton("Shuffle", iconRes = R.drawable.ic_shuffle) { playAll(true) }
                            }
                        }
                    }
                    items(albums, key = { it.id }) { a ->
                        AlbumCard(a, api.coverUrl(a.id, 400), nav.graph.imageLoader) { nav.open(Screen.Album(a.id)) }
                    }
                }
                // years, newest first (the header is item 0)
                FastScroller(
                    sections.filter { it.offset < albums.size },
                    rememberScrollerTarget(grid, header = 1),
                    extent = albums.size,
                    modifier = Modifier.align(Alignment.CenterEnd),
                )
            }
        }
    }
}
