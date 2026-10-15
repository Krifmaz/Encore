package app.encore.android.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.Artist
import app.encore.android.data.api.LibrarySearch
import app.encore.android.data.api.Track
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.PageTitle
import app.encore.android.ui.components.RoundCover
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

private data class SearchResults(val query: String, val found: LibrarySearch) {
    val tracks: List<Track> get() = found.songs
    val albums: List<Album> get() = found.albums
    val artists: List<Artist> get() = found.artists
    val empty: Boolean get() = found.isEmpty
}

/**
 * Library search (GET /api/library/search): "travis scott 90210" in any order, typos and accents forgiven; a Top
 * result, then Songs (with their album), Albums and Artists over what you already have (Discover searches the
 * catalogue).
 */
@Composable
fun SearchScreen(nav: Nav) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val p = MaterialTheme.palette
    var query by remember { mutableStateOf("") }
    var results by remember { mutableStateOf<SearchResults?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var generation by remember { mutableStateOf(0) }

    LaunchedEffect(query, generation) {
        val q = query.trim()
        if (q.length < 2) {
            results = null
            error = null
            return@LaunchedEffect
        }
        delay(300)
        busy = true
        try {
            results = withContext(Dispatchers.IO) {
                SearchResults(q, api.search(q, 30))
            }
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Search failed"
        } finally {
            busy = false
        }
    }

    Column(Modifier.fillMaxSize()) {
        PageTitle("Search")
        // the web's `.search .input`
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            placeholder = { Text("Search artists, albums, songs…", color = p.faint) },
            singleLine = true,
            shape = RoundedCornerShape(12.dp),
            leadingIcon = { Icon(painterResource(R.drawable.ic_search), contentDescription = null, tint = p.faint) },
            trailingIcon = {
                if (query.isNotEmpty()) {
                    IconButton(onClick = { query = "" }) { Icon(painterResource(R.drawable.ic_close), contentDescription = "Clear", tint = p.muted) }
                }
            },
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            colors = OutlinedTextFieldDefaults.colors(
                focusedBorderColor = p.accent,
                unfocusedBorderColor = p.border,
                focusedContainerColor = p.bg2,
                unfocusedContainerColor = p.bg2,
                cursorColor = p.accent,
            ),
            modifier = Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, bottom = 8.dp),
        )
        val r = results
        val err = error
        when {
            busy && r == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(color = p.accent, trackColor = p.border)
            }
            err != null -> ErrorBox(err) { generation += 1 }
            r == null -> EmptyState(R.drawable.ic_search, "Search your library", "Artists, albums and songs you already have. Discover searches the whole catalogue.")
            r.empty -> EmptyState(R.drawable.ic_search, "No results for “${r.query}”")
            else -> LazyColumn(Modifier.fillMaxSize()) {
                val f = r.found
                if (f.topSong != null || f.topAlbum != null || f.topArtist != null) {
                    item { SectionHead("Top result") }
                    item { LibraryTopCard(nav, f) }
                }
                if (r.tracks.isNotEmpty()) {
                    item { SectionHead("Songs") }
                    itemsIndexed(r.tracks, key = { _, t -> "track:" + t.id }) { i, t ->
                        TrackRow(
                            track = t,
                            imageLoader = nav.graph.imageLoader,
                            coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                            liked = t.id in liked,
                            playing = nowPlaying?.id == t.id,
                            onClick = { nav.player.play(r.tracks, i) },
                            onLike = { nav.graph.likes.toggle(t.id) },
                            subtitle = listOf(t.artist, t.album).filter { it.isNotEmpty() }.joinToString(" · "),
                            onMore = { nav.trackMenu(t) },
                        )
                    }
                }
                if (r.albums.isNotEmpty()) {
                    item { SectionHead("Albums") }
                    item {
                        LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                            items(r.albums, key = { it.id }) { a ->
                                AlbumCard(a, api.coverUrl(a.id, 400), nav.graph.imageLoader, Modifier.width(140.dp)) { nav.open(Screen.Album(a.id)) }
                            }
                        }
                    }
                }
                if (r.artists.isNotEmpty()) {
                    item { SectionHead("Artists") }
                    items(r.artists, key = { "artist:" + it.name }) { a ->
                        Row(
                            Modifier.fillMaxWidth().clickable { nav.open(Screen.Artist(a.name)) }.padding(horizontal = Gutter, vertical = 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            RoundCover(if (a.coverAlbum.isNotEmpty()) api.coverUrl(a.coverAlbum, 96) else null, nav.graph.imageLoader, size = 48.dp, label = a.name)
                            Spacer(Modifier.width(14.dp))
                            Column(Modifier.weight(1f)) {
                                Text(a.name, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text(
                                    "${a.albums} album${if (a.albums == 1) "" else "s"} · ${a.tracks} tracks",
                                    style = MaterialTheme.typography.bodySmall,
                                    color = p.muted,
                                )
                            }
                        }
                    }
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
}

/** The library's Top result: a big cover, the kind, the name, "artist · on album" for songs, and Play. */
@Composable
private fun LibraryTopCard(nav: Nav, f: LibrarySearch) {
    val api = nav.graph.api
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val t = f.topSong
    val a = f.topAlbum
    val ar = f.topArtist
    val kicker = when { t != null -> "Song"; a != null -> "Album"; else -> "Artist" }
    val title = t?.title ?: a?.title ?: ar?.name ?: ""
    val meta = when {
        t != null -> listOf(t.artist, if (t.album.isNotEmpty()) "on ${t.album}" else "").filter { it.isNotEmpty() }.joinToString(" · ")
        a != null -> listOf(a.artist, a.year).filter { it.isNotEmpty() }.joinToString(" · ")
        ar != null -> "${ar.albums} album${if (ar.albums == 1) "" else "s"} · ${ar.tracks} tracks"
        else -> ""
    }
    val open: () -> Unit = {
        when {
            t != null -> if (t.albumId.isNotEmpty()) nav.open(Screen.Album(t.albumId))
            a != null -> nav.open(Screen.Album(a.id))
            ar != null -> nav.open(Screen.Artist(ar.name))
        }
    }
    CardBox(Modifier.padding(horizontal = Gutter).clickable(onClick = open)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            when {
                ar != null -> RoundCover(if (ar.coverAlbum.isNotEmpty()) api.coverUrl(ar.coverAlbum, 400) else null, nav.graph.imageLoader, size = 96.dp, label = ar.name)
                else -> {
                    val albumId = t?.albumId ?: a?.id ?: ""
                    Cover(if (albumId.isNotEmpty()) api.coverUrl(albumId, 400) else null, nav.graph.imageLoader, size = 96.dp, corner = 12.dp, label = t?.album ?: title)
                }
            }
            Spacer(Modifier.width(14.dp))
            Column(Modifier.weight(1f)) {
                Text(kicker.uppercase(), style = MaterialTheme.typography.labelSmall, color = p.muted, fontWeight = FontWeight.SemiBold)
                Text(title, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (meta.isNotEmpty()) {
                    Text(meta, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
                Row(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    when {
                        t != null -> SmallButton("Play", iconRes = R.drawable.ic_play, primary = true) {
                            val i = f.songs.indexOfFirst { it.id == t.id }
                            if (i >= 0) nav.player.play(f.songs, i) else nav.player.play(listOf(t))
                        }
                        a != null -> SmallButton("Play", iconRes = R.drawable.ic_play, primary = true) { playLibraryAlbum(nav, a.id, scope) }
                        ar != null -> SmallButton("Open artist", iconRes = R.drawable.ic_person, primary = true) { nav.open(Screen.Artist(ar.name)) }
                    }
                }
            }
        }
    }
}
