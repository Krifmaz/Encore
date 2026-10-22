package app.encore.android.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.R
import app.encore.android.data.repo.CatalogArtist
import app.encore.android.data.repo.FyTrack
import app.encore.android.data.repo.Tags
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.Loadable
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.PageTitle
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Tile
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** web/js/pages/foryou.js `KIND`: what each ListenBrainz / Last.fm list is. */
private val FY_KIND = mapOf(
    "weekly-exploration" to "New music picked for you every Monday",
    "daily-jams" to "Songs you love, refreshed daily",
    "weekly-jams" to "Your favourites from the past weeks",
    "lastfm" to "Similar to your most played songs on Last.fm",
)

/** The web's `requestAll(tracks)`: one request per song, stopping at the weekly limit; a toast with the count. */
suspend fun requestAllTracks(nav: Nav, tracks: List<FyTrack>, statuses: RequestStatuses) {
    var ok = 0
    for (t in tracks) {
        try {
            val s = withContext(Dispatchers.IO) { nav.features.createRequest(t.requestBody()) }
            statuses[fyKey(t)] = s.ifEmpty { "queued" }
            ok++
        } catch (e: Exception) {
            if (isLimitError(e)) { Toasts.show(e.message ?: "Weekly request limit reached", "err"); break }
        }
    }
    Toasts.show("$ok song(s) requested", "ok")
}

/** The key of a recommended song in a [RequestStatuses] map. */
fun fyKey(t: FyTrack): String = t.mbid.ifEmpty { t.artist + "|" + t.title }

/** foryou.js `trackRow`: play when owned, "More like this", and "In library" / status / Request. */
@Composable
fun FyTrackRow(nav: Nav, t: FyTrack, statuses: RequestStatuses, playable: List<FyTrack>, onMore: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val lib = t.library
    val status = statuses[fyKey(t)] ?: t.requestStatus
    val artistScope = rememberCoroutineScope()
    Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        PlayableCover(nav, t.cover.ifEmpty { null }, t.album.ifEmpty { t.title }, 44.dp, lib != null) {
            val q = playable.mapNotNull { it.library }
            if (lib != null) nav.player.play(q, q.indexOf(lib).coerceAtLeast(0))
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(t.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Row {
                Text(
                    t.artist, style = MaterialTheme.typography.bodySmall, color = p.text, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false).clickable { openCatalogArtist(nav, t.artistMbid, t.artist, artistScope) },
                )
                if (t.album.isNotEmpty()) Text(" · " + t.album, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (t.lengthSec > 0) Text(formatDuration((t.lengthSec * 1000).toLong()), style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(horizontal = 4.dp))
        GhostIconButton(R.drawable.ic_spark, "More like this", onClick = onMore)
        when {
            lib != null -> Badge("In library", kind = "ok")
            status.isNotEmpty() -> StatusBadge(status)
            else -> SmallButton("Request", iconRes = R.drawable.ic_add) {
                scope.launch { submitRequest(nav.features, t.requestBody())?.let { statuses[fyKey(t)] = it } }
            }
        }
    }
}

/** The web's "More like <title>" modal: similar songs from Last.fm / Deezer radio, playable or requestable. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MoreLikeThisSheet(nav: Nav, artist: String, title: String, mbid: String, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val loader = rememberLoad("similar-tracks", artist, title) { nav.features.similarTracks(artist, title, mbid) }
    val statuses = rememberRequestStatuses()
    var nested by remember { mutableStateOf<FyTrack?>(null) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().padding(bottom = 24.dp)) {
            Text("More like $title", style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(horizontal = 22.dp, vertical = 8.dp))
            when (val s = loader.state.value) {
                is Loadable.Loading -> Row(Modifier.padding(horizontal = 22.dp, vertical = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Spinner(); Spacer(Modifier.width(8.dp)); Text("Finding similar songs…", color = p.muted)
                }
                is Loadable.Failed -> Box(Modifier.height(160.dp)) { ErrorBox(s.message) { loader.retry() } }
                is Loadable.Ready -> {
                    val d = s.value
                    if (d.tracks.isEmpty()) {
                        EmptyState(R.drawable.ic_spark, "Nothing similar found", "Add a Last.fm API key in Settings for the best results.")
                    } else {
                        val missing = d.tracks.filter { it.library == null && (statuses[fyKey(it)] ?: it.requestStatus).isEmpty() }
                        Row(Modifier.fillMaxWidth().padding(horizontal = 22.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text("from ${d.source}", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.weight(1f))
                            if (missing.isNotEmpty()) SmallButton("Request ${missing.size} missing", iconRes = R.drawable.ic_download) { scope.launch { requestAllTracks(nav, missing, statuses) } }
                        }
                        LazyColumn(Modifier.fillMaxWidth().height(if (d.tracks.size > 7) 440.dp else (d.tracks.size * 60 + 8).dp)) {
                            items(d.tracks.size, key = { i -> "ml-$i" }) { i -> FyTrackRow(nav, d.tracks[i], statuses, d.tracks, onMore = { nested = d.tracks[i] }) }
                        }
                    }
                }
            }
        }
    }
    nested?.let { t -> MoreLikeThisSheet(nav, t.artist, t.title, t.mbid, onDismiss = { nested = null }) }
}

/** The web's "Browse by genre" chips: library genres on the accent, then Last.fm's top tags. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun GenreChips(nav: Nav, tags: Tags) {
    val lib = tags.library.take(15)
    val lower = lib.map { it.lowercase() }.toSet()
    val all = lib.map { it to true } + tags.top.filter { it.lowercase() !in lower }.map { it to false }
    FlowRow(Modifier.fillMaxWidth().padding(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        for ((g, mine) in all) Badge(g, kind = if (mine) "accent" else "", onClick = { nav.open(Screen.Tag(g)) })
    }
}

/**
 * Discover before anything is typed: the For you picks as short horizontal rows (like Home's Recently added), each
 * with "See all" for the full For you page, then the genres. Owned songs play, the rest open a search for them.
 */
@Composable
fun ForYouRows(nav: Nav) {
    val features = nav.features
    val p = MaterialTheme.palette
    val loader = rememberLoad("for-you-rows") { features.forYou() }
    val tagsLoader = rememberLoad("tags") { try { features.tags() } catch (_: Exception) { Tags(emptyList(), emptyList()) } }
    val tags = (tagsLoader.state.value as? Loadable.Ready)?.value
    val seeAll = { nav.open(Screen.ForYou) }
    LazyColumn(Modifier.fillMaxSize()) {
        when (val s = loader.state.value) {
            is Loadable.Loading -> item { Row(Modifier.padding(horizontal = Gutter, vertical = 16.dp)) { Spinner() } }
            is Loadable.Failed -> item { NoteText("Couldn't load your picks: ${s.message}", Modifier.padding(vertical = 12.dp)) }
            is Loadable.Ready -> {
                val d = s.value
                val lf = d.lastfm
                val playlists = buildList {
                    if (lf != null && lf.tracks.isNotEmpty()) add(app.encore.android.data.repo.FyPlaylist("", "Songs for you", "lastfm", lf.tracks))
                    addAll(d.playlists.filter { it.tracks.isNotEmpty() })
                }
                val nothing = playlists.isEmpty() && lf?.artists.isNullOrEmpty() && lf?.albums.isNullOrEmpty()
                if (!d.connected || nothing) {
                    item {
                        EmptyState(
                            R.drawable.ic_search,
                            "Search your library and the whole catalogue",
                            if (!d.connected) "Connect Last.fm or ListenBrainz to get picks for you here." else "What you already have shows first; anything else can be requested with one click.",
                        )
                    }
                }
                if (lf != null && lf.artists.isNotEmpty()) {
                    item { SectionHead("Artists for you", action = "See all", onAction = seeAll) }
                    item {
                        LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                            items(lf.artists.size, key = { i -> "fy-artist-$i" }) { i -> CatalogArtistTile(nav, lf.artists[i], Modifier.width(120.dp), subtitle = " ") }
                        }
                    }
                }
                if (lf != null && lf.albums.isNotEmpty()) {
                    item { SectionHead("Albums for you", action = "See all", onAction = seeAll) }
                    item {
                        LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                            items(lf.albums.size, key = { i -> "fy-album-$i" }) { i ->
                                val a = lf.albums[i]
                                Tile(
                                    title = a.title,
                                    subtitle = listOf(a.artist, a.year).filter { it.isNotEmpty() }.joinToString(" · ").ifEmpty { " " },
                                    modifier = Modifier.width(150.dp),
                                    onClick = { nav.open(Screen.DiscoverSearch("${a.artist} ${a.title}", "album")) },
                                ) { m ->
                                    Box(m) {
                                        Cover(a.cover.ifEmpty { null }, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 12.dp, label = a.title)
                                        Availability(a.inLibrary, a.requestStatus, Modifier.padding(8.dp))
                                    }
                                }
                            }
                        }
                    }
                }
                playlists.forEachIndexed { pi, pl ->
                    val owned = pl.tracks.mapNotNull { it.library }
                    item(key = "fy-pl-head-$pi") {
                        SectionHead(pl.title, subtitle = "${owned.size}/${pl.tracks.size} in library", action = "See all", onAction = seeAll)
                    }
                    item(key = "fy-pl-$pi") {
                        LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                            items(pl.tracks.size, key = { i -> "fy-pl-$pi-$i" }) { i ->
                                val t = pl.tracks[i]
                                val lib = t.library
                                Tile(
                                    title = t.title,
                                    subtitle = t.artist.ifEmpty { " " },
                                    modifier = Modifier.width(150.dp),
                                    onClick = {
                                        if (lib != null) nav.player.play(owned, owned.indexOf(lib).coerceAtLeast(0))
                                        else nav.open(Screen.DiscoverSearch("${t.artist} ${t.title}", "track"))
                                    },
                                ) { m ->
                                    Box(m) {
                                        Cover(t.cover.ifEmpty { null }, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 12.dp, label = t.album.ifEmpty { t.title })
                                        Availability(lib != null, t.requestStatus, Modifier.padding(8.dp))
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if (tags != null && (tags.library.isNotEmpty() || tags.top.isNotEmpty())) {
            item { SectionHead("Browse by genre") }
            item {
                val lib = tags.library.take(15)
                val lower = lib.map { it.lowercase() }.toSet()
                val all = lib.map { it to true } + tags.top.filter { it.lowercase() !in lower }.map { it to false }
                LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    items(all.size, key = { i -> "fy-tag-$i" }) { i ->
                        val (g, mine) = all[i]
                        Badge(g, kind = if (mine) "accent" else "", onClick = { nav.open(Screen.Tag(g)) })
                    }
                }
            }
        }
        item { Spacer(Modifier.height(24.dp)) }
    }
}

/** For you (web/js/pages/foryou.js `render`): ListenBrainz playlists, Last.fm songs / artists / albums, genres. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ForYouScreen(nav: Nav) {
    val features = nav.features
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var reloads by remember { mutableStateOf(0) }
    val loader = rememberLoad("for-you", reloads) { features.forYou() }
    val tagsLoader = rememberLoad("tags") { try { features.tags() } catch (_: Exception) { Tags(emptyList(), emptyList()) } }
    val statuses = rememberRequestStatuses()
    var moreLike by remember { mutableStateOf<FyTrack?>(null) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()

    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("For you", scrollBehavior = scrollBehavior, onBack = nav.back) },
    ) { padding ->
        LoadableContent(loader) { d ->
            val tags = (tagsLoader.state.value as? Loadable.Ready)?.value
            // Last.fm's songs show as one more list, first
            val playlists = buildList {
                val lf = d.lastfm
                if (lf != null && lf.tracks.isNotEmpty()) add(app.encore.android.data.repo.FyPlaylist("", "Songs for you", "lastfm", lf.tracks))
                addAll(d.playlists)
            }
            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                item {
                    NoteText(
                        "Recommendations from your ListenBrainz and Last.fm listening. Songs you already have play right here; request the rest in one click.",
                        Modifier.padding(bottom = 10.dp),
                    )
                }
                when {
                    !d.connected -> item {
                        Box(Modifier.padding(Gutter)) {
                            CardBox {
                                EmptyState(
                                    R.drawable.ic_spark,
                                    "Connect Last.fm or ListenBrainz to get recommendations",
                                    "Connect either one on the Connections page. Encore scrobbles what you play here to them, and your other players (Navidrome, Finamp, Plexamp…) can scrobble too.",
                                )
                            }
                        }
                    }
                    playlists.isEmpty() && d.lastfm == null -> item {
                        Box(Modifier.padding(Gutter)) {
                            CardBox {
                                EmptyState(
                                    R.drawable.ic_spark,
                                    "No recommendations yet",
                                    "ListenBrainz builds Weekly Exploration once ${d.user} has a few weeks of listens. Check that your players scrobble to ListenBrainz.",
                                )
                            }
                        }
                    }
                    else -> {
                        for (e in d.errors) {
                            item { Box(Modifier.height(90.dp)) { ErrorBox(e) { reloads += 1 } } }
                        }
                        val lf = d.lastfm
                        if (lf != null && lf.artists.isNotEmpty()) {
                            item { SectionHead("Artists for you", subtitle = "Similar to what ${lf.user} plays most on Last.fm") }
                            tileRows(lf.artists, 3, key = "lf-artists") { a -> CatalogArtistTile(nav, a, subtitle = " ") }
                        }
                        if (lf != null && lf.albums.isNotEmpty()) {
                            item { SectionHead("Albums for you", subtitle = "The best-known album of artists you might like") }
                            tileRows(lf.albums, 2, key = "lf-albums") { a ->
                                Tile(
                                    title = a.title,
                                    subtitle = listOf(a.artist, a.year).filter { it.isNotEmpty() }.joinToString(" · "),
                                    onClick = { nav.open(Screen.DiscoverSearch("${a.artist} ${a.title}", "album")) },
                                ) { m ->
                                    Box(m) {
                                        Cover(a.cover.ifEmpty { null }, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 12.dp, label = a.title)
                                        Availability(a.inLibrary, a.requestStatus, Modifier.padding(8.dp))
                                    }
                                }
                            }
                        }
                        playlists.forEachIndexed { pi, pl ->
                            val owned = pl.tracks.filter { it.library != null }
                            val missing = pl.tracks.filter { it.library == null && (statuses[fyKey(it)] ?: it.requestStatus).isEmpty() }
                            item(key = "pl-head-$pi") {
                                Column {
                                    SectionHead(
                                        pl.title,
                                        subtitle = (FY_KIND[pl.kind] ?: "") + if (pl.tracks.isNotEmpty()) " · ${owned.size}/${pl.tracks.size} in library" else "",
                                    )
                                    Row(Modifier.padding(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                        if (owned.isNotEmpty()) SmallButton("Play owned", iconRes = R.drawable.ic_play) { nav.player.play(owned.mapNotNull { it.library }) }
                                        if (missing.isNotEmpty()) SmallButton("Request ${missing.size} missing", iconRes = R.drawable.ic_download, primary = true) {
                                            scope.launch { requestAllTracks(nav, missing, statuses) }
                                        }
                                        if (pl.id.isNotEmpty()) {
                                            val context = LocalContext.current
                                            SmallButton("ListenBrainz", iconRes = R.drawable.ic_open_in_new) { openUrl(context, "https://listenbrainz.org/playlist/${pl.id}") }
                                        }
                                    }
                                }
                            }
                            items(pl.tracks.size, key = { i -> "pl-$pi-$i" }) { i -> FyTrackRow(nav, pl.tracks[i], statuses, pl.tracks, onMore = { moreLike = pl.tracks[i] }) }
                        }
                    }
                }
                item { SectionHead("Browse by genre") }
                if (tags != null) item { GenreChips(nav, tags) }
                item { Spacer(Modifier.height(24.dp)) }
            }
            moreLike?.let { t -> MoreLikeThisSheet(nav, t.artist, t.title, t.mbid, onDismiss = { moreLike = null }) }
        }
    }
}

/** A genre page (foryou.js `renderTag`): the top artists of a tag, 48 a page, with "Load more". */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TagScreen(nav: Nav, tag: String) {
    val features = nav.features
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val first = rememberLoad("tag", tag) { features.tag(tag, 1) }
    var moreArtists by remember { mutableStateOf<List<CatalogArtist>>(emptyList()) }
    var page by remember { mutableStateOf(1) }
    var loadingMore by remember { mutableStateOf(false) }
    var hasMore by remember { mutableStateOf(true) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("", scrollBehavior = scrollBehavior, onBack = nav.back) },
    ) { padding ->
        LoadableContent(first) { d ->
            val artists = d.artists + moreArtists
            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                item {
                    Column(Modifier.padding(horizontal = Gutter)) {
                        Text("GENRE", style = MaterialTheme.typography.labelSmall, color = p.muted, letterSpacing = 1.2.sp)
                        Text(tag.replaceFirstChar { it.uppercase() }, style = MaterialTheme.typography.headlineMedium)
                        Text(
                            if (d.summary.isNotEmpty()) d.summary + " · " + d.source else "Top artists from ${d.source}",
                            style = MaterialTheme.typography.bodyMedium, color = p.muted, modifier = Modifier.padding(top = 6.dp, bottom = 10.dp),
                        )
                    }
                }
                if (artists.isEmpty()) {
                    item { EmptyState(R.drawable.ic_person, "No artists for this genre") }
                } else {
                    tileRows(artists, 3, key = "tag") { a -> CatalogArtistTile(nav, a, subtitle = " ") }
                }
                if (hasMore && d.artists.size >= 40) {
                    item {
                        Row(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalArrangement = Arrangement.Center) {
                            EncoreButton("Load more", enabled = !loadingMore) {
                                loadingMore = true
                                scope.launch {
                                    try {
                                        val next = withContext(Dispatchers.IO) { features.tag(tag, page + 1) }
                                        page += 1
                                        moreArtists = moreArtists + next.artists
                                        if (next.artists.size < 40) hasMore = false
                                    } catch (e: Exception) {
                                        Toasts.show(e.message ?: "Couldn't load more", "err")
                                    }
                                    loadingMore = false
                                }
                            }
                        }
                    }
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
}
