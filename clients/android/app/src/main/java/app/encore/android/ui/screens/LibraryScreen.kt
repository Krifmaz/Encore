package app.encore.android.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.data.api.Album
import app.encore.android.data.api.Track
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.rememberLoad
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private val ALBUM_SORTS = listOf("added" to "Recently added", "artist" to "Artist", "name" to "Title", "year" to "Year")

/** Library: Albums / Artists / Songs (the web's #/library tabs). */
@Composable
fun LibraryScreen(nav: Nav) {
    var tab by remember { mutableStateOf(0) }
    Column(Modifier.fillMaxSize()) {
        Text(
            "Library",
            style = MaterialTheme.typography.headlineSmall,
            modifier = Modifier.padding(start = 16.dp, top = 16.dp, end = 16.dp),
        )
        ChoiceRow(listOf("Albums", "Artists", "Songs"), tab, { tab = it })
        when (tab) {
            0 -> AlbumsTab(nav)
            1 -> ArtistsTab(nav)
            else -> SongsTab(nav)
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
            err != null -> TextButton(onClick = { paged.error = null; paged.generation += 1 }) { Text("$err – try again") }
            paged.loading -> CircularProgressIndicator()
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
        LazyVerticalGrid(
            columns = GridCells.Adaptive(150.dp),
            modifier = Modifier.fillMaxSize(),
            contentPadding = PaddingValues(8.dp),
        ) {
            items(paged.items, key = { it.id }) { a ->
                AlbumCard(a, api.coverUrl(a.id, 300), nav.graph.imageLoader) { nav.open(Screen.Album(a.id)) }
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
        LazyColumn(Modifier.fillMaxSize()) {
            items(artists, key = { it.name }) { a ->
                Row(
                    Modifier.fillMaxWidth().clickable { nav.open(Screen.Artist(a.name)) }.padding(horizontal = 16.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Cover(
                        if (a.coverAlbum.isNotEmpty()) api.coverUrl(a.coverAlbum, 96) else null,
                        nav.graph.imageLoader,
                        size = 48.dp,
                        corner = 24.dp,
                    )
                    Spacer(Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Text(a.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            "${a.albums} album${if (a.albums == 1) "" else "s"} · ${a.tracks} song${if (a.tracks == 1) "" else "s"}",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun SongsTab(nav: Nav) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val playerState by nav.player.state.collectAsState()
    val paged = remember { Paged<Track>(200) { it.id } }
    LazyColumn(Modifier.fillMaxSize()) {
        itemsIndexed(paged.items, key = { _, t -> t.id }) { i, t ->
            TrackRow(
                track = t,
                imageLoader = nav.graph.imageLoader,
                coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                liked = t.id in liked,
                playing = playerState.current?.id == t.id,
                onClick = { nav.player.play(paged.items.toList(), i) },
                onLike = { nav.graph.likes.toggle(t.id) },
            )
        }
        if (!paged.done) {
            item { PageLoader(paged) { offset, limit -> api.tracks(offset = offset, limit = limit) } }
        }
    }
}
