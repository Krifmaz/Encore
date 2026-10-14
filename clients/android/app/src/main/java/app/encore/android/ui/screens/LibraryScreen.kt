package app.encore.android.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import app.encore.android.ui.components.FastScroller
import app.encore.android.ui.components.rememberLibraryIndex
import app.encore.android.ui.components.rememberScrollerTarget
import kotlinx.coroutines.CoroutineScope
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
 * A list the server pages with limit/offset, by position: rows not loaded yet are null (shown as placeholders),
 * so a jump from the scrubber loads only the page that holds the row, and the pages before it load when they
 * come into view. Pages are fixed blocks of [pageSize] rows; each loads once.
 */
private class Paged<T>(val pageSize: Int) {
    val rows = mutableStateListOf<T?>()
    /** the end of the list has been seen */
    var done by mutableStateOf(false)
    var loading by mutableStateOf(false)
    var error by mutableStateOf<String?>(null)
    var generation by mutableStateOf(0)
    /** set by the screen on every composition: how to fetch a page, and where to run it */
    var fetch: (offset: Int, limit: Int) -> List<T> = { _, _ -> emptyList() }
    var scope: CoroutineScope? = null
    private val loaded = HashSet<Int>()
    private val inFlight = HashSet<Int>()

    fun reset() {
        rows.clear()
        loaded.clear()
        inFlight.clear()
        done = false
        loading = false
        error = null
        generation += 1
    }

    fun retry() {
        inFlight.clear()
        loading = false
        error = null
        generation += 1
    }

    /** Loads the page that holds row [index] (once). */
    fun want(index: Int) {
        val page = index / pageSize
        val start = page * pageSize
        if (page in loaded || page in inFlight || error != null) return
        if (done && start >= rows.size) return
        val sc = scope ?: return
        val gen = generation
        val f = fetch
        inFlight.add(page)
        loading = true
        sc.launch {
            try {
                val more = withContext(Dispatchers.IO) { f(start, pageSize) }
                if (gen != generation) return@launch
                while (rows.size < start) rows.add(null)
                more.forEachIndexed { i, t -> if (start + i < rows.size) rows[start + i] = t else rows.add(t) }
                loaded.add(page)
                if (more.size < pageSize) {
                    done = true
                    // placeholders past the real end (the library shrank since the index was read)
                    while (rows.size > start + more.size) rows.removeAt(rows.lastIndex)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (gen == generation) error = e.message ?: "Couldn't load more"
            } finally {
                if (gen == generation) {
                    inFlight.remove(page)
                    loading = inFlight.isNotEmpty()
                }
            }
        }
    }

    /** A jump from the scrubber: placeholders up to row [offset], then the page that holds it. */
    fun jumpTo(offset: Int) {
        if (done && offset >= rows.size) return
        while (rows.size <= offset) rows.add(null)
        want(offset)
    }

    /** The loaded rows around [index] (up to the nearest placeholders), and where [index] is in them. */
    fun loadedRun(index: Int): Pair<List<T>, Int> {
        var a = index
        while (a > 0 && rows[a - 1] != null) a--
        var b = index
        while (b + 1 < rows.size && rows[b + 1] != null) b++
        return rows.subList(a, b + 1).filterNotNull() to (index - a)
    }
}

/** The next page at the end of the list (when it comes into view), or the error with a retry. */
@Composable
private fun <T> PageLoader(paged: Paged<T>) {
    LaunchedEffect(paged.generation, paged.rows.size) { paged.want(paged.rows.size) }
    Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
        val err = paged.error
        when {
            err != null -> TextButton(onClick = { paged.retry() }) { Text("$err – try again", color = MaterialTheme.palette.text) }
            paged.loading -> CircularProgressIndicator(color = MaterialTheme.palette.accent, trackColor = MaterialTheme.palette.border)
        }
    }
}

/** A row not loaded yet: asks for its page while it is on screen. */
@Composable
private fun <T> Placeholder(paged: Paged<T>, index: Int, content: @Composable () -> Unit) {
    LaunchedEffect(index / paged.pageSize, paged.generation) { paged.want(index) }
    if (paged.error != null) {
        TextButton(onClick = { paged.retry() }) { Text("Try again", color = MaterialTheme.palette.text) }
    } else {
        content()
    }
}

/** An album tile while it loads: the cover square and two text lines in the panel color. */
@Composable
private fun AlbumPlaceholder() {
    val p = MaterialTheme.palette
    Column {
        Box(Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(12.dp)).background(p.panel2))
        Spacer(Modifier.height(9.dp))
        Box(Modifier.fillMaxWidth(0.8f).height(14.dp).clip(RoundedCornerShape(4.dp)).background(p.panel))
        Spacer(Modifier.height(5.dp))
        Box(Modifier.fillMaxWidth(0.5f).height(12.dp).clip(RoundedCornerShape(4.dp)).background(p.panel))
    }
}

/** A song row while it loads (the size of a [TrackRow]). */
@Composable
private fun TrackPlaceholder() {
    val p = MaterialTheme.palette
    Row(Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(48.dp).clip(RoundedCornerShape(8.dp)).background(p.panel2))
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Box(Modifier.fillMaxWidth(0.6f).height(14.dp).clip(RoundedCornerShape(4.dp)).background(p.panel))
            Spacer(Modifier.height(6.dp))
            Box(Modifier.fillMaxWidth(0.4f).height(12.dp).clip(RoundedCornerShape(4.dp)).background(p.panel))
        }
    }
}

@Composable
private fun AlbumsTab(nav: Nav) {
    val api = nav.graph.api
    var sort by remember { mutableStateOf(0) }
    val key = ALBUM_SORTS[sort].first
    val scope = rememberCoroutineScope()
    val paged = remember { Paged<Album>(120) }
    paged.scope = scope
    paged.fetch = { offset, limit -> api.albums(sort = key, offset = offset, limit = limit) }
    val grid = rememberLazyGridState()
    val sections = rememberLibraryIndex(api, "albums", key)
    Column(Modifier.fillMaxSize()) {
        ChoiceRow(ALBUM_SORTS.map { it.second }, sort, {
            sort = it
            paged.reset()
            scope.launch { grid.scrollToItem(0) }
        })
        if (paged.done && paged.rows.isEmpty()) {
            EmptyState(R.drawable.ic_library, "No albums found")
            return@Column
        }
        Box(Modifier.fillMaxSize()) {
            LazyVerticalGrid(
                columns = GridCells.Adaptive(140.dp),
                state = grid,
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = Gutter, end = Gutter, top = 4.dp, bottom = 24.dp),
                horizontalArrangement = Arrangement.spacedBy(14.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                items(paged.rows.size) { i ->
                    val a = paged.rows[i]
                    if (a == null) {
                        Placeholder(paged, i) { AlbumPlaceholder() }
                    } else {
                        AlbumCard(a, api.coverUrl(a.id, 400), nav.graph.imageLoader) { nav.open(Screen.Album(a.id)) }
                    }
                }
                if (!paged.done) {
                    item(span = { GridItemSpan(maxLineSpan) }) { PageLoader(paged) }
                }
            }
            FastScroller(
                sections,
                rememberScrollerTarget(grid),
                extent = paged.rows.size,
                modifier = Modifier.align(Alignment.CenterEnd),
                onJump = { paged.jumpTo(it) },
            )
        }
    }
}

@Composable
private fun ArtistsTab(nav: Nav) {
    val api = nav.graph.api
    val loader = rememberLoad("artists") { api.artists() }
    val grid = rememberLazyGridState()
    val sections = rememberLibraryIndex(api, "artists", "name")
    LoadableContent(loader) { artists ->
        if (artists.isEmpty()) {
            EmptyState(R.drawable.ic_person, "No artists found")
            return@LoadableContent
        }
        Box(Modifier.fillMaxSize()) {
            LazyVerticalGrid(
                columns = GridCells.Adaptive(140.dp),
                state = grid,
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
            // the list and the index both come from the artist list; sections past its end (it changed since) are left out
            FastScroller(
                sections.filter { it.offset < artists.size },
                rememberScrollerTarget(grid),
                extent = artists.size,
                modifier = Modifier.align(Alignment.CenterEnd),
            )
        }
    }
}

@Composable
private fun SongsTab(nav: Nav) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val scope = rememberCoroutineScope()
    val paged = remember { Paged<Track>(200) }
    paged.scope = scope
    paged.fetch = { offset, limit -> api.tracks(offset = offset, limit = limit) }
    val list = rememberLazyListState()
    // the Songs tab lists by artist (api.tracks' default sort)
    val sections = rememberLibraryIndex(api, "tracks", "artist")
    if (paged.done && paged.rows.isEmpty()) {
        EmptyState(R.drawable.ic_note, "No songs found")
        return
    }
    Box(Modifier.fillMaxSize()) {
        LazyColumn(Modifier.fillMaxSize(), state = list) {
            items(paged.rows.size) { i ->
                val t = paged.rows[i]
                if (t == null) {
                    Placeholder(paged, i) { TrackPlaceholder() }
                } else {
                    TrackRow(
                        track = t,
                        imageLoader = nav.graph.imageLoader,
                        coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                        liked = t.id in liked,
                        playing = nowPlaying?.id == t.id,
                        onClick = {
                            // the loaded songs around it (after a jump, rows above can be not loaded yet)
                            val (queue, at) = paged.loadedRun(i)
                            nav.player.play(queue, at)
                        },
                        onLike = { nav.graph.likes.toggle(t.id) },
                        subtitle = listOf(t.artist, t.album).filter { it.isNotEmpty() }.joinToString(" · "),
                        onMore = { nav.trackMenu(t) },
                    )
                }
            }
            if (!paged.done) {
                item { PageLoader(paged) }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
        FastScroller(
            sections,
            rememberScrollerTarget(list),
            extent = paged.rows.size,
            modifier = Modifier.align(Alignment.CenterEnd),
            onJump = { paged.jumpTo(it) },
        )
    }
}
