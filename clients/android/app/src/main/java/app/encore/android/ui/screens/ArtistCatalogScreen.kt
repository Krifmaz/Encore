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
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.R
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
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.Loadable
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.RoundCover
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Tile
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.components.formatLongDuration
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// ---- artist (web/js/pages/discover.js renderArtist) --------------------------------------------------------------

/** The order the discography's groups come in. */
private val GROUP_ORDER = listOf(
    "Album", "EP", "Single", "Compilation", "Live", "Soundtrack", "Remix", "DJ-mix", "Mixtape/Street", "Demo", "Other", "Broadcast", "Interview", "Spokenword", "Audiobook",
)

private fun groupKey(rg: CatalogAlbum): String = rg.secondary.firstOrNull() ?: rg.type.ifEmpty { "Other" }

private fun groupRank(k: String): Int = GROUP_ORDER.indexOf(k).let { if (it < 0) 99 else it }

/** The MusicBrainz page of an artist: header, popular songs, library albums, discography with filters, similar artists. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CatalogArtistScreen(nav: Nav, mbid: String) {
    val features = nav.features
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val loader = rememberLoad("catalog-artist", mbid) { features.artist(mbid) }
    // photo, bio and popular songs; similar artists; releases MusicBrainz is missing – all optional, loaded after the page
    val extras = rememberLoad("catalog-extras", mbid) { try { features.artistExtras(mbid) } catch (_: Exception) { ArtistExtras("", "", "", emptyList()) } }
    val similar = rememberLoad("catalog-similar", mbid) { try { features.artistSimilar(mbid) } catch (_: Exception) { SimilarArtists("", emptyList()) } }
    val ext = rememberLoad("catalog-ext", mbid) { try { features.artistCatalog(mbid) } catch (_: Exception) { emptyList() } }
    val statuses = rememberRequestStatuses()
    LiveRequestStatuses(nav, statuses)
    var filter by remember { mutableStateOf(0) }
    var confirmMissing by remember { mutableStateOf<Pair<String, List<CatalogAlbum>>?>(null) }
    var extTracks by remember { mutableStateOf<ExtRelease?>(null) }
    var moreLike by remember { mutableStateOf<Pair<String, String>?>(null) }
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()

    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("", scrollBehavior = scrollBehavior, onBack = nav.back) },
    ) { padding ->
        LoadableContent(loader) { a ->
            val x = (extras.state.value as? Loadable.Ready)?.value
            val sim = (similar.state.value as? Loadable.Ready)?.value
            val extReleases = (ext.state.value as? Loadable.Ready)?.value
            val extLoading = ext.state.value is Loadable.Loading
            val groups = a.releaseGroups.groupBy { groupKey(it) }
            val keys = groups.keys.sortedBy { groupRank(it) }
            val owned = a.releaseGroups.count { it.inLibrary }
            val firstCover = a.releaseGroups.firstOrNull { it.type == "Album" && it.secondary.isEmpty() }?.cover ?: ""
            val rgIds = a.releaseGroups.map { it.id }.toSet()
            val titles = a.releaseGroups.map { it.title.lowercase() }.toSet()
            val localOnly = a.libraryAlbums.filter { l -> !(l.rgid.isNotEmpty() && l.rgid in rgIds) && l.album.lowercase() !in titles }
            val artistKeys = listOf("ar:${a.id}")
            val status = statuses.statusOf(artistKeys, a.requestStatus)
            val statusText = statuses.labelOf(artistKeys, a.requestLabel)

            fun playAll(shuffle: Boolean) {
                scope.launch {
                    val songs = try { withContext(Dispatchers.IO) { features.artistLibraryTracks(a.id, a.name) } } catch (e: Exception) { Toasts.show(e.message ?: "Couldn't load the songs", "err"); return@launch }
                    if (songs.isEmpty()) Toasts.show("Nothing by this artist in your library", "err") else nav.player.play(songs, shuffle = shuffle)
                }
            }

            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                // .detail-head.artist
                item {
                    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter), horizontalAlignment = Alignment.CenterHorizontally) {
                        RoundCover((x?.image?.ifEmpty { null }) ?: firstCover.ifEmpty { null }, nav.graph.imageLoader, size = 150.dp, label = a.name)
                        Spacer(Modifier.height(14.dp))
                        Text(
                            listOf(a.type.ifEmpty { "Artist" }, a.country, if (a.begin.isNotEmpty()) "since ${a.begin.take(4)}" else "").filter { it.isNotEmpty() }.joinToString(" · ").uppercase(),
                            style = MaterialTheme.typography.labelSmall,
                            color = p.muted,
                            letterSpacing = 1.2.sp,
                        )
                        Text(a.name, style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 6.dp))
                        if (a.disambiguation.isNotEmpty()) {
                            Text(a.disambiguation, style = MaterialTheme.typography.bodyMedium, color = p.muted, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 4.dp))
                        }
                        if (a.tags.isNotEmpty()) {
                            Row(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                for (t in a.tags.take(5)) Badge(t, onClick = { nav.open(Screen.Tag(t)) })
                            }
                        }
                        Row(Modifier.padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            if (a.libraryTracks > 0) {
                                EncoreButton("Play", iconRes = R.drawable.ic_play, primary = true) { playAll(false) }
                                EncoreButton("Shuffle", iconRes = R.drawable.ic_shuffle) { playAll(true) }
                            }
                        }
                        Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                            if (status.isNotEmpty()) {
                                LiveBadge(status, statusText)
                            } else {
                                EncoreButton("Request discography", iconRes = R.drawable.ic_download) {
                                    scope.launch {
                                        val body = buildJsonObject {
                                            put("kind", "artist")
                                            put("mbid", a.id)
                                            put("artist", a.name)
                                            if (firstCover.isNotEmpty()) put("cover_url", firstCover)
                                        }
                                        val st = submitRequest(features, body) ?: return@launch
                                        val pending = st == "pending"
                                        statuses.mark(artistKeys, st, if (pending) "Awaiting approval" else "Discography requested")
                                        // the server decides which albums and EPs (and singles) it queues (title and year dedupe,
                                        // library check); each one arrives as a request event and marks its tile
                                    }
                                }
                            }
                        }
                        Row(Modifier.padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                            Badge("$owned / ${a.releaseGroups.size} releases in library", kind = if (owned > 0) "ok" else "")
                            if (a.libraryTracks > 0) Badge("${a.libraryTracks} tracks")
                        }
                        Row(
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
                // albums in the library
                if (a.libraryAlbums.isNotEmpty()) {
                    item { SectionHead("In your library", subtitle = "${a.libraryAlbums.size}") }
                    tileRows(a.libraryAlbums, 2, key = "lib") { l ->
                        Tile(
                            title = l.album,
                            subtitle = listOf(l.albumArtist, l.year).filter { it.isNotEmpty() }.joinToString(" · "),
                            onClick = { nav.open(Screen.Album(l.albumId)) },
                        ) { m -> Cover(nav.graph.api.coverUrl(l.albumId, 400), nav.graph.imageLoader, m, corner = 12.dp, label = l.album) }
                    }
                }
                // discography with the All / Missing / In library filter
                item { SectionHead("Discography", subtitle = "${a.releaseGroups.size}") }
                item { ChoiceRow(listOf("All", "Missing", "In library"), filter, onSelect = { filter = it }) }
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
                if (!any) {
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
            extTracks?.let { m -> ExtTracksSheet(nav, a.name, m, statuses, onDismiss = { extTracks = null }) }
            moreLike?.let { (artist, title) -> MoreLikeThisSheet(nav, artist, title, "", onDismiss = { moreLike = null }) }
        }
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
private fun ExtTracksSheet(nav: Nav, artistName: String, m: ExtRelease, statuses: RequestStatuses, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val loader = rememberLoad("ext-tracks", m.title) { nav.features.catalogTracks(m.ext, artistName) }
    val releaseStatus = statuses.statusOf(extReleaseKeys(artistName, m), m.requestStatus)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().padding(bottom = 24.dp)) {
            Text(m.title, style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 22.dp, vertical = 8.dp))
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
