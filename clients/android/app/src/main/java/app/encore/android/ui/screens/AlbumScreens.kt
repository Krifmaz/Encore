package app.encore.android.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
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
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.obj
import app.encore.android.core.str
import app.encore.android.data.api.Album
import app.encore.android.data.api.AlbumDetail
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.SurroundState
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
import app.encore.android.ui.components.SmallButton
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
import app.encore.android.features.library.AlbumSelect
import app.encore.android.features.library.RestOfAlbumRules
import app.encore.android.data.repo.ExtRelease
import app.encore.android.features.library.AlbumTypesRules
import app.encore.android.features.library.ChangeTypeDialog
import app.encore.android.features.library.TrashConfirm
import app.encore.android.features.library.AlbumEditSheet
import app.encore.android.features.library.AlbumMatchSheet
import app.encore.android.features.library.LibraryFixRules
import app.encore.android.features.library.OnSongFixed
import app.encore.android.features.library.SongFixUi
import app.encore.android.features.library.SurroundRules
import app.encore.android.features.library.SurroundUi
import app.encore.android.features.library.SurroundVersions
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
 * What the album page shows beyond [AlbumDetail]: size, the history line, per-track "requested by" notes, the
 * MusicBrainz release group ("" when the album has none) for "Get the rest of the album" and "Release info", and a
 * stereo album's surround versions (0.35; null on a Surround album and from older servers).
 */
private class AlbumPage(
    val detail: AlbumDetail,
    val size: Long,
    val history: String,
    val notes: Map<Long, String>,
    val rgid: String = "",
    val surround: SurroundState? = null,
    /** its type (0.36): `{key, label, format, kind, chosen, auto, why, why_text}`; null on a Surround album / older server */
    val type: JsonObject? = null,
)

/**
 * GET /api/library/albums/{id} (server/src/routes/library.rs `album`): the album's `history` of who added its songs
 * and `added_at` (0.36) make the history line (an older server: the rows' `requested_by`, `requested_at` and
 * `added_at`); a row's note names who asked for it, kept on the song after Clear finished.
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
        // a Surround album: its format and its stereo album
        atmos = o.bool("atmos"),
        quality = o.str("quality"),
        stereoAlbumId = o.obj("stereo")?.str("album_id").orEmpty(),
    )
    val detail = AlbumDetail(album, tracks, o.dbl("duration"))

    val history = AlbumTypesRules.albumHistory(o, ::formatDay)

    val notes = HashMap<Long, String>()
    for (r in rows) {
        val note = AlbumTypesRules.requesterNote(r, ::formatDay)
        if (note.isNotEmpty()) notes[r.dbl("id").toLong()] = note
    }
    return AlbumPage(detail, o.dbl("size").toLong(), history, notes, o.str("mb_releasegroupid"), SurroundState.from(o["surround"]), o.obj("type"))
}

/**
 * One album of the library. A Surround album (0.35, library.rs: "the stereo library's tools stay off") plays and
 * queues, names its format and links to its stereo album; it has no likes, playlists, sharing or correcting tools.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun AlbumScreen(nav: Nav, albumId: String) {
    val api = nav.graph.api
    val surround = SurroundRules.isSurroundAlbum(albumId)
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
    // ⋯ → Move album to Trash: the dialog of its dry run; ⋯ → Change type…
    var trashDialog by remember { mutableStateOf<AlbumTypesRules.TrashDialog?>(null) }
    var changeType by remember { mutableStateOf(false) }
    // the admin's sheets: "match" (Tag / match metadata), "edit" (Edit details), "artwork" (Change artwork…)
    var albumSheet by remember { mutableStateOf("") }
    // "Get the rest of the album" for an album MusicBrainz doesn't list: another catalogue's track list (with its
    // artist), or the songs typed in (RestOfAlbum.kt)
    var restFound by remember { mutableStateOf<Pair<String, ExtRelease>?>(null) }
    var restTyped by remember { mutableStateOf<RestAsk?>(null) }
    val restStatuses = rememberRequestStatuses()
    // "Get the rest of the album": the album on Discover, where the songs that aren't here yet can be requested –
    // else another catalogue's track list, else a list to type in (never a dead end on MusicBrainz)
    fun restOfAlbum(rgid: String, artist: String, title: String) {
        if (rgid.isNotEmpty()) { nav.open(Screen.CatalogAlbum(rgid)); return }
        scope.launch {
            try {
                val found = withContext(Dispatchers.IO) { nav.features.albumFind(artist, title) }
                when (RestOfAlbumRules.found(found)) {
                    RestOfAlbumRules.Found.MUSICBRAINZ -> nav.open(Screen.CatalogAlbum(found.str("rgid")))
                    RestOfAlbumRules.Found.ELSEWHERE -> {
                        Toasts.show(RestOfAlbumRules.sourceNote(found.str("source")), "info")
                        restFound = found.str("artist").ifEmpty { artist } to restRelease(found)
                    }
                    RestOfAlbumRules.Found.NONE -> restTyped = RestAsk(artist, title, found.str("message"))
                }
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
                if (!surround) {
                    IconButton(onClick = {
                        val a = (loader.state.value as? Loadable.Ready)?.value?.detail?.album
                        shareAlbum(a?.title ?: "", a?.artist ?: "")
                    }) {
                        Icon(painterResource(R.drawable.ic_share), contentDescription = "Share link")
                    }
                }
            }
        },
    ) { padding ->
        LoadableContent(loader) { page ->
            val detail = page.detail
            val album = detail.album
            val tracks = detail.tracks
            val multiDisc = tracks.map { it.disc }.distinct().size > 1
            val formats = if (surround && album.quality.isNotEmpty()) album.quality
                else tracks.map { it.format.uppercase() }.filter { it.isNotEmpty() }.distinct().joinToString(" / ")
            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                // .detail-head
                item {
                    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter), horizontalAlignment = Alignment.CenterHorizontally) {
                        Box(Modifier.size(220.dp).shadow(24.dp, RoundedCornerShape(16.dp), clip = false)) {
                            Cover(api.coverUrl(albumId, 1000), nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 16.dp, label = album.title)
                        }
                        Spacer(Modifier.height(18.dp))
                        Text(if (surround) "SURROUND ALBUM · IN YOUR LIBRARY" else "ALBUM · IN YOUR LIBRARY", style = MaterialTheme.typography.labelSmall, color = p.muted, letterSpacing = 1.2.sp)
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
                        // the format, and the links between the stereo and surround versions (0.35)
                        if (formats.isNotEmpty() || surround || page.surround != null || page.type != null) {
                            FlowRow(
                                Modifier.padding(top = 12.dp),
                                horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
                                verticalArrangement = Arrangement.spacedBy(6.dp),
                            ) {
                                // its type (0.36): "Album", "Live · EP", "Compilation · One-off"; admins tap it for why
                                page.type?.let { t ->
                                    val why = t.str("why_text")
                                    Badge(t.str("label"), kind = "accent", onClick = if (me?.isAdmin == true && why.isNotEmpty()) ({ Toasts.show(why, "info") }) else null)
                                }
                                if (formats.isNotEmpty()) Badge(formats, kind = if (surround) "accent" else "")
                                SurroundVersions(nav, album, page.surround)
                            }
                        }
                        if (SurroundRules.canFetch(page.surround, me?.isAdmin == true)) {
                            SmallButton("Get Dolby Atmos version", iconRes = R.drawable.ic_download, modifier = Modifier.padding(top = 10.dp)) {
                                SurroundUi.getAtmos(scope, { api.requestAlbumSurround(albumId) }) { loader.refresh() }
                            }
                        }
                    }
                }
                item {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                        PlayButtons(nav, tracks) {
                            // a Surround album: playing it and its stereo version (library.rs `surround_album_menu`)
                            if (surround) MoreButton(
                                entries = listOf(
                                    MenuEntry("Add to queue", R.drawable.ic_queue_music) { tracks.forEach { nav.player.addToQueue(it) }; Toasts.show("Added to queue", "ok") },
                                    MenuEntry("Play next", R.drawable.ic_skip_next) { tracks.asReversed().forEach { nav.player.playNext(it) }; Toasts.show("Plays next", "ok") },
                                    if (album.stereoAlbumId.isNotEmpty()) MenuEntry("Go to the stereo version", R.drawable.ic_disc) { nav.open(Screen.Album(album.stereoAlbumId)) } else null,
                                ),
                            ) else MoreButton(
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
                                    if (me?.isAdmin == true && page.type != null) MenuEntry(AlbumTypesRules.CHANGE_TYPE, R.drawable.ic_note) { changeType = true } else null,
                                    if (me?.isAdmin == true) MenuEntry("Change artwork…", R.drawable.ic_disc) { albumSheet = "artwork" } else null,
                                    if (me?.isAdmin == true) MenuEntry("Find artwork automatically", R.drawable.ic_refresh) {
                                        albumAction {
                                            val n = withContext(Dispatchers.IO) { nav.features.findAlbumArtwork(albumId) }
                                            Toasts.show("Artwork embedded in $n file(s)", "ok")
                                            SongFixUi.changed()
                                        }
                                    } else null,
                                    if (me?.isAdmin == true) MenuSeparator else null,
                                    // to the Trash of Files (0.36), restorable there: the dialog of a dry run of this album
                                    if (me?.isAdmin == true) MenuEntry(AlbumTypesRules.MOVE_ALBUM, R.drawable.ic_delete, danger = true) {
                                        albumAction {
                                            val d = withContext(Dispatchers.IO) { api.trashAlbums(listOf(albumId), dryRun = true) }
                                            trashDialog = AlbumTypesRules.trashDialog(d)
                                        }
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
                    val fmt = if (t.isSurround && t.quality.isNotEmpty()) t.quality else (t.format.uppercase() + if (t.bitrate > 0) " ${t.bitrate}k" else "").trim()
                    val verified = LibraryFixRules.verifyBadge(t.verifyStatus) != null
                    TrackRow(
                        track = t,
                        imageLoader = null,
                        coverUrl = null,
                        liked = t.id in liked,
                        playing = nowPlaying?.id == t.id,
                        onClick = { nav.player.play(tracks, i) },
                        // Surround songs have no likes
                        onLike = if (t.isSurround) null else ({ nav.graph.likes.toggle(t.id) }),
                        number = t.trackNo,
                        subtitle = if (t.artist.isNotEmpty() && t.artist != album.artist) "${t.artist} · $fmt" else fmt,
                        subtitleLinks = if (t.artist.isNotEmpty() && t.artist != album.artist) rememberSongLinks(nav, t, withAlbum = false, suffix = " · $fmt") else null,
                        note = page.notes[t.id],
                        // the audio check's verdict: "wrong audio" marks the songs to correct (library.rs `verify_badge`);
                        // "Surround" on a song with a surround version (⋯ → Play the surround version)
                        badge = if (verified || t.surroundId > 0L) ({
                            if (verified) VerifyBadge(t)
                            if (t.surroundId > 0L) Badge("Surround", kind = "accent")
                        }) else null,
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
    // ⋯ → Get the rest of the album, not on MusicBrainz: the catalogue's track list, or the songs to type in
    restFound?.let { (artist, m) -> ExtTracksSheet(nav, artist, m, restStatuses, onDismiss = { restFound = null }) }
    restTyped?.let { ask -> RestTypedDialog(nav, ask, me?.isAdmin == true, onDismiss = { restTyped = null }) }
    // ⋯ → Move album to Trash (admins): the dry run's words, then the album goes; Open Trash in the toast
    trashDialog?.let { d ->
        TrashConfirm(d, onDismiss = { trashDialog = null }) {
            albumAction {
                val r = withContext(Dispatchers.IO) { api.moveAlbumToTrash(albumId) }
                Toasts.show(r.str("text").ifEmpty { AlbumTypesRules.movedText(1, 1, "") }, "ok", action = AlbumTypesRules.OPEN_TRASH, showMs = AlbumTypesRules.TOAST_MS) {
                    nav.open(Screen.Files("trash"))
                }
                // the Library's albums list and Type counts load again
                AlbumSelect.generation += 1
                nav.back()
            }
        }
    }
    // ⋯ → Change type… (admins): saved, the page again
    val typeNow = (loader.state.value as? Loadable.Ready)?.value?.type
    if (changeType && typeNow != null) {
        ChangeTypeDialog(shown?.title.orEmpty(), typeNow, onDismiss = { changeType = false }) { format ->
            albumAction {
                withContext(Dispatchers.IO) { api.setAlbumType(albumId, format) }
                Toasts.show(AlbumTypesRules.TYPE_SAVED, "ok")
                AlbumSelect.generation += 1
                loader.refresh()
            }
        }
    }
}
