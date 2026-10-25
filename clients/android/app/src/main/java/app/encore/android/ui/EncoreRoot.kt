package app.encore.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import app.encore.android.AppGraph
import app.encore.android.R
import app.encore.android.data.api.Track
import app.encore.android.data.session.AuthState
import app.encore.android.data.session.Session
import app.encore.android.features.bugs.BugReportScreen
import app.encore.android.features.bugs.BugReportsScreen
import app.encore.android.features.offline.OfflineBanner
import app.encore.android.features.offline.OfflineMenu
import app.encore.android.features.offline.OfflineMode
import app.encore.android.features.offline.rememberOfflineMenu
import app.encore.android.features.files.FilesScreen
import app.encore.android.features.group.GroupBar
import app.encore.android.features.group.GroupDialogs
import app.encore.android.features.group.GroupListen
import app.encore.android.remote.DevicesSheet
import app.encore.android.remote.LinkedBar
import app.encore.android.remote.RemoteUi
import app.encore.android.ui.components.AddToPlaylistSheet
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.formatDay
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.ReportHost
import app.encore.android.ui.components.ReportUi
import app.encore.android.ui.components.ToastHost
import app.encore.android.ui.components.Toasts
import app.encore.android.data.repo.PickerOpts
import app.encore.android.ui.screens.AccountScreen
import app.encore.android.ui.theme.focusRing
import app.encore.android.ui.screens.AlbumScreen
import app.encore.android.ui.screens.ArtistScreen
import app.encore.android.ui.screens.AuthFlow
import app.encore.android.features.library.AlbumTypesRules
import app.encore.android.features.library.PlaylistLinkHost
import app.encore.android.features.library.SongFixHost
import app.encore.android.features.library.SongFixUi
import app.encore.android.features.library.SurroundUi
import app.encore.android.features.requests.ActivityScreen
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.features
import androidx.compose.runtime.rememberCoroutineScope
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import app.encore.android.ui.screens.CatalogAlbumScreen
import app.encore.android.ui.screens.CatalogArtistScreen
import app.encore.android.ui.screens.DiscoverScreen
import app.encore.android.ui.screens.ForYouScreen
import app.encore.android.ui.screens.FullPlayer
import app.encore.android.ui.screens.HomeScreen
import app.encore.android.ui.screens.LeaderboardScreen
import app.encore.android.ui.screens.LibraryScreen
import app.encore.android.ui.screens.LyricsRequests
import app.encore.android.ui.screens.PickerScreen
import app.encore.android.ui.screens.PlaylistEditScreen
import app.encore.android.ui.screens.PlaylistImportScreen
import app.encore.android.ui.screens.RequestsScreen
import app.encore.android.ui.screens.TagScreen
import app.encore.android.ui.screens.SignalPathSheet
import app.encore.android.ui.screens.MiniPlayer
import app.encore.android.ui.screens.PlaylistScreen
import app.encore.android.ui.screens.PlaylistsScreen
import app.encore.android.ui.screens.SettingsScreen
import app.encore.android.ui.screens.UpNextSheet
import app.encore.android.ui.screens.rememberPlayerMenuActions
import app.encore.android.ui.theme.palette
import app.encore.android.ui.tv.RailItem
import app.encore.android.ui.tv.SidePanel
import app.encore.android.ui.tv.TvNav
import app.encore.android.ui.tv.TvPlayer
import app.encore.android.ui.tv.TvPrefs
import app.encore.android.ui.tv.TvScaled
import app.encore.android.ui.tv.TvShell
import app.encore.android.ui.tv.rememberTvLayout

/**
 * Places the app can go; tabs are the roots, the rest stack on top of the current tab.
 *
 * For you / Requests / Leaderboard / Account open from the Home avatar menu through the entries below; Discover's
 * artist and album catalog pages and the request picker stack like any other screen.
 */
sealed interface Screen {
    data class Album(val id: String) : Screen
    data class Artist(val name: String) : Screen
    data class Playlist(val id: Long, val name: String) : Screen
    data object Settings : Screen
    data object ForYou : Screen
    data object Requests : Screen
    data object Leaderboard : Screen
    data object Account : Screen

    // features part: Discover's MusicBrainz pages, genre pages, the download picker and playlist editing
    /** An artist on MusicBrainz (web `#/artist/<mbid>`). */
    data class CatalogArtist(val mbid: String) : Screen
    /** A release group on MusicBrainz (web `#/album/<rgid>`). */
    data class CatalogAlbum(val rgid: String) : Screen
    /** Discover opened with a search already typed (web `#/search?q=&type=`). */
    data class DiscoverSearch(val query: String, val type: String = "all") : Screen
    /** A genre page (web `#/tag/<name>`). */
    data class Tag(val name: String) : Screen
    /** The manual download picker (web `pickerModal`). */
    data class Picker(val opts: PickerOpts) : Screen
    /** Reorder / remove songs of a playlist. */
    data class PlaylistEdit(val id: Long) : Screen
    /**
     * Import a playlist (`into == null`) or add songs from a link to one; [url] fills the link in. From the Requests
     * link box ("Import as playlist", the web Downloader's): [requestMissing] ticked, [albumMode] chosen, and the
     * link previewed at once.
     */
    data class PlaylistImport(val into: Long?, val url: String = "", val requestMissing: Boolean = false, val albumMode: String? = null) : Screen

    /** Admins: Soulseek transfers, torrent / Usenet downloads and the server log (the web's Activity, under the Downloader). */
    data object Activity : Screen

    /** Settings opened at one page or card (a `#/settings?s=…` link, e.g. the picker's "Settings → Soulseek"). */
    data class SettingsAt(val id: String) : Screen

    // bug reports (features/bugs): the list (opened from the Home menu or Settings → This device; [from] is sent along
    // with a new report as the page it came from) and one report with its thread
    data class BugReports(val from: String = "") : Screen
    data class BugReport(val id: Long) : Screen

    /**
     * The admin file manager (features/files, 0.33): Settings → Server → Files, or the Home menu. [startRoot] (0.36):
     * a root to open at once ("trash": the toast after albums went to the Trash, "Open Trash").
     */
    data class Files(val startRoot: String = "") : Screen
}

/**
 * The bottom tabs (the web's main nav on phones): Home, Discover, Library, Playlists. Discover is the one search:
 * what the library has first, then the catalogue (it took over the old Search tab).
 */
enum class Tab(val label: String, val icon: Int) {
    HOME("Home", R.drawable.ic_home),
    DISCOVER("Discover", R.drawable.ic_search),
    LIBRARY("Library", R.drawable.ic_library),
    PLAYLISTS("Playlists", R.drawable.ic_queue_music),
}

/** Everything a screen needs to move around and play. */
class Nav(
    val graph: AppGraph,
    val player: PlayerConnection,
    val open: (Screen) -> Unit,
    val back: () -> Unit,
    /** Switch to a bottom tab (clears the stack). */
    val showTab: (Tab) -> Unit,
    /** The "Add to playlist" sheet for these songs. */
    val addToPlaylist: (List<Long>) -> Unit,
    /** The "⋯" menu for one song in a list (web/js/pages/library.js `trackMenu`); drawn once at the root. */
    val showTrackMenu: (track: Track, onAlbumPage: Boolean) -> Unit,
) {
    /** `onAlbumPage`: true leaves out "Go to album". */
    fun trackMenu(track: Track, onAlbumPage: Boolean = false) = showTrackMenu(track, onAlbumPage)
}

/**
 * The app's root: the sign-in flow, or the signed-in app. On a TV (Android TV / Google TV) both are drawn zoomed out
 * ([TvScaled]) and the signed-in app uses the TV layout (side menu, TV player) instead of the phone one.
 */
@Composable
fun EncoreRoot(graph: AppGraph, player: PlayerConnection) {
    val auth by graph.store.auth.collectAsState()
    // on a TV, unless Settings → TV says "Phone layout"
    val tv = rememberTvLayout()
    TvScaled(tv) {
        when (val a = auth) {
            is AuthState.Loading -> Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background))
            is AuthState.SignedOut -> {
                AuthFlow(graph)
                ToastHost(bottomPadding = 24.dp)
            }
            is AuthState.SignedIn -> MainScreen(graph, player, a.session, tv)
        }
    }
}

/** A song's "⋯" menu, requested by any list; drawn once at the root. */
private class TrackMenuRequest(val track: Track, val onAlbumPage: Boolean)

/**
 * The signed-in app: the pages with their navigation, the players, and every sheet and menu drawn once at the root.
 *
 * @param tv the TV layout ([TvShell]: side menu, [TvPlayer]) instead of the phone one (bottom tabs, mini player,
 *   [FullPlayer]).
 */
@Composable
private fun MainScreen(graph: AppGraph, player: PlayerConnection, session: Session, tv: Boolean) {
    val p = MaterialTheme.palette
    // saveable: the tab survives the activity being made again (a configuration change the manifest doesn't keep
    // the activity through, or the system closing the app in the background)
    var tab by rememberSaveable { mutableStateOf(Tab.HOME) }
    val stack = remember { mutableStateListOf<Screen>() }
    // saveable for the same reason: the open full player (and its Up next sheet) stays open when the phone is turned
    // either way, even if the activity is made again, instead of dropping back to the page underneath
    var showPlayer by rememberSaveable { mutableStateOf(false) }
    var showUpNext by rememberSaveable { mutableStateOf(false) }
    var addToPlaylistIds by remember { mutableStateOf<List<Long>?>(null) }
    var trackMenuFor by remember { mutableStateOf<TrackMenuRequest?>(null) }
    val nowPlaying by player.nowPlaying.collectAsState()
    // sheets the player part opens from its PlayerMenuActions (lyrics / signal path / up next)
    val lyricsOpen by player.lyricsOpen.collectAsState()
    val signalPathOpen by player.signalPathOpen.collectAsState()
    val upNextOpen by player.upNextOpen.collectAsState()
    // this phone as a remote for another device (remote/RemoteDevice.kt) and the Devices sheet
    val link by player.remote.link.collectAsState()
    val devicesOpen by RemoteUi.devicesOpen.collectAsState()
    // a group listen (features/group): its bar above the mini player; a group link shared to the app asks to join
    val group by GroupListen.state.collectAsState()
    val groupLink by GroupListen.pendingLink.collectAsState()
    LaunchedEffect(groupLink, session) {
        val text = groupLink ?: return@LaunchedEffect
        GroupListen.pendingLink.value = null
        GroupListen.init(graph.app)
        GroupListen.openInvite(text)
    }

    LaunchedEffect(session) {
        // the server's local network address while on the home network, its public one otherwise
        app.encore.android.data.api.ServerRoute.start(graph.app)
        OfflineMode.start(graph.app)
    }
    // who is signed in (role and permissions), for the song menu's correcting entries and the Library's admin tab
    val me by SongFixUi.me.collectAsState()
    LaunchedEffect(session) {
        SongFixUi.setMe(null)
        SongFixUi.setMe(try { withContext(Dispatchers.IO) { graph.features.me() } } catch (e: CancellationException) { throw e } catch (_: Exception) { null })
    }
    // whether the server has the Surround library (0.35): the Library's Surround tab, the song menu's surround entries
    LaunchedEffect(session) { SurroundUi.probe(graph) }
    val fixScope = rememberCoroutineScope()
    LaunchedEffect(session) { graph.likes.refresh(force = true) }
    // the first page of each Library tab's covers, so the tabs open with their covers in place
    LaunchedEffect(session) { graph.coverPreloader.firstPages() }
    // the next song's cover, fetched ahead so it shows at once when the song changes
    val queueIndex by player.queueIndex.collectAsState()
    LaunchedEffect(queueIndex, nowPlaying?.id) { player.queue.value.getOrNull(queueIndex + 1)?.let { graph.coverPreloader.nextSong(it.albumId) } }

    val nav = remember(graph, player) {
        Nav(
            graph = graph,
            player = player,
            open = { s -> showPlayer = false; showUpNext = false; stack.add(s) },
            back = { if (stack.isNotEmpty()) stack.removeAt(stack.lastIndex) },
            showTab = { t -> showPlayer = false; stack.clear(); tab = t },
            addToPlaylist = { ids -> if (ids.isNotEmpty()) addToPlaylistIds = ids },
            showTrackMenu = { t, onAlbum -> trackMenuFor = TrackMenuRequest(t, onAlbum) },
        )
    }
    // the song menu's "Download for offline": made here, so the storage permission's answer (Android 8–9) still
    // finds it after the menu closed
    val offlineMenu = rememberOfflineMenu(nav)
    // the menu actions are made once: they read the layout as it is now, not as it was then
    val tvNow by rememberUpdatedState(tv)
    val menuActions = rememberPlayerMenuActions(
        nav = nav,
        onClose = { showPlayer = false },
        // the TV player has Up next as one of its halves: switch to it (opening the player) instead of a sheet
        onUpNext = { if (tvNow) { TvPrefs.setPanel(SidePanel.QUEUE); showPlayer = true } else { showUpNext = true } },
        onAddToPlaylist = { ids -> addToPlaylistIds = ids },
    )

    BackHandler(enabled = lyricsOpen || showPlayer || stack.isNotEmpty() || tab != Tab.HOME) {
        when {
            lyricsOpen -> player.closeLyrics()
            showPlayer -> showPlayer = false
            stack.isNotEmpty() -> stack.removeAt(stack.lastIndex)
            else -> tab = Tab.HOME
        }
    }

    // the page on screen: the top of the stack, or the tab's root
    val page: @Composable () -> Unit = {
        when (val top = stack.lastOrNull()) {
            is Screen.Album -> AlbumScreen(nav, top.id)
            is Screen.Artist -> ArtistScreen(nav, top.name)
            is Screen.Playlist -> PlaylistScreen(nav, top.id, top.name)
            is Screen.Settings -> SettingsScreen(nav, session)
            is Screen.ForYou -> ForYouScreen(nav)
            is Screen.Requests -> RequestsScreen(nav)
            is Screen.Leaderboard -> LeaderboardScreen(nav)
            is Screen.Account -> AccountScreen(nav, session)
            is Screen.CatalogArtist -> CatalogArtistScreen(nav, top.mbid)
            is Screen.CatalogAlbum -> CatalogAlbumScreen(nav, top.rgid)
            is Screen.DiscoverSearch -> DiscoverScreen(nav, initialQuery = top.query, initialType = top.type, stacked = true)
            is Screen.Tag -> TagScreen(nav, top.name)
            is Screen.Picker -> PickerScreen(nav, top.opts)
            is Screen.PlaylistEdit -> PlaylistEditScreen(nav, top.id)
            is Screen.PlaylistImport -> PlaylistImportScreen(nav, top.into, top.url, top.requestMissing, top.albumMode)
            is Screen.Activity -> ActivityScreen(nav)
            is Screen.SettingsAt -> SettingsScreen(nav, session, startAt = top.id)
            is Screen.BugReports -> BugReportsScreen(nav, top.from)
            is Screen.BugReport -> BugReportScreen(nav, top.id)
            is Screen.Files -> FilesScreen(nav, top.startRoot)
            null -> when (tab) {
                Tab.HOME -> HomeScreen(nav, session)
                Tab.LIBRARY -> LibraryScreen(nav)
                Tab.PLAYLISTS -> PlaylistsScreen(nav)
                Tab.DISCOVER -> DiscoverScreen(nav)
            }
        }
    }
    // "Continue <song> from 2:31 · was playing on <device>": pick up where another device left off
    // linked: "Controlling <device>" instead (the pick-up bar is hidden while this phone is a remote)
    // in a group listen: "Group · 3" (and "Paused here · Catch up") instead
    val stateBar: @Composable () -> Unit = {
        when {
            group != null -> GroupBar(nav)
            link == null -> SpotBar(nav, session)
            else -> LinkedBar(nav)
        }
    }

    Box(Modifier.fillMaxSize()) {
        if (tv) {
            TvShell(
                nav = nav,
                selected = TvNav.selected(showPlayer, stack.lastOrNull(), tab),
                // another page (or the player closing over it): the focus goes onto the page
                pageKey = listOf(stack.size, stack.lastOrNull(), tab, showPlayer),
                onSelect = { item ->
                    when (item) {
                        RailItem.NOW_PLAYING -> { showPlayer = true }
                        RailItem.SEARCH -> nav.showTab(Tab.DISCOVER)
                        RailItem.HOME -> nav.showTab(Tab.HOME)
                        RailItem.LIBRARY -> nav.showTab(Tab.LIBRARY)
                        RailItem.PLAYLISTS -> nav.showTab(Tab.PLAYLISTS)
                        // For you and Settings are pages of the stack on a phone; on the TV they are places of their own
                        RailItem.FOR_YOU -> { showPlayer = false; stack.clear(); stack.add(Screen.ForYou) }
                        RailItem.SETTINGS -> { showPlayer = false; stack.clear(); stack.add(Screen.Settings) }
                    }
                },
                topBar = {
                    Column {
                        OfflineBanner()
                        stateBar()
                    }
                },
                page = page,
            )
        } else Scaffold(
            containerColor = p.bg,
            topBar = { OfflineBanner() },
            bottomBar = {
                Column {
                    stateBar()
                    if (nowPlaying != null) MiniPlayer(nav, onOpen = { showPlayer = true })
                    Box(Modifier.fillMaxWidth().height(1.dp).background(p.border))
                    NavigationBar(containerColor = p.bg2, tonalElevation = 0.dp) {
                        for (t in Tab.entries) {
                            NavigationBarItem(
                                // the tabs pick their own ripple, so they need the TV focus ring added (FocusRing.kt)
                                modifier = Modifier.focusRing(p.accent),
                                selected = t == tab && stack.isEmpty(),
                                onClick = {
                                    stack.clear()
                                    tab = t
                                },
                                icon = { Icon(painterResource(t.icon), contentDescription = null) },
                                label = { Text(t.label, style = MaterialTheme.typography.labelMedium) },
                                colors = NavigationBarItemDefaults.colors(
                                    selectedIconColor = p.text,
                                    selectedTextColor = p.text,
                                    indicatorColor = p.panel2,
                                    unselectedIconColor = p.muted,
                                    unselectedTextColor = p.muted,
                                ),
                            )
                        }
                    }
                }
            },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding)) { page() }
        }
        if (showPlayer && nowPlaying != null) {
            if (tv) TvPlayer(nav, menuActions, onClose = { showPlayer = false })
            else FullPlayer(nav, menuActions, onClose = { showPlayer = false })
        }
        // ⋯ → Lyrics (a song's menu, the player's): the full player opens in lyrics mode; the TV player on its lyrics
        LyricsRequests(nav, onOpenPlayer = {
            if (tv) TvPrefs.setPanel(SidePanel.LYRICS)
            showPlayer = true
        })
        if (signalPathOpen && nowPlaying != null) {
            SignalPathSheet(nav, onDismiss = { player.closeSignalPath() })
        }
        if (showUpNext || upNextOpen) {
            UpNextSheet(nav, onDismiss = { showUpNext = false; player.closeUpNext() })
        }
        if (devicesOpen) {
            DevicesSheet(nav, onDismiss = { RemoteUi.devicesOpen.value = false })
        }
        // group listen: its sheet, Start, Join with a code, "Join <host>'s group listen?"
        GroupDialogs(nav)
        // an Encore playlist link shared to the app: "Add to my playlists?" (kept until signed in, as group links are)
        PlaylistLinkHost(nav, session)
        addToPlaylistIds?.let { ids ->
            AddToPlaylistSheet(graph.api, ids, onDismiss = { addToPlaylistIds = null })
        }
        trackMenuFor?.let { req ->
            TrackMenu(nav, req, offlineMenu, me, fixScope, onDismiss = { trackMenuFor = null })
        }
        // ⋯ → Report a problem (songs and albums)
        ReportHost(nav)
        // bring your own music: Replace with my own file, Upload your own files
        app.encore.android.features.library.OwnFileHost(nav)
        // ⋯ → Edit song details… / Delete song from disk (correcting a library song)
        SongFixHost(nav)
        // toasts sit above the mini player and the tabs, in a window of their own above every sheet and dialog
        // (the TV layout has no bars at the bottom)
        ToastHost(bottomPadding = if (tv) 24.dp else if (nowPlaying != null) 150.dp else 84.dp)
    }
}

/**
 * The "⋯" of one song in a list (web/js/pages/library.js `trackMenu`): Play next, Add to queue, Add to playlist,
 * Lyrics, Share link, Go to album, its surround version (0.35), Report a problem, and for those who may correct the
 * song: Edit song details…, This is the right song, Delete song from disk. Shown as a bottom-anchored popup at the root
 * so every list shares one. A Surround song (library.rs `surround_track_menu`) keeps playing, Lyrics and its albums.
 */
@Composable
private fun TrackMenu(nav: Nav, req: TrackMenuRequest, offlineMenu: OfflineMenu, me: Me?, fixScope: CoroutineScope, onDismiss: () -> Unit) {
    val t = req.track
    val player = nav.player
    val surround = t.isSurround
    // who added the song (0.36, GET /api/library/tracks/{id}/added): the menu's first line once the server said; none
    // until then, when it has nothing to say, offline, or from an older server
    var added by remember(t.id) { mutableStateOf("") }
    LaunchedEffect(t.id) {
        if (OfflineMode.active.value) return@LaunchedEffect
        added = try {
            withContext(Dispatchers.IO) { AlbumTypesRules.addedOf(nav.graph.api.trackAdded(t.id), ::formatDay) }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            ""
        }
    }
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        PopMenu(
            expanded = true,
            onDismiss = onDismiss,
            entries = listOf(
                if (added.isNotEmpty()) MenuEntry(added, info = true, onClick = {}) else null,
                MenuEntry("Play next", R.drawable.ic_skip_next) { player.playNext(t); Toasts.show("Plays next", "ok") },
                MenuEntry("Add to queue", R.drawable.ic_queue_music) { player.addToQueue(t); Toasts.show("Added to queue", "ok") },
                if (!surround) MenuEntry("Add to playlist", R.drawable.ic_playlist_add) { nav.addToPlaylist(listOf(t.id)) } else null,
                *(if (!surround) offlineMenu.entries(listOf(t)) else emptyList()).toTypedArray(),
                MenuEntry("Lyrics", R.drawable.ic_lyrics) {
                    val d = player.menuActions
                    if (d != null && player.nowPlaying.value?.id == t.id) d.lyrics() else Toasts.show("Play the song, then open Lyrics from the player", "info")
                },
                if (!surround) MenuEntry("Share link", R.drawable.ic_share) {
                    val d = player.menuActions
                    if (d != null && player.nowPlaying.value?.id == t.id) d.shareLink() else Toasts.show("Play the song, then share it from the player", "info")
                } else null,
                if (t.albumId.isNotEmpty() && !req.onAlbumPage) MenuEntry("Go to album", R.drawable.ic_album) { nav.open(Screen.Album(t.albumId)) } else null,
                if (surround) SurroundUi.stereoEntry(nav, t) else null,
                if (t.albumArtist.isNotEmpty() || t.artist.isNotEmpty()) MenuEntry("Go to artist", R.drawable.ic_person) { app.encore.android.ui.screens.openTrackArtist(nav, t) } else null,
                // its surround version: play it, or (admins) fetch the Dolby Atmos version
                *SurroundUi.entries(nav, t, me, fixScope).toTypedArray(),
                if (!surround) MenuEntry("Report a problem", R.drawable.ic_flag) { ReportUi.track(t.id, t.title) } else null,
                // bring your own music (admins and the "own_files" permission)
                app.encore.android.features.library.OwnFileUi.entry(me, t.id, t.title, surround),
                // admins, and members with "fix" on a song their request brought in (library.rs `track_menu`)
                *(if (!surround) SongFixUi.entries(nav, t, me, fixScope, req.onAlbumPage) else emptyList()).toTypedArray(),
            ),
        )
    }
}
