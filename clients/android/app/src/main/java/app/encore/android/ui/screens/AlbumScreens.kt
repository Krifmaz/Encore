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
import androidx.compose.foundation.lazy.LazyColumn
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
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
import app.encore.android.core.dbl
import app.encore.android.core.str
import app.encore.android.data.api.Album
import app.encore.android.data.api.AlbumDetail
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.data.api.Tr
import app.encore.android.features.offline.rememberOfflineMenu
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.Loadable
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.ReportUi
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.formatDay
import app.encore.android.ui.components.formatLongDuration
import app.encore.android.ui.components.formatSize
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.components.shareLinkOf
import app.encore.android.ui.theme.palette
import app.encore.android.data.repo.Me
import app.encore.android.features.library.AlbumArtworkSheet
import app.encore.android.features.library.AlbumEditSheet
import app.encore.android.features.library.AlbumMatchSheet
import app.encore.android.features.library.LibraryFixRules
import app.encore.android.features.library.OnSongFixed
import app.encore.android.features.library.SongFixUi
import app.encore.android.features.library.VerifyBadge
import app.encore.android.ui.Screen
import app.encore.android.ui.components.MenuSeparator
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject

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

/**
 * What the album page shows beyond [AlbumDetail]: size, the history line, per-track "requested by" notes, and the
 * MusicBrainz release group ("" when the album has none) for "Get the rest of the album" and "Release info".
 */
private class AlbumPage(val detail: AlbumDetail, val size: Long, val history: String, val notes: Map<Long, String>, val rgid: String = "")

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
        title = o.str("album").ifEmpty { "Unknown album" },
        artist = o.str("albumartist"),
        year = o.str("year").take(4),
        trackCount = tracks.size,
        formats = tracks.map { it.format }.distinct().joinToString(","),
        titleTr = Tr.of(o, "album"),
        artistTr = Tr.of(o, "albumartist"),
    )
    val detail = AlbumDetail(album, tracks, o.dbl("duration"))

    val requesters = rows.map { it.str("requested_by") }.filter { it.isNotEmpty() }.distinct().joinToString(", ")
    val requestedAt = rows.map { it.dbl("requested_at") }.filter { it > 0 }.minOrNull()
    val addedAt = rows.map { it.dbl("added_at") }.filter { it > 0 }.minOrNull()
    val history = listOf(
        if (requesters.isNotEmpty()) "Requested by $requesters" + (if (requestedAt != null) " on ${formatDay(requestedAt)}" else "") else "",
        if (addedAt != null) (if (requesters.isNotEmpty()) "added " else "Added ") + formatDay(addedAt) else "",
    ).filter { it.isNotEmpty() }.joinToString(" · ")

    val notes = HashMap<Long, String>()
    for (r in rows) {
        val who = r.str("requested_by")
        if (who.isEmpty()) continue
        val at = r.dbl("requested_at")
        val id2 = r.dbl("id").toLong()
        notes[id2] = "requested by $who" + if (at > 0) " on ${formatDay(at)}" else ""
    }
    return AlbumPage(detail, o.dbl("size").toLong(), history, notes, o.str("mb_releasegroupid"))
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AlbumScreen(nav: Nav, albumId: String) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val loader = rememberLoad(albumId) { loadAlbumPage(nav, albumId) }
    // a song corrected or deleted from its menu, or the album changed from this page's menu: the page loads again
    // (in place, keeping the scroll position)
    OnSongFixed { loader.refresh() }
    // "Download for offline" / "Remove download" (features/offline)
    val offlineMenu = rememberOfflineMenu(nav)
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    var me by remember { mutableStateOf<Me?>(null) }
    LaunchedEffect(Unit) { me = try { withContext(Dispatchers.IO) { nav.features.me() } } catch (e: CancellationException) { throw e } catch (_: Exception) { null } }
    var confirmDelete by remember { mutableStateOf<Pair<String, Int>?>(null) }
    // the admin's sheets: "match" (Tag / match metadata), "edit" (Edit details), "artwork" (Change artwork…)
    var albumSheet by remember { mutableStateOf("") }
    // "Get the rest of the album": the album on Discover, where the songs that aren't here yet can be requested
    fun restOfAlbum(rgid: String, artist: String, title: String) {
        if (rgid.isNotEmpty()) { nav.open(Screen.CatalogAlbum(rgid)); return }
        scope.launch {
            try {
                val found = withContext(Dispatchers.IO) { nav.features.albumLookup(artist, title) }
                if (found.isNotEmpty()) nav.open(Screen.CatalogAlbum(found))
                else Toasts.show("MusicBrainz doesn't know this album – look it up in the Downloader instead", "warn")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "warn")
            }
        }
    }
    fun albumAction(block: suspend () -> Unit) {
        scope.launch {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }
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
                            album.shownTitle(),
                            style = MaterialTheme.typography.headlineMedium,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(top = 6.dp),
                        )
                        album.titleUnder()?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = p.muted, textAlign = TextAlign.Center) }
                        Text(
                            album.shownArtist(),
                            style = MaterialTheme.typography.bodyLarge,
                            fontWeight = FontWeight.SemiBold,
                            color = p.text,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(top = 6.dp).clickable { openArtistPage(nav, album.artist) },
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
                                    *offlineMenu.entries(tracks).toTypedArray(),
                                    MenuEntry("Share link", R.drawable.ic_share) { shareAlbum(album.title, album.artist) },
                                    if (me?.isAdmin == true) MenuEntry("All songs are right (confirm)", R.drawable.ic_check) {
                                        albumAction {
                                            withContext(Dispatchers.IO) { nav.features.verifyManual(tracks.map { it.id }) }
                                            Toasts.show("Album confirmed", "ok")
                                            SongFixUi.changed()
                                        }
                                    } else null,
                                    MenuEntry("Lyrics for all songs", R.drawable.ic_lyrics) {
                                        Toasts.show("Looking for lyrics…", "info")
                                        albumAction {
                                            val (found, checked) = withContext(Dispatchers.IO) { nav.features.albumLyrics(albumId) }
                                            Toasts.show(if (checked > 0) "Lyrics found for $found of $checked song(s)" else "Every song already has lyrics", "ok")
                                        }
                                    },
                                    MenuEntry("Get the rest of the album", R.drawable.ic_download) { restOfAlbum(page.rgid, album.artist, album.title) },
                                    if (page.rgid.isNotEmpty()) MenuEntry("Release info", R.drawable.ic_disc) { nav.open(Screen.CatalogAlbum(page.rgid)) } else null,
                                    MenuEntry("Report a problem", R.drawable.ic_flag) { ReportUi.album(albumId, album.title) { nav.back() } },
                                    if (me?.isAdmin == true) MenuSeparator else null,
                                    if (me?.isAdmin == true) MenuEntry("Tag / match metadata", R.drawable.ic_note) { albumSheet = "match" } else null,
                                    if (me?.isAdmin == true) MenuEntry("Edit details (album, artist, songs)", R.drawable.ic_edit) { albumSheet = "edit" } else null,
                                    if (me?.isAdmin == true) MenuEntry("Change artwork…", R.drawable.ic_disc) { albumSheet = "artwork" } else null,
                                    if (me?.isAdmin == true) MenuEntry("Find artwork automatically", R.drawable.ic_refresh) {
                                        albumAction {
                                            val n = withContext(Dispatchers.IO) { nav.features.findAlbumArtwork(albumId) }
                                            Toasts.show("Artwork embedded in $n file(s)", "ok")
                                            SongFixUi.changed()
                                        }
                                    } else null,
                                    if (me?.isAdmin == true) MenuSeparator else null,
                                    if (me?.isAdmin == true) MenuEntry("Delete album from disk", R.drawable.ic_delete, danger = true) {
                                        confirmDelete = album.title to tracks.size
                                    } else null,
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
                        subtitleLinks = if (t.artist.isNotEmpty() && t.artist != album.artist) rememberSongLinks(nav, t, withAlbum = false, suffix = " · $fmt") else null,
                        note = page.notes[t.id],
                        // the audio check's verdict: "wrong audio" marks the songs to correct (library.rs `verify_badge`)
                        badge = if (LibraryFixRules.verifyBadge(t.verifyStatus) != null) ({ VerifyBadge(t) }) else null,
                        onMore = { nav.trackMenu(t, onAlbumPage = true) },
                    )
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
    val shown = (loader.state.value as? Loadable.Ready)?.value?.detail?.album
    when (albumSheet) {
        "match" -> AlbumMatchSheet(nav, albumId, shown?.artist.orEmpty(), shown?.title.orEmpty(), onDismiss = { albumSheet = "" })
        "edit" -> AlbumEditSheet(nav, albumId, onDismiss = { albumSheet = "" })
        "artwork" -> AlbumArtworkSheet(nav, albumId, shown?.title.orEmpty(), onDismiss = { albumSheet = "" })
    }
    // ⋯ → Delete album from disk (admins)
    confirmDelete?.let { (title, files) ->
        ConfirmDialog(
            title = "Delete album",
            text = "Permanently delete \"$title\" ($files files) from disk?",
            okLabel = "Delete",
            danger = true,
            onDismiss = { confirmDelete = null },
        ) {
            albumAction {
                withContext(Dispatchers.IO) { nav.features.deleteAlbum(albumId) }
                Toasts.show("Album deleted", "ok")
                nav.back()
            }
        }
    }
}
