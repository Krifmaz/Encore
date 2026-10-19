package app.encore.android.ui.screens

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items as rowItems
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import app.encore.android.PreloadCoversAhead
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.LibrarySearch
import app.encore.android.data.api.Track
import app.encore.android.features.library.OnSongFixed
import app.encore.android.features.library.SongFixUi
import app.encore.android.features.library.WrongAudioList
import app.encore.android.features.offline.DownloadedList
import app.encore.android.features.offline.OfflineLibraryScreen
import app.encore.android.features.offline.OfflineMode
import app.encore.android.features.offline.OfflineStore
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MenuSeparator
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Tile
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * web/js/pages/library.js SORT_OPTS: per tab, the server's sort key and its label (the first is the default).
 * The chip beside the tabs shows the label's [CHIP] form.
 */
private val SORTS = mapOf(
    "albums" to listOf("added" to "Date added", "released" to "Release date", "album" to "Title A–Z", "artist" to "Artist A–Z", "duration" to "Length", "size" to "Size"),
    "artists" to listOf("name" to "Name A–Z", "added" to "Date added", "albums" to "Most albums", "tracks" to "Most songs"),
    "tracks" to listOf("added" to "Date added", "released" to "Release date", "title" to "Title A–Z", "artist" to "Artist A–Z", "album" to "Album A–Z", "duration" to "Length"),
)

/** web/js/pages/library.js DESC_FIRST: sorts that start newest / longest / biggest first. */
private val DESC_FIRST = setOf("added", "released", "duration", "size", "albums", "tracks")

/** The chip's short words for a sort's label (the menu shows the whole label). */
private val CHIP = mapOf(
    "Date added" to "Added", "Release date" to "Released", "Title A–Z" to "Title", "Artist A–Z" to "Artist",
    "Album A–Z" to "Album", "Name A–Z" to "Name", "Most albums" to "Albums", "Most songs" to "Songs",
)

private val TABS = listOf("albums" to "Albums", "artists" to "Artists", "tracks" to "Songs")

/**
 * The Library page's search and tab, kept while the app runs so going to an album and back (or to another tab)
 * finds them as they were (the web keeps the query in the address, `#/library?q=`).
 */
private object LibraryState {
    /** what is typed in the search box */
    var text by mutableStateOf("")
    /** with a query: the one results view (true), or the selected tab filtered by the query (false) */
    var results by mutableStateOf(true)
    var tab by mutableStateOf(0)
}

/** The first matches each section of the results view shows. */
private const val RESULTS_LIMIT = 12

/** A tab's sort: the server's key and "asc" / "desc". */
private data class LibSort(val key: String, val dir: String)

private fun defaultSort(tab: String): LibSort {
    val key = SORTS.getValue(tab).first().first
    return LibSort(key, if (key in DESC_FIRST) "desc" else "asc")
}

/** The sort each tab had last, kept on this phone (like the web's `encore.libSort`). */
private class LibrarySorts(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences("encore_library", Context.MODE_PRIVATE)

    fun get(tab: String): LibSort {
        val saved = try { prefs.getString("sort.$tab", null) } catch (_: Exception) { null } ?: return defaultSort(tab)
        val key = saved.substringBefore(':')
        val dir = saved.substringAfter(':', "")
        if (SORTS.getValue(tab).none { it.first == key }) return defaultSort(tab)
        return LibSort(key, if (dir == "asc" || dir == "desc") dir else if (key in DESC_FIRST) "desc" else "asc")
    }

    fun set(tab: String, s: LibSort) {
        try { prefs.edit().putString("sort.$tab", "${s.key}:${s.dir}").apply() } catch (_: Exception) { /* kept for this visit */ }
    }
}

/**
 * The Sort ▾ chip beside the tabs: the current sort and its direction; the menu picks another sort (each starts
 * in its own direction) or reverses this one.
 */
@Composable
private fun SortChip(tab: String, sort: LibSort, onChange: (LibSort) -> Unit, modifier: Modifier = Modifier) {
    val p = MaterialTheme.palette
    val opts = SORTS.getValue(tab)
    val full = opts.firstOrNull { it.first == sort.key }?.second ?: opts.first().second
    val label = CHIP[full] ?: full
    var open by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(12.dp)
    Box(modifier.padding(end = Gutter)) {
        Row(
            Modifier
                .clip(shape)
                .background(p.panel)
                .border(1.dp, p.border, shape)
                .clickable(onClickLabel = "Sort: $full") { open = true }
                .padding(start = 10.dp, end = 6.dp, top = 9.dp, bottom = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                painterResource(if (sort.dir == "asc") R.drawable.ic_arrow_upward else R.drawable.ic_arrow_downward),
                contentDescription = if (sort.dir == "asc") "Ascending" else "Descending",
                tint = p.muted,
                modifier = Modifier.size(14.dp),
            )
            Spacer(Modifier.width(4.dp))
            Text(label, style = MaterialTheme.typography.labelLarge, color = p.text, maxLines = 1)
            Icon(painterResource(R.drawable.ic_expand_more), contentDescription = "Sort", tint = p.muted, modifier = Modifier.size(18.dp))
        }
        PopMenu(
            open,
            { open = false },
            opts.map { (key, l) ->
                MenuEntry(l, iconRes = if (key == sort.key) R.drawable.ic_check else null) {
                    if (key != sort.key) onChange(LibSort(key, if (key in DESC_FIRST) "desc" else "asc"))
                }
            } + listOf(
                MenuSeparator,
                MenuEntry("Reverse order", iconRes = if (sort.dir == "asc") R.drawable.ic_arrow_downward else R.drawable.ic_arrow_upward) { onChange(sort.copy(dir = if (sort.dir == "asc") "desc" else "asc")) },
            ),
        )
    }
}

/**
 * Library: Albums / Artists / Songs (the web's #/library tabs), Downloaded once songs are saved on this phone
 * (features/offline), and for admins Wrong audio (the songs the audio check flagged). In offline mode the saved songs
 * are the whole library.
 *
 * The search box (0.31) searches the whole library whatever tab is selected: while it has a query the page shows
 * one results view (Artists, Albums, Songs, each with "Show all N" that opens its tab filtered by the query); a
 * tab tapped while a query is typed shows that tab filtered. Nothing anywhere: "Search everywhere" opens Discover.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LibraryScreen(nav: Nav) {
    val offline by OfflineMode.active.collectAsState()
    if (offline) {
        OfflineLibraryScreen(nav)
        return
    }
    var tab by LibraryState::tab
    val context = LocalContext.current
    val store = remember(context) { LibrarySorts(context) }
    val saved = remember(context) { OfflineStore.of(context) }
    val savedAll by saved.all.collectAsState()
    // admins: the web's Wrong audio tab (after Downloaded, so that one keeps its place)
    val me by SongFixUi.me.collectAsState()
    val tabs = TABS + (if (saved.songs(savedAll).isNotEmpty()) listOf("downloaded" to "Downloaded") else emptyList()) +
        (if (me?.isAdmin == true) listOf("issues" to "Wrong audio") else emptyList())
    val labels = tabs.map { it.second }
    val shown = if (tab < labels.size) tab else 0
    // every tab's sort, remembered per tab
    var sorts by remember { mutableStateOf(TABS.associate { (k, _) -> k to store.get(k) }) }
    // (Downloaded has no sort: it keeps the albums as they were saved)
    val tabKey = TABS.getOrNull(shown)?.first ?: TABS[0].first
    val sort = sorts.getValue(tabKey)
    val scope = rememberCoroutineScope()
    // the query the lists use: what is typed, a moment after the typing stops (at once when cleared)
    var q by remember { mutableStateOf(LibraryState.text.trim()) }
    LaunchedEffect(LibraryState.text) {
        val v = LibraryState.text.trim()
        if (v == q) return@LaunchedEffect
        if (v.isNotEmpty()) delay(300)
        q = v
    }
    val resultsView = q.isNotEmpty() && LibraryState.results
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
            LibrarySearchField(LibraryState.text) { v ->
                LibraryState.text = v
                // typing shows the results view again
                LibraryState.results = true
            }
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                // with a query, a tab shows that tab filtered by it
                ChoiceRow(labels, if (resultsView) -1 else shown, { tab = it; LibraryState.results = false }, Modifier.weight(1f), end = 8.dp)
                if (shown < TABS.size && !resultsView) {
                    SortChip(tabKey, sort, { next ->
                        sorts = sorts + (tabKey to next)
                        store.set(tabKey, next)
                    })
                }
            }
            if (resultsView) {
                LibraryResults(nav, q) { section ->
                    tab = section
                    LibraryState.results = false
                }
            } else when (tabs[shown].first) {
                "albums" -> AlbumsTab(nav, sort, q)
                "artists" -> ArtistsTab(nav, sort, q)
                "tracks" -> SongsTab(nav, sort, q)
                "issues" -> WrongAudioList(nav, q)
                else -> DownloadedList(nav)
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

    /**
     * Loads the pages already loaded again, in place (a song was corrected or deleted from its ⋯): the rows stay on
     * screen, and the scroll position with them, until each page's new answer replaces them.
     */
    fun refresh() {
        val pages = loaded.sorted()
        loaded.clear()
        inFlight.clear()
        done = false
        loading = false
        error = null
        generation += 1
        for (page in pages) want(page * pageSize)
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
    LaunchedEffect(paged, paged.generation, paged.rows.size) { paged.want(paged.rows.size) }
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
    LaunchedEffect(paged, index / paged.pageSize, paged.generation) { paged.want(index) }
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
private fun AlbumsTab(nav: Nav, sort: LibSort, q: String = "") {
    val api = nav.graph.api
    val scope = rememberCoroutineScope()
    // a new list per sort (and query); the index is for the same sort and direction, so its offsets are this list's
    // rows (a filtered list has no index)
    val paged = remember(sort, q) { Paged<Album>(120) }
    paged.scope = scope
    paged.fetch = { offset, limit -> api.albums(sort = sort.key, offset = offset, limit = limit, q = q, dir = sort.dir) }
    val grid = rememberLazyGridState()
    LaunchedEffect(sort, q) { if (grid.firstVisibleItemIndex > 0) grid.scrollToItem(0) }
    val index = rememberLibraryIndex(api, "albums", sort.key, dir = sort.dir)
    val sections = if (q.isEmpty()) index else emptyList()
    // covers 2–3 screens ahead of the scroll, and around the spot after a scrubber jump
    PreloadCoversAhead(grid, paged.rows.size, 400) { i -> paged.rows.getOrNull(i)?.id }
    Column(Modifier.fillMaxSize()) {
        if (paged.done && paged.rows.isEmpty()) {
            EmptyState(R.drawable.ic_library, if (q.isEmpty()) "No albums found" else "No albums match “$q”")
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
private fun ArtistsTab(nav: Nav, sort: LibSort, q: String = "") {
    val api = nav.graph.api
    val loader = rememberLoad("artists", sort, q) { api.artists(q = q, sort = sort.key, dir = sort.dir) }
    val grid = rememberLazyGridState()
    LaunchedEffect(sort, q) { if (grid.firstVisibleItemIndex > 0) grid.scrollToItem(0) }
    val index = rememberLibraryIndex(api, "artists", sort.key, dir = sort.dir)
    val sections = if (q.isEmpty()) index else emptyList()
    LoadableContent(loader) { artists ->
        if (artists.isEmpty()) {
            EmptyState(R.drawable.ic_person, if (q.isEmpty()) "No artists found" else "No artists match “$q”")
            return@LoadableContent
        }
        PreloadCoversAhead(grid, artists.size, 400) { i -> artists.getOrNull(i)?.coverAlbum }
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
private fun SongsTab(nav: Nav, sort: LibSort, q: String = "") {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val scope = rememberCoroutineScope()
    val paged = remember(sort, q) { Paged<Track>(200) }
    paged.scope = scope
    paged.fetch = { offset, limit -> api.tracks(q = q, offset = offset, limit = limit, sort = sort.key, dir = sort.dir) }
    // a song corrected or deleted from its ⋯: the loaded pages load again
    OnSongFixed { paged.refresh() }
    val list = rememberLazyListState()
    LaunchedEffect(sort, q) { if (list.firstVisibleItemIndex > 0) list.scrollToItem(0) }
    val index = rememberLibraryIndex(api, "tracks", sort.key, dir = sort.dir)
    val sections = if (q.isEmpty()) index else emptyList()
    PreloadCoversAhead(list, paged.rows.size, 96) { i -> paged.rows.getOrNull(i)?.albumId }
    if (paged.done && paged.rows.isEmpty()) {
        EmptyState(R.drawable.ic_note, if (q.isEmpty()) "No songs found" else "No songs match “$q”")
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
                        subtitleLinks = rememberSongLinks(nav, t),
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

// ---- the search box and the one results view (0.31) ----------------------------------------------------------

/** The Library's search box (the web's `.search .input`). */
@Composable
private fun LibrarySearchField(value: String, onChange: (String) -> Unit) {
    val p = MaterialTheme.palette
    val focus = LocalFocusManager.current
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        placeholder = { Text("Search artists, albums, songs in your library…", color = p.faint, maxLines = 1) },
        singleLine = true,
        shape = RoundedCornerShape(12.dp),
        leadingIcon = { Icon(painterResource(R.drawable.ic_search), contentDescription = null, tint = p.faint) },
        trailingIcon = {
            if (value.isNotEmpty()) {
                IconButton(onClick = { onChange("") }) { Icon(painterResource(R.drawable.ic_close), contentDescription = "Clear", tint = p.muted) }
            }
        },
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
        keyboardActions = KeyboardActions(onSearch = { focus.clearFocus() }),
        colors = OutlinedTextFieldDefaults.colors(
            focusedBorderColor = p.accent,
            unfocusedBorderColor = p.border,
            focusedContainerColor = p.bg2,
            unfocusedContainerColor = p.bg2,
            cursorColor = p.accent,
        ),
        modifier = Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 4.dp),
    )
}

/**
 * The results view of a library search (GET /api/library/search): Artists (round tiles), Albums (singles too) and
 * Songs (rows that play), each with its first matches and "Show all N" ([onShowAll] with the tab: 0 albums,
 * 1 artists, 2 songs). Servers before 0.31 send no counts: "Show all" then appears when a section is full.
 */
@Composable
private fun LibraryResults(nav: Nav, q: String, onShowAll: (Int) -> Unit) {
    val api = nav.graph.api
    val p = MaterialTheme.palette
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    var found by remember { mutableStateOf<Pair<String, LibrarySearch>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var generation by remember { mutableStateOf(0) }
    // a song corrected or deleted from its ⋯: the search runs again (the results stay until it answers)
    OnSongFixed { generation += 1 }
    // the newest query wins: a new one cancels the request of the one before
    LaunchedEffect(q, generation) {
        error = null
        try {
            val r = withContext(Dispatchers.IO) { api.search(q, RESULTS_LIMIT) }
            found = q to r
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Search failed"
        }
    }
    val f = found?.takeIf { it.first == q }?.second
    val err = error
    when {
        f == null && err != null -> ErrorBox(err) { generation += 1 }
        f == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(color = p.accent, trackColor = p.border)
        }
        f.isEmpty -> Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
            EmptyState(R.drawable.ic_search, "Nothing in your library matches “$q”")
            EncoreButton("Search everywhere", iconRes = R.drawable.ic_explore, primary = true) { nav.open(Screen.DiscoverSearch(q)) }
        }
        else -> LazyColumn(Modifier.fillMaxSize()) {
            val c = f.counts
            fun showAll(total: Int?, shown: Int): String? = when {
                total != null -> if (total > shown) "Show all $total" else null
                shown >= RESULTS_LIMIT -> "Show all"
                else -> null
            }
            if (f.artists.isNotEmpty()) {
                val action = showAll(c?.artists, f.artists.size)
                item(key = "artists-head") { SectionHead("Artists", action = action, onAction = if (action != null) ({ onShowAll(1) }) else null) }
                item(key = "artists") {
                    LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        rowItems(f.artists, key = { "artist:" + it.name }) { a ->
                            Tile(
                                title = a.shownName(),
                                subtitle = "${a.albums} album${if (a.albums == 1) "" else "s"}",
                                centered = true,
                                modifier = Modifier.width(120.dp),
                                onClick = { nav.open(Screen.Artist(a.name)) },
                            ) { m ->
                                Cover(if (a.coverAlbum.isNotEmpty()) api.coverUrl(a.coverAlbum, 400) else null, nav.graph.imageLoader, m, corner = 999.dp, label = a.name)
                            }
                        }
                    }
                }
            }
            if (f.albums.isNotEmpty()) {
                val action = showAll(c?.albums, f.albums.size)
                item(key = "albums-head") { SectionHead("Albums", action = action, onAction = if (action != null) ({ onShowAll(0) }) else null) }
                item(key = "albums") {
                    LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        rowItems(f.albums, key = { "album:" + it.id }) { a ->
                            AlbumCard(a, api.coverUrl(a.id, 400), nav.graph.imageLoader, Modifier.width(140.dp)) { nav.open(Screen.Album(a.id)) }
                        }
                    }
                }
            }
            if (f.songs.isNotEmpty()) {
                val action = showAll(c?.songs, f.songs.size)
                item(key = "songs-head") { SectionHead("Songs", action = action, onAction = if (action != null) ({ onShowAll(2) }) else null) }
                items(f.songs.size, key = { i -> "song:" + f.songs[i].id + ":" + i }) { i ->
                    val t = f.songs[i]
                    TrackRow(
                        track = t,
                        imageLoader = nav.graph.imageLoader,
                        coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                        liked = t.id in liked,
                        playing = nowPlaying?.id == t.id,
                        onClick = { nav.player.play(f.songs, i) },
                        onLike = { nav.graph.likes.toggle(t.id) },
                        subtitle = listOf(t.artist, t.album).filter { it.isNotEmpty() }.joinToString(" · "),
                        subtitleLinks = rememberSongLinks(nav, t),
                        onMore = { nav.trackMenu(t) },
                    )
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }
}
