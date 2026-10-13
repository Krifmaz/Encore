package app.encore.android.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.Track
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Tile
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** web/js/pages/library.js SORT_OPTS.albums: the server's key and the label. */
private val ALBUM_SORTS = listOf("added" to "Date added", "released" to "Release date", "album" to "Title A–Z", "artist" to "Artist A–Z", "duration" to "Length", "size" to "Size")

/** Library: Albums / Artists / Songs (the web's #/library tabs). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryScreen(nav: Nav) {
    var tab by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = MaterialTheme.palette.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            EncoreTopBar("Library", scrollBehavior = scrollBehavior, actions = {
                SmallButton("Shuffle", iconRes = R.drawable.ic_shuffle, primary = true, modifier = Modifier.padding(end = Gutter)) {
                    scope.launch {
                        val songs = try { withContext(Dispatchers.IO) { nav.graph.api.shuffle(50, emptyList()) } } catch (_: Exception) { emptyList() }
                        if (songs.isEmpty()) Toasts.show("Your library is empty", "err") else nav.player.play(songs)
                    }
                }
            })
        },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(padding)) {
            ChoiceRow(listOf("Albums", "Artists", "Songs"), tab, { tab = it })
            when (tab) {
                0 -> AlbumsTab(nav)
                1 -> ArtistsTab(nav)
                else -> SongsTab(nav)
            }
        }
    }
}

/**
 * A list the server pages with limit/offset: loads the next page when its end comes into view. Rows already
 * shown are dropped from later pages (the library can change between pages, and Compose keys must be unique).
 */
private class Paged<T>(val pageSize: Int, val keyOf: (T) -> Any) {
    val items = mutableStateListOf<T>()
    val seen = HashSet<Any>()
    /** rows the server has sent (the next offset); can be more than [items] after duplicates were dropped */
    var fetched by mutableStateOf(0)
    var done by mutableStateOf(false)
    var loading by mutableStateOf(false)
    var error by mutableStateOf<String?>(null)
    var generation by mutableStateOf(0)

    fun reset() {
        items.clear()
        seen.clear()
        fetched = 0
        done = false
        loading = false
        error = null
        generation += 1
    }
}

@Composable
private fun <T> PageLoader(paged: Paged<T>, fetch: (offset: Int, limit: Int) -> List<T>) {
    LaunchedEffect(paged.generation, paged.fetched) {
        if (paged.done || paged.loading || paged.error != null) return@LaunchedEffect
        paged.loading = true
        try {
            val more = withContext(Dispatchers.IO) { fetch(paged.fetched, paged.pageSize) }
            paged.items.addAll(more.filter { paged.seen.add(paged.keyOf(it)) })
            if (more.size < paged.pageSize) paged.done = true
            paged.fetched += more.size
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            paged.error = e.message ?: "Couldn't load more"
        } finally {
            paged.loading = false
        }
    }
    Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
        val err = paged.error
        when {
            err != null -> TextButton(onClick = { paged.error = null; paged.generation += 1 }) { Text("$err – try again", color = MaterialTheme.palette.text) }
            paged.loading -> CircularProgressIndicator(color = MaterialTheme.palette.accent, trackColor = MaterialTheme.palette.border)
        }
    }
}

@Composable
private fun AlbumsTab(nav: Nav) {
    val api = nav.graph.api
    var sort by remember { mutableStateOf(0) }
    val paged = remember { Paged<Album>(120) { it.id } }
    Column(Modifier.fillMaxSize()) {
        ChoiceRow(ALBUM_SORTS.map { it.second }, sort, { sort = it; paged.reset() })
        if (paged.done && paged.items.isEmpty()) {
            EmptyState(R.drawable.ic_library, "No albums found")
            return@Column
        }
        LazyVerticalGrid(
            columns = GridCells.Adaptive(140.dp),
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = Gutter, end = Gutter, top = 4.dp, bottom = 24.dp),
            horizontalArrangement = Arrangement.spacedBy(14.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            items(paged.items, key = { it.id }) { a ->
                AlbumCard(a, api.coverUrl(a.id, 400), nav.graph.imageLoader) { nav.open(Screen.Album(a.id)) }
            }
            if (!paged.done) {
                item(span = { GridItemSpan(maxLineSpan) }) {
                    val key = ALBUM_SORTS[sort].first
                    PageLoader(paged) { offset, limit -> api.albums(sort = key, offset = offset, limit = limit) }
                }
            }
        }
    }
}

@Composable
private fun ArtistsTab(nav: Nav) {
    val api = nav.graph.api
    val loader = rememberLoad("artists") { api.artists() }
    LoadableContent(loader) { artists ->
        if (artists.isEmpty()) {
            EmptyState(R.drawable.ic_person, "No artists found")
            return@LoadableContent
        }
        LazyVerticalGrid(
            columns = GridCells.Adaptive(140.dp),
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(start = Gutter, end = Gutter, top = 4.dp, bottom = 24.dp),
            horizontalArrangement = Arrangement.spacedBy(14.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            items(artists, key = { it.name }) { a ->
                // the web's `.tile.round`
                Tile(
                    title = a.name,
                    subtitle = "${a.albums} album${if (a.albums == 1) "" else "s"} · ${a.tracks} tracks",
                    centered = true,
                    onClick = { nav.open(Screen.Artist(a.name)) },
                ) { m ->
                    Cover(
                        if (a.coverAlbum.isNotEmpty()) api.coverUrl(a.coverAlbum, 400) else null,
                        nav.graph.imageLoader,
                        m,
                        corner = 999.dp,
                        label = a.name,
                    )
                }
            }
        }
    }
}

@Composable
private fun SongsTab(nav: Nav) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val paged = remember { Paged<Track>(200) { it.id } }
    if (paged.done && paged.items.isEmpty()) {
        EmptyState(R.drawable.ic_note, "No songs found")
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        itemsIndexed(paged.items, key = { _, t -> t.id }) { i, t ->
            TrackRow(
                track = t,
                imageLoader = nav.graph.imageLoader,
                coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                liked = t.id in liked,
                playing = nowPlaying?.id == t.id,
                onClick = { nav.player.play(paged.items.toList(), i) },
                onLike = { nav.graph.likes.toggle(t.id) },
                subtitle = listOf(t.artist, t.album).filter { it.isNotEmpty() }.joinToString(" · "),
                onMore = { nav.trackMenu(t) },
            )
        }
        if (!paged.done) {
            item { PageLoader(paged) { offset, limit -> api.tracks(offset = offset, limit = limit) } }
        }
        item { Spacer(Modifier.height(24.dp)) }
    }
}
