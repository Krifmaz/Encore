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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.shadow

import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.ArtistSongs
import app.encore.android.data.api.Track
import app.encore.android.data.repo.AlbumPage
import app.encore.android.data.repo.ArtistExtras
import app.encore.android.data.repo.ArtistPage
import app.encore.android.data.repo.CatalogAlbum
import app.encore.android.data.repo.ExtRelease
import app.encore.android.data.repo.ExtTrack
import app.encore.android.data.repo.PickerOpts
import app.encore.android.data.repo.SimilarArtists
import app.encore.android.data.repo.TopTrack
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.FastScroller
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.Loadable
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.RoundCover
import app.encore.android.ui.components.ScrollerTarget
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Tile
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.components.formatLongDuration
import app.encore.android.ui.components.rememberLibraryIndex
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.components.shareLinkOf
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// ---- artist (web/js/pages/discover.js renderArtist) --------------------------------------------------------------

/**
 * Request discography. With [releaseIds] (the boxes ticked in the sheet) the server requests exactly those,
 * minus the owned and requested ones; without, every release the server's release-type filter (or this
 * request's own [filter], {kinds, skip}) wants that isn't owned or requested yet. The server decides (title
 * and year dedupe, library check); each release arrives as a request event and marks its tile. → the request's
 * status, or null.
 */
private suspend fun requestDiscography(features: app.encore.android.data.repo.FeaturesApi, a: ArtistPage, cover: String, releaseIds: List<String>?, filter: JsonObject?): String? {
    val body = buildJsonObject {
        put("kind", "artist")
        put("mbid", a.id)
        put("artist", a.name)
        if (cover.isNotEmpty()) put("cover_url", cover)
        if (releaseIds != null) put("release_ids", JsonArray(releaseIds.map { JsonPrimitive(it) }))
        if (filter != null) put("release_filter", filter)
    }
    return submitRequest(features, body)
}

/** One box of the Request discography sheet: a MusicBrainz release group, or a release only Deezer / Apple Music know. */
private data class DiscoEntry(
    val id: String,
    val title: String,
    val year: String,
    /** The group it is shown under ("Album", "Live" …). */
    val kind: String,
    /** The MusicBrainz primary type ("Album", "EP", "Single" …), for the release-type fallback sent to older servers. */
    val primary: String,
    val allowed: Boolean,
    val skipped: String,
    val owned: Boolean,
    /** The live request status, "" when not requested. */
    val requested: String,
    val requestedLabel: String,
) {
    /** Owned or requested releases can't be ticked: there is nothing to request. */
    val fixed: Boolean get() = owned || requested.isNotEmpty()
}

private fun primaryOf(type: String): String = when (type) {
    "" -> "Other"
    "Compilation" -> "Album"
    else -> type
}

/**
 * The Request discography sheet (the web's dialog): every release on the page – MusicBrainz, and the Deezer /
 * Apple Music ones that have loaded – grouped by kind, a box per release, ticked to start when Settings →
 * Release types allows it and it isn't owned or requested; Select all / Deselect all per group and overall;
 * the button counts the ticks. Only the ticked releases are requested.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun DiscographySheet(a: ArtistPage, ext: List<ExtRelease>, statuses: RequestStatuses, onDismiss: () -> Unit, onRequest: (List<DiscoEntry>) -> Unit) {
    val p = MaterialTheme.palette
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val entries = a.releaseGroups.map { rg ->
        val rgKeys = albumKeys(rg)
        val status = statuses.statusOf(rgKeys, rg.requestStatus)
        DiscoEntry(rg.id, rg.title, rg.year, rg.kind.ifEmpty { groupKey(rg) }, primaryOf(rg.type), rg.allowed, rg.skipped, rg.inLibrary, status, statuses.labelOf(rgKeys, rg.requestLabel))
    } + ext.filter { it.id.isNotEmpty() }.map { m ->
        val mKeys = extReleaseKeys(a.name, m)
        val status = statuses.statusOf(mKeys, m.requestStatus)
        DiscoEntry(m.id, m.title, m.year, m.kind.ifEmpty { m.type.ifEmpty { "Other" } }, primaryOf(m.type), m.allowed, m.skipped, m.inLibrary, status, statuses.labelOf(mKeys, m.requestLabel))
    }

    val groups = entries.groupBy { it.kind }
    val keys = groups.keys.sortedBy { groupRank(it) }
    var ticked by remember { mutableStateOf(entries.filter { it.allowed && !it.fixed }.map { it.id }.toSet()) }
    fun selectable(list: List<DiscoEntry>): Set<String> = list.filter { !it.fixed }.map { it.id }.toSet()
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 22.dp).padding(bottom = 24.dp)) {
            Text("Request discography", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(vertical = 8.dp))
            Text(
                "Every release of ${a.name} on this page, from MusicBrainz${if (a.catalogMerge) ", Deezer and Apple Music" else ""}. Ticked to start: what Settings → Release types allows and you don't have or haven't requested yet.",
                style = MaterialTheme.typography.bodySmall, color = p.muted,
            )
            Row(Modifier.fillMaxWidth().padding(top = 10.dp), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                Text("${entries.size} releases", style = MaterialTheme.typography.bodySmall, color = p.muted)
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    SmallButton("Select all") { ticked = selectable(entries) }
                    SmallButton("Deselect all") { ticked = emptySet() }
                }
            }
            for (k in keys) {
                val list = groups[k] ?: continue
                Row(Modifier.fillMaxWidth().padding(top = 14.dp, bottom = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("${k}s", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                    Text("${list.size}", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(end = 8.dp))
                    SmallButton("Select all") { ticked = ticked + selectable(list) }
                    Spacer(Modifier.width(6.dp))
                    SmallButton("Deselect all") { ticked = ticked - list.map { it.id }.toSet() }
                }
                for (e in list) {
                    ReleaseBox(e, e.id in ticked) { on -> ticked = if (on) ticked + e.id else ticked - e.id }
                }
            }
            Row(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                EncoreButton("Cancel") { onDismiss() }
                val n = ticked.size
                EncoreButton("Request $n release${if (n == 1) "" else "s"}", iconRes = R.drawable.ic_download, primary = true) {
                    if (ticked.isEmpty()) {
                        Toasts.show("Tick at least one release", "err")
                    } else {
                        onRequest(entries.filter { it.id in ticked }) // in the page's order
                    }
                }
            }
        }
    }
}

/** A release's box: title, year and why it starts unticked (owned, requested, or off in Release types). */
@Composable
private fun ReleaseBox(e: DiscoEntry, checked: Boolean, onChange: (Boolean) -> Unit) {
    val p = MaterialTheme.palette
    val note = when {
        e.owned -> "In library"
        e.requested.isNotEmpty() -> e.requestedLabel.ifEmpty { "Requested" }
        e.skipped.isNotEmpty() -> "${e.skipped} – off in Release types"
        else -> ""
    }
    Row(
        Modifier.fillMaxWidth().clickable(enabled = !e.fixed) { onChange(!checked) }.alpha(if (e.fixed) 0.6f else 1f),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Checkbox(checked = checked, onCheckedChange = { onChange(it) }, enabled = !e.fixed)
        Column(Modifier.weight(1f)) {
            Text(e.title, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val sub = listOf(e.year, note).filter { it.isNotEmpty() }.joinToString(" · ")
            if (sub.isNotEmpty()) Text(sub, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}


/** The order the discography's groups come in. */
private val GROUP_ORDER = listOf(
    "Album", "EP", "Single", "Compilation", "Live", "Soundtrack", "Remix", "DJ-mix", "Mixtape/Street", "Demo", "Other", "Broadcast", "Interview", "Spokenword", "Audiobook",
)

private fun groupKey(rg: CatalogAlbum): String = rg.secondary.firstOrNull() ?: rg.type.ifEmpty { "Other" }

private fun groupRank(k: String): Int = GROUP_ORDER.indexOf(k).let { if (it < 0) 99 else it }

/** The page of an artist MusicBrainz knows, by its id (artist links, Discover, For you). */
@Composable
fun CatalogArtistScreen(nav: Nav, mbid: String) = ArtistPageScreen(nav, mbid = mbid, name = "")

/**
 * The page of an artist known by name only: the Library tab's artist grid, no network, or a name MusicBrainz doesn't
 * know. The same page with what the library has: the header with Play / Shuffle and the library's albums by that name
 * as its "In your library" section.
 */
@Composable
fun ArtistScreen(nav: Nav, name: String) = ArtistPageScreen(nav, mbid = "", name = name)

/** What the artist page shows: the MusicBrainz page (only the name for a library-only artist) and the library's albums by name. */
private class ArtistPageData(val page: ArtistPage, val libraryAlbums: List<Album>)

/**
 * The one artist page: header, popular songs, library albums, discography with filters, similar artists. By
 * MusicBrainz id it has everything; by name only, the header and the library's albums (the rest needs the id).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ArtistPageScreen(nav: Nav, mbid: String, name: String) {
    val byName = mbid.isEmpty()
    val features = nav.features
    val api = nav.graph.api
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val loader = rememberLoad("artist-page", mbid, name) {
        if (byName) ArtistPageData(ArtistPage(name = name), api.albums(sort = "released", artist = name, limit = 200))
        else ArtistPageData(features.artist(mbid), emptyList())
    }
    // photo, bio and popular songs; similar artists; releases MusicBrainz is missing – all optional, loaded after the page
    val extras = rememberLoad("catalog-extras", mbid) { if (byName) ArtistExtras() else try { features.artistExtras(mbid) } catch (_: Exception) { ArtistExtras() } }
    val similar = rememberLoad("catalog-similar", mbid) { if (byName) SimilarArtists() else try { features.artistSimilar(mbid) } catch (_: Exception) { SimilarArtists() } }
    val ext = rememberLoad("catalog-ext", mbid) { if (byName) emptyList() else try { features.artistCatalog(mbid) } catch (_: Exception) { emptyList() } }
    // by name: the years of the library's albums for the side scrubber, and as many albums to a row as fit (the
    // Library tab's grid: 140 dp tiles, at least two)
    val sections = if (byName) rememberLibraryIndex(api, "albums", "released", artist = name) else emptyList()
    val listState = rememberLazyListState()
    val libColumns = ((LocalConfiguration.current.screenWidthDp - 2 * Gutter.value + 14f) / (140f + 14f)).toInt().coerceAtLeast(2)
    val statuses = rememberRequestStatuses()
    LiveRequestStatuses(nav, statuses)
    var filter by remember { mutableStateOf(0) }
    var confirmMissing by remember { mutableStateOf<Pair<String, List<CatalogAlbum>>?>(null) }
    var extTracks by remember { mutableStateOf<ExtRelease?>(null) }
    var moreLike by remember { mutableStateOf<Pair<String, String>?>(null) }
    var discoSheet by remember { mutableStateOf(false) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()

    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = {
            if (byName) {
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
            } else {
                EncoreTopBar("", scrollBehavior = scrollBehavior, onBack = nav.back)
            }
        },
    ) { padding ->
        LoadableContent(loader) { d ->
            val a = d.page
            val x = (extras.state.value as? Loadable.Ready)?.value
            val sim = (similar.state.value as? Loadable.Ready)?.value
            val extReleases = (ext.state.value as? Loadable.Ready)?.value
            val extLoading = ext.state.value is Loadable.Loading
            val groups = a.releaseGroups.groupBy { groupKey(it) }
            val keys = groups.keys.sortedBy { groupRank(it) }
            val owned = a.releaseGroups.count { it.inLibrary }
            val firstCover = if (byName) d.libraryAlbums.firstOrNull()?.let { api.coverUrl(it.id, 400) } ?: ""
                else a.releaseGroups.firstOrNull { it.type == "Album" && it.secondary.isEmpty() }?.cover ?: ""
            val rgIds = a.releaseGroups.map { it.id }.toSet()
            val titles = a.releaseGroups.map { it.title.lowercase() }.toSet()
            val localOnly = a.libraryAlbums.filter { l -> !(l.rgid.isNotEmpty() && l.rgid in rgIds) && l.album.lowercase() !in titles }
            val artistKeys = listOf("ar:${a.id}")
            val status = statuses.statusOf(artistKeys, a.requestStatus)
            val statusText = statuses.labelOf(artistKeys, a.requestLabel)
            // 0.31: the songs liked from this artist and the artist's songs in the library (hidden on older servers)
            // (a catalogue artist with nothing in the library asks for neither)
            val libName = if (byName) name else if (a.libraryTracks > 0) a.name else ""
            val likedLoad = rememberLoad("artist-liked", libName, mbid) {
                if (libName.isEmpty()) ArtistSongs.EMPTY else try { api.artistLiked(libName, mbid) } catch (_: Exception) { ArtistSongs.EMPTY }
            }
            val songsLoad = rememberLoad("artist-songs", libName, mbid) {
                if (libName.isEmpty()) ArtistSongs.EMPTY else try { api.artistSongs(libName, mbid, 0, ARTIST_SONGS_FIRST) } catch (_: Exception) { ArtistSongs.EMPTY }
            }
            val liked = (likedLoad.state.value as? Loadable.Ready)?.value ?: ArtistSongs.EMPTY
            val firstSongs = (songsLoad.state.value as? Loadable.Ready)?.value ?: ArtistSongs.EMPTY
            // "Show all N": every song, loaded page by page into this list
            var allSongs by remember(libName, mbid) { mutableStateOf<List<Track>?>(null) }
            var loadingAll by remember(libName, mbid) { mutableStateOf(false) }
            val songs = allSongs ?: firstSongs.tracks
            val likedIds by nav.graph.likes.liked.collectAsState()
            val nowPlaying by nav.player.nowPlaying.collectAsState()
            val libAlbumCount = if (byName) d.libraryAlbums.size else a.libraryAlbums.size

            fun showAllSongs() {
                if (loadingAll || allSongs != null) return
                loadingAll = true
                scope.launch {
                    val out = ArrayList<Track>()
                    try {
                        withContext(Dispatchers.IO) {
                            while (out.size < ARTIST_SONGS_MAX) {
                                val page = api.artistSongs(libName, mbid, out.size, ARTIST_SONGS_PAGE)
                                out.addAll(page.tracks)
                                if (page.tracks.size < ARTIST_SONGS_PAGE || out.size >= page.total) break
                            }
                        }
                        allSongs = out
                    } catch (e: kotlinx.coroutines.CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "Couldn't load the songs", "err")
                    } finally {
                        loadingAll = false
                    }
                }
            }

            fun playAll(shuffle: Boolean) {
                scope.launch {
                    // by name: every spelling, alias and collaboration of the artist in the library (GET /api/library/artist-tracks)
                    val songs = try {
                        withContext(Dispatchers.IO) { if (byName) api.artistTracks(name) else features.artistLibraryTracks(a.id, a.name) }
                    } catch (e: Exception) { Toasts.show(e.message ?: "Couldn't load the songs", "err"); return@launch }
                    if (songs.isEmpty()) Toasts.show("Nothing by this artist in your library", "err") else nav.player.play(songs, shuffle = shuffle)
                }
            }

            Box(Modifier.fillMaxSize().padding(padding)) {
                LazyColumn(Modifier.fillMaxSize(), state = listState) {
                    // .detail-head.artist
                    item {
                        Column(Modifier.fillMaxWidth().padding(horizontal = Gutter), horizontalAlignment = Alignment.CenterHorizontally) {
                            RoundCover((x?.image?.ifEmpty { null }) ?: firstCover.ifEmpty { null }, nav.graph.imageLoader, size = 150.dp, label = a.name)
                            Spacer(Modifier.height(14.dp))
                            Text(
                                if (byName) "ARTIST · IN YOUR LIBRARY"
                                else listOf(a.type.ifEmpty { "Artist" }, a.country, if (a.begin.isNotEmpty()) "since ${a.begin.take(4)}" else "").filter { it.isNotEmpty() }.joinToString(" · ").uppercase(),
                                style = MaterialTheme.typography.labelSmall,
                                color = p.muted,
                                letterSpacing = 1.2.sp,
                            )
                            Text(a.name, style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 6.dp))
                            if (byName) {
                                val albums = d.libraryAlbums
                                Text(
                                    "${albums.size} album${if (albums.size == 1) "" else "s"} · ${albums.sumOf { it.trackCount }} tracks",
                                    style = MaterialTheme.typography.bodyMedium,
                                    color = p.muted,
                                    modifier = Modifier.padding(top = 2.dp),
                                )
                            }
                            if (a.disambiguation.isNotEmpty()) {
                                Text(a.disambiguation, style = MaterialTheme.typography.bodyMedium, color = p.muted, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 4.dp))
                            }
                            if (a.tags.isNotEmpty()) {
                                Row(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                    for (t in a.tags.take(5)) Badge(t, onClick = { nav.open(Screen.Tag(t)) })
                                }
                            }
                            Row(Modifier.padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                if (byName || a.libraryTracks > 0) {
                                    EncoreButton("Play", iconRes = R.drawable.ic_play, primary = true) { playAll(false) }
                                    EncoreButton("Shuffle", iconRes = R.drawable.ic_shuffle) { playAll(true) }
                                }
                            }
                            // requests, the counts and the links need MusicBrainz
                            if (!byName) Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                                if (status.isNotEmpty()) {
                                    LiveBadge(status, statusText)
                                } else {
                                    EncoreButton("Request discography", iconRes = R.drawable.ic_download) {
                                        // the releases to tick, for this request (older servers have no release types: the plain request)
                                        if (a.releaseKinds.isNotEmpty()) {
                                            discoSheet = true
                                        } else {
                                            scope.launch {
                                                val st = requestDiscography(features, a, firstCover, null, null) ?: return@launch

                                                statuses.mark(artistKeys, st, if (st == "pending") "Awaiting approval" else "Discography requested")
                                            }
                                        }
                                    }
                                }
                            }
                            if (!byName) Row(Modifier.padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                                Badge("$owned / ${a.releaseGroups.size} releases in library", kind = if (owned > 0) "ok" else "")
                                if (a.libraryTracks > 0) Badge("${a.libraryTracks} tracks")
                            }
                            if (!byName) Row(
                                Modifier.padding(top = 8.dp).fillMaxWidth(),
                                horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
                            ) {
                                SmallButton("MusicBrainz", iconRes = R.drawable.ic_open_in_new) { openUrl(context, "https://musicbrainz.org/artist/${a.id}") }
                                for ((k, u) in a.links.take(2)) SmallButton(k) { openUrl(context, u) }
                            }
                        }
                    }
                    // bio (Wikipedia / Deezer)
                    if (x != null && x.bio.isNotEmpty()) {
                        item {
                            var open by remember { mutableStateOf(false) }
                            Box(Modifier.padding(start = Gutter, end = Gutter, top = 18.dp)) {
                                CardBox {
                                    Column {
                                        Text(x.bio, style = MaterialTheme.typography.bodyMedium, color = p.text, maxLines = if (open) Int.MAX_VALUE else 4, overflow = TextOverflow.Ellipsis)
                                        Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                                            SmallButton(if (open) "Show less" else "Show more") { open = !open }
                                            if (x.bioUrl.isNotEmpty()) {
                                                Text("Wikipedia", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.clickable { openUrl(context, x.bioUrl) })
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // popular songs (Deezer)
                    if (x != null && x.top.isNotEmpty()) {
                        item { SectionHead("Popular") }
                        val playable = x.top.mapNotNull { it.library }
                        items(x.top.size, key = { i -> "top-$i" }) { i ->
                            val t = x.top[i]
                            TopTrackRow(nav, a.name, t, statuses, onPlay = { lib -> nav.player.play(playable, playable.indexOf(lib).coerceAtLeast(0)) }, onMore = { moreLike = a.name to t.title })
                        }
                    }
                    // liked from this artist (0.31): song rows that play, Play all; hidden when none
                    if (liked.tracks.isNotEmpty()) {
                        item(key = "liked-head") {
                            SectionHead(
                                "Liked from this artist",
                                subtitle = plural(liked.total, "song"),
                                action = "Play all",
                                onAction = { nav.player.play(liked.tracks, 0) },
                            )
                        }
                        items(liked.tracks.size, key = { i -> "liked-$i-${liked.tracks[i].id}" }) { i ->
                            val t = liked.tracks[i]
                            TrackRow(
                                track = t,
                                imageLoader = nav.graph.imageLoader,
                                coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                                liked = t.id in likedIds,
                                playing = nowPlaying?.id == t.id,
                                onClick = { nav.player.play(liked.tracks, i) },
                                onLike = { nav.graph.likes.toggle(t.id) },
                                subtitle = t.shownAlbum().ifEmpty { t.shownArtist() },
                                onMore = { nav.trackMenu(t) },
                            )
                        }
                    }
                    // in your library: the artist's songs (the first ten, Show all N) and albums
                    if (songs.isNotEmpty() || libAlbumCount > 0) {
                        item(key = "lib-head") {
                            SectionHead(
                                "In your library",
                                subtitle = listOfNotNull(
                                    if (firstSongs.total > 0) plural(firstSongs.total, "song") else null,
                                    if (libAlbumCount > 0) plural(libAlbumCount, "album") else null,
                                ).joinToString(" · "),
                            )
                        }
                    }
                    if (songs.isNotEmpty()) {
                        val more = allSongs == null && firstSongs.total > songs.size
                        item(key = "lib-songs-head") {
                            SubHead("Songs", action = if (more) (if (loadingAll) "Loading…" else "Show all ${firstSongs.total}") else null, onAction = { showAllSongs() })
                        }
                        items(songs.size, key = { i -> "lib-song-$i-${songs[i].id}" }) { i ->
                            val t = songs[i]
                            TrackRow(
                                track = t,
                                imageLoader = nav.graph.imageLoader,
                                coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                                liked = t.id in likedIds,
                                playing = nowPlaying?.id == t.id,
                                onClick = { nav.player.play(songs, i) },
                                onLike = { nav.graph.likes.toggle(t.id) },
                                subtitle = listOf(t.shownAlbum(), t.year.take(4)).filter { it.isNotEmpty() }.joinToString(" · "),
                                onMore = { nav.trackMenu(t) },
                            )
                        }
                        if (libAlbumCount > 0) item(key = "lib-albums-head") { SubHead("Albums") }
                    }
                    // albums in the library: by name, the library's own albums of the artist (the Library tab's grid)
                    if (byName && d.libraryAlbums.isNotEmpty()) {
                        tileRows(d.libraryAlbums, libColumns, key = "lib") { al ->
                            AlbumCard(al, api.coverUrl(al.id, 400), nav.graph.imageLoader) { nav.open(Screen.Album(al.id)) }
                        }
                    }
                    if (a.libraryAlbums.isNotEmpty()) {
                        tileRows(a.libraryAlbums, 2, key = "lib") { l ->
                            Tile(
                                title = l.album,
                                subtitle = listOf(l.albumArtist, l.year).filter { it.isNotEmpty() }.joinToString(" · "),
                                onClick = { nav.open(Screen.Album(l.albumId)) },
                            ) { m -> Cover(nav.graph.api.coverUrl(l.albumId, 400), nav.graph.imageLoader, m, corner = 12.dp, label = l.album) }
                        }
                    }
                    // discography with the All / Missing / In library filter (MusicBrainz's)
                    if (!byName) item { SectionHead("Discography", subtitle = "${a.releaseGroups.size}") }
                    if (!byName) item { ChoiceRow(listOf("All", "Missing", "In library"), filter, onSelect = { filter = it }) }
                    var any = false
                    for (k in keys) {
                        val all = groups[k] ?: continue
                        val list = all.filter { rg -> filter == 0 || (if (filter == 2) rg.inLibrary else !rg.inLibrary) }
                        if (list.isEmpty()) continue
                        any = true
                        val missing = all.filter { rg -> !rg.inLibrary && statuses.statusOf(albumKeys(rg), rg.requestStatus).isEmpty() }
                        item(key = "group-$k") {
                            SectionHead(
                                "${k}s",
                                subtitle = "${list.size}",
                                action = if (missing.isNotEmpty()) "Request ${missing.size} missing" else null,
                                onAction = if (missing.isNotEmpty()) ({ confirmMissing = k to missing }) else null,
                            )
                        }
                        tileRows(list, 2, key = "rg-$k") { rg ->
                            CatalogAlbumTile(
                                nav, rg, statuses,
                                subtitle = listOf(rg.date.ifEmpty { rg.year }, if (rg.secondary.isNotEmpty()) rg.type else "").filter { it.isNotEmpty() }.joinToString(" · "),
                            )
                        }
                    }
                    if (!any && !byName) {
                        item {
                            EmptyState(
                                R.drawable.ic_disc,
                                if (a.releaseGroups.isNotEmpty()) (if (filter == 2) "Nothing from this artist in the library yet" else "You have everything 🎉") else "No releases on MusicBrainz",
                            )
                        }
                    }
                    // releases Deezer / Apple Music / Bandcamp / Discogs know and MusicBrainz doesn't
                    if (extLoading) {
                        item {
                            Row(Modifier.padding(horizontal = Gutter, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                                Spinner()
                                Spacer(Modifier.width(8.dp))
                                Text("Checking Deezer and Apple Music for releases MusicBrainz is missing…", style = MaterialTheme.typography.bodySmall, color = p.muted)
                            }
                        }
                    } else if (!extReleases.isNullOrEmpty()) {
                        val found = extReleases.flatMap { r -> r.sources.map { it.first } }.distinct()
                        val foundText = if (found.size > 1) found.dropLast(1).joinToString(", ") + " and " + found.last() else found.joinToString()
                        item { SectionHead("Not in MusicBrainz", subtitle = "${extReleases.size}") }
                        item {
                            NoteText(
                                "Found on $foundText. Requests download from Soulseek, streamrip, YouTube Music or Bandcamp; files are tagged from the source's data. " +
                                    "Add them to MusicBrainz with Harmony and Encore re-tags them automatically.",
                                Modifier.padding(bottom = 6.dp),
                            )
                        }
                        tileRows(extReleases, 2, key = "ext") { m -> ExtReleaseTile(nav, a.name, m, statuses, onOpen = { extTracks = m }) }
                    }
                    // similar artists
                    if (sim != null && sim.artists.isNotEmpty()) {
                        item { SectionHead("Similar artists", subtitle = sim.source) }
                        tileRows(sim.artists, 3, key = "similar") { s -> CatalogArtistTile(nav, s, subtitle = " ") }
                    }
                    if (localOnly.isNotEmpty()) {
                        item { SectionHead("Only in your library", subtitle = "${localOnly.size}") }
                        item { NoteText("Albums MusicBrainz doesn't list for this artist (or not matched yet).", Modifier.padding(bottom = 6.dp)) }
                        tileRows(localOnly, 2, key = "local") { l ->
                            Tile(
                                title = l.album,
                                subtitle = listOf(l.albumArtist, l.year).filter { it.isNotEmpty() }.joinToString(" · "),
                                onClick = { nav.open(Screen.Album(l.albumId)) },
                            ) { m -> Cover(nav.graph.api.coverUrl(l.albumId, 400), nav.graph.imageLoader, m, corner = 12.dp, label = l.album) }
                        }
                    }
                    item { Spacer(Modifier.height(24.dp)) }
                }
                if (byName && d.libraryAlbums.isNotEmpty()) {
                    // the library's albums start after the header, Liked from this artist, the section's title and its
                    // songs (by name there is no bio or Popular)
                    val likedRows = if (liked.tracks.isNotEmpty()) 1 + liked.tracks.size else 0
                    val songRows = if (songs.isNotEmpty()) 1 + songs.size + 1 else 0
                    val libStart = 1 + (if (x != null && x.bio.isNotEmpty()) 1 else 0) + (if (x != null && x.top.isNotEmpty()) 1 + x.top.size else 0) +
                        likedRows + 1 + songRows
                    val target = remember(listState, libStart, libColumns) {
                        ScrollerTarget(
                            firstVisible = { (listState.firstVisibleItemIndex - libStart).coerceAtLeast(0) * libColumns },
                            visibleCount = { listState.layoutInfo.visibleItemsInfo.size * libColumns },
                            scrolling = { listState.isScrollInProgress },
                            scrollTo = { listState.scrollToItem(libStart + it / libColumns) },
                        )
                    }
                    FastScroller(sections, target, extent = d.libraryAlbums.size, modifier = Modifier.align(Alignment.CenterEnd))
                }
            }

            if (discoSheet) {
                DiscographySheet(a, extReleases ?: emptyList(), statuses, onDismiss = { discoSheet = false }) { picked ->
                    discoSheet = false
                    scope.launch {
                        // a server from before 0.26 doesn't know release_ids: it gets the kinds of the ticked releases as
                        // this request's release types instead, and skips only the extras none of them belong to
                        val kinds = a.releaseKinds.map { it.key }.filter { k -> picked.any { it.primary == k } }
                        val skip = a.releaseSkip.map { it.key }.filter { k -> picked.none { it.kind == k } }
                        val filter = buildJsonObject {
                            put("kinds", JsonArray(kinds.map { JsonPrimitive(it) }))
                            put("skip", JsonArray(skip.map { JsonPrimitive(it) }))
                        }
                        val st = requestDiscography(features, a, firstCover, picked.map { it.id }, filter) ?: return@launch
                        statuses.mark(artistKeys, st, if (st == "pending") "Awaiting approval" else "Discography requested")
                    }
                }
            }


            confirmMissing?.let { (k, missing) ->
                ConfirmDialog(
                    title = "Request ${missing.size} ${k.lowercase()}(s)?",
                    text = missing.joinToString(" · ") { it.title },
                    okLabel = "Request",
                    onDismiss = { confirmMissing = null },
                ) {
                    scope.launch {
                        var ok = 0
                        for (rg in missing) {
                            try {
                                val s = withContext(Dispatchers.IO) { features.createRequest(albumRequestBody(rg)) }
                                statuses.mark(albumKeys(rg), s.ifEmpty { "queued" })
                                ok++
                            } catch (e: Exception) {
                                Toasts.show(e.message ?: "Request failed", "err")
                                if (isLimitError(e)) break
                            }
                        }
                        Toasts.show("$ok release(s) requested", "ok")
                    }
                }
            }
            extTracks?.let { m -> ExtTracksSheet(nav, a.name, m, statuses, artistMbid = a.id, onDismiss = { extTracks = null }) }
            moreLike?.let { (artist, title) -> MoreLikeThisSheet(nav, artist, title, "", onDismiss = { moreLike = null }) }
        }
    }
}

/** "In your library": the first songs shown, the page size of "Show all N", and the most it loads. */
private const val ARTIST_SONGS_FIRST = 10
private const val ARTIST_SONGS_PAGE = 200
private const val ARTIST_SONGS_MAX = 5000

private fun plural(n: Int, what: String) = "$n $what${if (n == 1) "" else "s"}"

/** A heading inside a section ("Songs", "Albums" under "In your library") with an optional small button. */
@Composable
private fun SubHead(title: String, action: String? = null, onAction: (() -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 6.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
        if (action != null && onAction != null) SmallButton(action, onClick = onAction)
    }
}

/** One of Deezer's popular songs of the artist: play when owned, else "Track" requests it; the spark finds more like it. */
@Composable
private fun TopTrackRow(nav: Nav, artistName: String, t: TopTrack, statuses: RequestStatuses, onPlay: (Track) -> Unit, onMore: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val key = "top:" + t.title + t.version
    val keys = listOf(songNameKey(artistName, t.title + if (t.version.isNotEmpty()) " " + t.version else ""), key)
    val status = statuses.statusOf(keys, "")
    val lib = t.library
    Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        PlayableCover(nav, t.cover.ifEmpty { null }, t.album, 40.dp, lib != null) { if (lib != null) onPlay(lib) }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Row {
                Text(t.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                if (t.version.isNotEmpty()) Text(" " + t.version, style = MaterialTheme.typography.bodyMedium, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Text(t.album, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (t.durationSec > 0) Text(formatDuration((t.durationSec * 1000).toLong()), style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(horizontal = 6.dp))
        GhostIconButton(R.drawable.ic_spark, "More like this", onClick = onMore)
        when {
            lib != null -> Badge("In library", kind = "ok")
            status.isNotEmpty() -> LiveBadge(status, statuses.labelOf(keys, ""))
            else -> SmallButton("Track", iconRes = R.drawable.ic_add) {
                scope.launch {
                    val body = buildJsonObject {
                        put("kind", "track")
                        put("artist", artistName)
                        put("title", t.title + if (t.version.isNotEmpty()) " " + t.version else "")
                        if (t.album.isNotEmpty()) put("album", t.album)
                        if (t.cover.isNotEmpty()) put("cover_url", t.cover)
                        if (t.durationSec > 0) put("length", t.durationSec)
                        if (t.isrc.isNotEmpty()) put("isrc", t.isrc)
                        put("album_mode", "song")
                    }
                    submitRequest(nav.features, body)?.let { statuses.mark(keys, it) }
                }
            }
        }
    }
}

/** Every key a release outside MusicBrainz is known by in request events, the live ones first. */
private fun extReleaseKeys(artistName: String, m: ExtRelease): List<String> =
    listOf(extKeyOf(m.ext), albumNameKey(artistName, m.title), "ext:" + m.title).filter { it.isNotEmpty() }

/** A release outside MusicBrainz: badge, request / play, the sources, "Add to MB" (Harmony) and "Pick". */
@Composable
private fun ExtReleaseTile(nav: Nav, artistName: String, m: ExtRelease, statuses: RequestStatuses, onOpen: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val keys = extReleaseKeys(artistName, m)
    val status = statuses.statusOf(keys, m.requestStatus)
    val body = buildJsonObject {
        put("kind", "album")
        put("artist", artistName)
        put("title", m.title)
        put("album", m.title)
        if (m.year.isNotEmpty()) put("year", m.year)
        if (m.cover.isNotEmpty()) put("cover_url", m.cover)
        if (m.ext !is JsonNull) put("ext", m.ext)
    }
    Column {
        Tile(
            title = m.title,
            subtitle = listOf(m.date.ifEmpty { m.year }, m.type, if (m.tracks > 0) "${m.tracks} tracks" else "").filter { it.isNotEmpty() }.joinToString(" · "),
            onClick = { if (m.libraryAlbumId.isNotEmpty()) nav.open(Screen.Album(m.libraryAlbumId)) else onOpen() },
        ) { mod ->
            Box(mod) {
                Cover(m.cover.ifEmpty { null }, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 12.dp, label = m.title)
                when {
                    m.inLibrary -> Badge("In library", kind = "ok", modifier = Modifier.padding(8.dp))
                    status.isNotEmpty() -> LiveBadge(status, statuses.labelOf(keys, m.requestLabel), Modifier.padding(8.dp))
                    else -> Badge("Not in MusicBrainz", kind = "warn", modifier = Modifier.padding(8.dp))
                }
                when {
                    m.libraryAlbumId.isNotEmpty() -> ArtFab(R.drawable.ic_play, "Play", Modifier.align(Alignment.BottomEnd).padding(8.dp)) { playLibraryAlbum(nav, m.libraryAlbumId, scope) }
                    !m.inLibrary && status.isEmpty() -> ArtFab(R.drawable.ic_add, "Request", Modifier.align(Alignment.BottomEnd).padding(8.dp)) {
                        scope.launch { submitRequest(nav.features, body)?.let { statuses.mark(keys, it) } }
                    }
                }
            }
        }
        Row(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for ((name, url) in m.sources.take(2)) {
                Text(name, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.clickable { openUrl(context, url) })
            }
            if (m.harmony.isNotEmpty()) Text("Add to MB", style = MaterialTheme.typography.bodySmall, color = p.text, modifier = Modifier.clickable { openUrl(context, m.harmony) })
            Text("Pick", style = MaterialTheme.typography.bodySmall, color = p.text, modifier = Modifier.clickable {
                nav.open(Screen.Picker(PickerOpts(kind = "album", artist = artistName, title = m.title, album = m.title, year = m.year, coverUrl = m.cover, trackCount = m.tracks, ext = m.ext)))
            })
        }
    }
}

/**
 * The track list of a release outside MusicBrainz (the web's modal): songs in the library play (Play / Shuffle
 * the owned ones in album order), missing ones can be requested, badges follow the requests.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ExtTracksSheet(nav: Nav, artistName: String, m: ExtRelease, statuses: RequestStatuses, artistMbid: String = "", onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val loader = rememberLoad("ext-tracks", m.title) { nav.features.catalogTracks(m.ext, artistName) }
    val releaseStatus = statuses.statusOf(extReleaseKeys(artistName, m), m.requestStatus)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().padding(bottom = 24.dp)) {
            Text(m.title, style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 22.dp, vertical = 8.dp))
            // Deezer / Apple albums: the MusicBrainz release editor, filled in, in the browser (a signed link: no sign-in there)
            val seedable = (m.ext as? kotlinx.serialization.json.JsonObject)?.let { o -> listOf("deezer", "apple").any { k -> o[k] != null && o[k] !is JsonNull } } == true
            if (seedable) {
                Row(Modifier.padding(horizontal = 22.dp, vertical = 4.dp)) {
                    EncoreButton("Add to MusicBrainz", iconRes = R.drawable.ic_open_in_new) {
                        scope.launch {
                            try {
                                val url = withContext(Dispatchers.IO) { nav.features.mbSeedLink(m.ext, artistMbid, artistName) }
                                if (url != null) openUrl(context, url) else Toasts.show("This album is not on Deezer or Apple Music", "err")
                            } catch (e: kotlinx.coroutines.CancellationException) {
                                throw e
                            } catch (e: Exception) {
                                Toasts.show(e.message ?: "Could not open MusicBrainz", "err")
                            }
                        }
                    }
                }
            }
            when (val s = loader.state.value) {
                is Loadable.Loading -> Box(Modifier.fillMaxWidth().height(120.dp), contentAlignment = Alignment.Center) { Spinner(24.dp) }
                is Loadable.Failed -> Text(s.message, color = p.muted, modifier = Modifier.padding(horizontal = 22.dp, vertical = 12.dp))
                is Loadable.Ready -> {
                    val tracks: List<ExtTrack> = s.value
                    val owned = tracks.filter { it.library != null }
                    val queue = owned.mapNotNull { it.library }
                    if (tracks.isEmpty()) {
                        Text("No track list available.", color = p.muted, modifier = Modifier.padding(horizontal = 22.dp, vertical = 12.dp))
                    } else {
                        if (queue.isNotEmpty()) {
                            Row(Modifier.padding(horizontal = 22.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                EncoreButton("Play", iconRes = R.drawable.ic_play, primary = true) { nav.player.play(queue, 0) }
                                EncoreButton("Shuffle", iconRes = R.drawable.ic_shuffle) { nav.player.play(queue, 0, shuffle = true) }
                            }
                            NoteText("${owned.size} of ${tracks.size} songs in your library", Modifier.padding(bottom = 6.dp))
                        }
                        LazyColumn(Modifier.fillMaxWidth().height(if (tracks.size > 8) 420.dp else (tracks.size * 52 + 8).dp)) {
                            items(tracks.size) { i ->
                                val t = tracks[i]
                                val lib = t.library
                                val keys = listOf(songNameKey(t.artist.ifEmpty { artistName }, t.title))
                                val status = statuses.statusOf(keys, t.requestStatus)
                                Row(
                                    Modifier.fillMaxWidth().clickable(enabled = lib != null) { nav.player.play(queue, owned.indexOf(t).coerceAtLeast(0)) }
                                        .padding(start = 22.dp, end = 12.dp, top = 6.dp, bottom = 6.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    Text(if (t.track > 0) t.track.toString() else "", color = if (lib != null) p.accent else p.faint, textAlign = TextAlign.End, modifier = Modifier.width(24.dp))
                                    Spacer(Modifier.width(12.dp))
                                    Column(Modifier.weight(1f)) {
                                        Text(t.title, fontWeight = FontWeight.SemiBold, color = if (lib != null) p.text else p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                        if (t.artist.isNotEmpty()) Text(t.artist, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    }
                                    Text(formatDuration((t.lengthSec * 1000).toLong()), style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(horizontal = 6.dp))
                                    when {
                                        t.inLibrary -> Badge("In library", kind = "ok")
                                        status.isNotEmpty() -> LiveBadge(status, statuses.labelOf(keys, t.requestLabel))
                                        releaseStatus.isNotEmpty() -> {}
                                        else -> SmallButton("Track", iconRes = R.drawable.ic_add) {
                                            scope.launch {
                                                val body = buildJsonObject {
                                                    put("kind", "track")
                                                    put("artist", t.artist.ifEmpty { artistName })
                                                    put("title", t.title)
                                                    put("album", m.title)
                                                    if (m.cover.isNotEmpty()) put("cover_url", m.cover)
                                                    if (m.year.isNotEmpty()) put("year", m.year)
                                                    if (t.lengthSec > 0) put("length", t.lengthSec)
                                                    if (t.isrc.isNotEmpty()) put("isrc", t.isrc)
                                                    put("album_mode", "song")
                                                }
                                                submitRequest(nav.features, body)?.let { statuses.mark(keys, it) }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---- album (web/js/pages/discover.js renderAlbum) ----------------------------------------------------------------

/** The MusicBrainz page of a release group: header, Play / Request / Pick download, the best release's track list. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CatalogAlbumScreen(nav: Nav, rgid: String) {
    val features = nav.features
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val loader = rememberLoad("catalog-album", rgid) { features.album(rgid) }
    val statuses = rememberRequestStatuses()
    LiveRequestStatuses(nav, statuses)
    var moreLike by remember { mutableStateOf<Triple<String, String, String>?>(null) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()

    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("", scrollBehavior = scrollBehavior, onBack = nav.back) },
    ) { padding ->
        LoadableContent(loader) { a: AlbumPage ->
            val rel = a.release
            val tracks = rel?.tracks ?: emptyList()
            val total = tracks.sumOf { it.lengthSec }
            val multi = rel != null && rel.discCount > 1
            val owned = tracks.filter { it.library != null }
            val queue = owned.mapNotNull { it.library }
            val albumKeyList = listOf(a.id, albumNameKey(a.artist, a.title))
            val albumStatus = statuses.statusOf(albumKeyList, a.requestStatus)

            fun playOwned(i: Int, shuffle: Boolean = false) {
                if (queue.isEmpty()) { Toasts.show("None of these songs are in your library yet", "err"); return }
                nav.player.play(queue, i.coerceAtLeast(0), shuffle = shuffle)
            }

            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                item {
                    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter), horizontalAlignment = Alignment.CenterHorizontally) {
                        Box(Modifier.size(200.dp).shadow(24.dp, RoundedCornerShape(16.dp), clip = false)) {
                            Cover(a.cover.replace("front-250", "front-500").ifEmpty { null }, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 16.dp, label = a.title)
                        }
                        Spacer(Modifier.height(18.dp))
                        Text(
                            (a.type.ifEmpty { "Release" } + if (a.secondary.isNotEmpty()) " · " + a.secondary.joinToString(", ") else "").uppercase(),
                            style = MaterialTheme.typography.labelSmall, color = p.muted, letterSpacing = 1.2.sp,
                        )
                        Text(a.title, style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 6.dp))
                        Text(
                            a.artist,
                            style = MaterialTheme.typography.bodyLarge,
                            fontWeight = FontWeight.SemiBold,
                            modifier = Modifier.padding(top = 4.dp).clickable(enabled = a.artistId.isNotEmpty()) { nav.open(Screen.CatalogArtist(a.artistId)) },
                        )
                        Text(
                            listOf(a.year, if (rel != null) "${tracks.size} tracks" else "", if (rel != null && total > 0) formatLongDuration(total) else "", rel?.label ?: "")
                                .filter { it.isNotEmpty() }.joinToString(" · "),
                            style = MaterialTheme.typography.bodyMedium, color = p.muted, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 2.dp),
                        )
                        if (a.tags.isNotEmpty()) {
                            Row(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) { for (t in a.tags.take(5)) Badge(t) }
                        }
                        // songs you own play right here, in album order, even when only some of the album is in the library
                        Row(Modifier.padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            if (queue.isNotEmpty()) {
                                EncoreButton("Play", iconRes = R.drawable.ic_play, primary = true) { playOwned(0) }
                                EncoreButton("Shuffle", iconRes = R.drawable.ic_shuffle) { playOwned(0, true) }
                            }
                            if (a.inLibrary) {
                                if (a.libraryAlbumId.isEmpty()) Availability(true, "")
                            } else if (albumStatus.isNotEmpty()) {
                                LiveBadge(albumStatus, statuses.labelOf(albumKeyList, a.requestLabel))
                            } else {
                                EncoreButton("Request album", iconRes = R.drawable.ic_add, primary = true) {
                                    scope.launch {
                                        val body = buildJsonObject {
                                            put("kind", "album"); put("mbid", a.id); put("artist", a.artist); put("title", a.title); put("album", a.title)
                                            if (a.year.isNotEmpty()) put("year", a.year)
                                            if (a.cover.isNotEmpty()) put("cover_url", a.cover)
                                        }
                                        submitRequest(features, body)?.let { statuses.mark(albumKeyList, it) }
                                    }
                                }
                            }
                        }
                        if (queue.isNotEmpty() && queue.size < tracks.size) {
                            Text("${queue.size} of ${tracks.size} songs in your library", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(top = 6.dp))
                        }
                        Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            if (a.libraryAlbumId.isNotEmpty()) EncoreButton("Open in library", iconRes = R.drawable.ic_disc) { nav.open(Screen.Album(a.libraryAlbumId)) }
                            EncoreButton("Pick download", iconRes = R.drawable.ic_search) {
                                nav.open(Screen.Picker(PickerOpts(
                                    kind = "album", mbid = a.id, artist = a.artist, artistMbid = a.artistId, title = a.title, album = a.title,
                                    year = a.year, coverUrl = a.cover, trackCount = tracks.size,
                                )))
                            }
                        }
                        Row(Modifier.padding(top = 8.dp)) {
                            SmallButton("MusicBrainz", iconRes = R.drawable.ic_open_in_new) { openUrl(context, "https://musicbrainz.org/release-group/${a.id}") }
                        }
                        Spacer(Modifier.height(12.dp))
                    }
                }
                if (rel == null) {
                    item { EmptyState(R.drawable.ic_disc, "No track listing available") }
                } else {
                    items(tracks.size, key = { i -> "rt-$i" }) { i ->
                        val t = tracks[i]
                        val lib = t.library
                        val keys = listOf(t.recordingId, songNameKey(t.artist, t.title)).filter { it.isNotEmpty() }
                        val status = statuses.statusOf(keys, t.requestStatus)
                        Row(
                            Modifier.fillMaxWidth().clickable(enabled = lib != null) { playOwned(owned.indexOf(t)) }.padding(start = Gutter, end = 4.dp, top = 6.dp, bottom = 6.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                if (multi) "${t.disc}-${t.track}" else t.track.toString(),
                                style = MaterialTheme.typography.bodyMedium,
                                color = if (lib != null) p.accent else p.faint,
                                textAlign = TextAlign.End,
                                modifier = Modifier.width(34.dp),
                            )
                            Spacer(Modifier.width(12.dp))
                            Column(Modifier.weight(1f)) {
                                Text(t.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, color = if (lib != null) p.text else p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                if (t.artist.isNotEmpty() && t.artist != a.artist) Text(t.artist, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                            Text(formatDuration((t.lengthSec * 1000).toLong()), style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(horizontal = 6.dp))
                            GhostIconButton(R.drawable.ic_spark, "More like this", onClick = { moreLike = Triple(t.artist, t.title, t.recordingId) })
                            GhostIconButton(R.drawable.ic_search, "Pick a download for this song", onClick = {
                                nav.open(Screen.Picker(PickerOpts(
                                    kind = "track", mbid = t.recordingId, artist = t.artist, title = t.title, album = a.title, year = a.year, coverUrl = a.cover, lengthSec = t.lengthSec,
                                )))
                            })
                            when {
                                t.inLibrary -> Badge("In library", kind = "ok", modifier = Modifier.padding(end = 8.dp))
                                status.isNotEmpty() -> LiveBadge(status, statuses.labelOf(keys, t.requestLabel), Modifier.padding(end = 8.dp))
                                albumStatus.isNotEmpty() -> {}
                                else -> SmallButton("Track", iconRes = R.drawable.ic_add, modifier = Modifier.padding(end = 8.dp)) {
                                    scope.launch {
                                        val body = buildJsonObject {
                                            put("kind", "track"); put("mbid", t.recordingId); put("artist", t.artist); put("title", t.title); put("album", a.title)
                                            if (a.cover.isNotEmpty()) put("cover_url", a.cover)
                                            if (a.year.isNotEmpty()) put("year", a.year)
                                            if (t.lengthSec > 0) put("length", t.lengthSec)
                                            if (t.isrc.isNotEmpty()) put("isrc", t.isrc)
                                            put("album_mode", "song")
                                        }
                                        submitRequest(features, body)?.let { statuses.mark(keys, it) }
                                    }
                                }
                            }
                        }
                    }
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
            moreLike?.let { (artist, title, mbid) -> MoreLikeThisSheet(nav, artist, title, mbid, onDismiss = { moreLike = null }) }
        }
    }
}
