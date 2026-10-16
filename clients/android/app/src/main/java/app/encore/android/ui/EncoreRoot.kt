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
import app.encore.android.remote.DevicesSheet
import app.encore.android.remote.LinkedBar
import app.encore.android.remote.RemoteUi
import app.encore.android.ui.components.AddToPlaylistSheet
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.ReportHost
import app.encore.android.ui.components.ReportUi
import app.encore.android.ui.components.ToastHost
import app.encore.android.ui.components.Toasts
import app.encore.android.data.repo.PickerOpts
import app.encore.android.ui.screens.AccountScreen
import app.encore.android.ui.screens.AlbumScreen
import app.encore.android.ui.screens.ArtistScreen
import app.encore.android.ui.screens.AuthFlow
import app.encore.android.ui.screens.CatalogAlbumScreen
import app.encore.android.ui.screens.CatalogArtistScreen
import app.encore.android.ui.screens.DiscoverScreen
import app.encore.android.ui.screens.ForYouScreen
import app.encore.android.ui.screens.FullPlayer
import app.encore.android.ui.screens.HomeScreen
import app.encore.android.ui.screens.LeaderboardScreen
import app.encore.android.ui.screens.LibraryScreen
import app.encore.android.ui.screens.LyricsScreen
import app.encore.android.ui.screens.PickerScreen
import app.encore.android.ui.screens.PlaylistEditScreen
import app.encore.android.ui.screens.PlaylistImportScreen
import app.encore.android.ui.screens.RequestsScreen
import app.encore.android.ui.screens.TagScreen
import app.encore.android.ui.screens.SignalPathSheet
import app.encore.android.ui.screens.MiniPlayer
import app.encore.android.ui.screens.PlaylistScreen
import app.encore.android.ui.screens.PlaylistsScreen
import app.encore.android.ui.screens.SearchScreen
import app.encore.android.ui.screens.SettingsScreen
import app.encore.android.ui.screens.UpNextSheet
import app.encore.android.ui.screens.rememberPlayerMenuActions
import app.encore.android.ui.theme.palette

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
    /** Import a playlist (`into == null`) or add songs from a link to one. */
    data class PlaylistImport(val into: Long?) : Screen
}

/** The bottom tabs (the web's main nav on phones): Home, Search, Library, Playlists, Discover. */
enum class Tab(val label: String, val icon: Int) {
    HOME("Home", R.drawable.ic_home),
    SEARCH("Search", R.drawable.ic_search),
    LIBRARY("Library", R.drawable.ic_library),
    PLAYLISTS("Playlists", R.drawable.ic_queue_music),
    DISCOVER("Discover", R.drawable.ic_explore),
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

@Composable
fun EncoreRoot(graph: AppGraph, player: PlayerConnection) {
    val auth by graph.store.auth.collectAsState()
    when (val a = auth) {
        is AuthState.Loading -> Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background))
        is AuthState.SignedOut -> AuthFlow(graph)
        is AuthState.SignedIn -> MainScreen(graph, player, a.session)
    }
}

/** A song's "⋯" menu, requested by any list; drawn once at the root. */
private class TrackMenuRequest(val track: Track, val onAlbumPage: Boolean)

@Composable
private fun MainScreen(graph: AppGraph, player: PlayerConnection, session: Session) {
    val p = MaterialTheme.palette
    var tab by remember { mutableStateOf(Tab.HOME) }
    val stack = remember { mutableStateListOf<Screen>() }
    var showPlayer by remember { mutableStateOf(false) }
    var showUpNext by remember { mutableStateOf(false) }
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
    val menuActions = rememberPlayerMenuActions(
        nav = nav,
        onClose = { showPlayer = false },
        onUpNext = { showUpNext = true },
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

    Box(Modifier.fillMaxSize()) {
        Scaffold(
            containerColor = p.bg,
            bottomBar = {
                Column {
                    // "Continue <song> from 2:31 · was playing on <device>": pick up where another device left off
                    // linked: "Controlling <device>" instead (the pick-up bar is hidden while this phone is a remote)
                    if (link == null) SpotBar(nav, session) else LinkedBar(nav)
                    if (nowPlaying != null) MiniPlayer(nav, onOpen = { showPlayer = true })
                    Box(Modifier.fillMaxWidth().height(1.dp).background(p.border))
                    NavigationBar(containerColor = p.bg2, tonalElevation = 0.dp) {
                        for (t in Tab.entries) {
                            NavigationBarItem(
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
            Box(Modifier.fillMaxSize().padding(padding)) {
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
                    is Screen.PlaylistImport -> PlaylistImportScreen(nav, top.into)
                    null -> when (tab) {
                        Tab.HOME -> HomeScreen(nav, session)
                        Tab.SEARCH -> SearchScreen(nav)
                        Tab.LIBRARY -> LibraryScreen(nav)
                        Tab.PLAYLISTS -> PlaylistsScreen(nav)
                        Tab.DISCOVER -> DiscoverScreen(nav)
                    }
                }
            }
        }
        if (showPlayer && nowPlaying != null) {
            FullPlayer(nav, menuActions, onClose = { showPlayer = false })
        }
        if (lyricsOpen && nowPlaying != null) {
            LyricsScreen(nav, onClose = { player.closeLyrics() })
        }
        if (signalPathOpen && nowPlaying != null) {
            SignalPathSheet(nav, onDismiss = { player.closeSignalPath() })
        }
        if (showUpNext || upNextOpen) {
            UpNextSheet(nav, onDismiss = { showUpNext = false; player.closeUpNext() })
        }
        if (devicesOpen) {
            DevicesSheet(nav, onDismiss = { RemoteUi.devicesOpen.value = false })
        }
        addToPlaylistIds?.let { ids ->
            AddToPlaylistSheet(graph.api, ids, onDismiss = { addToPlaylistIds = null })
        }
        trackMenuFor?.let { req ->
            TrackMenu(nav, req, onDismiss = { trackMenuFor = null })
        }
        // ⋯ → Report a problem (songs and albums)
        ReportHost(nav)
        // toasts sit above the mini player and the tabs
        ToastHost(Modifier.align(Alignment.BottomCenter).padding(bottom = if (nowPlaying != null) 150.dp else 84.dp))
    }
}

/**
 * The "⋯" of one song in a list (web/js/pages/library.js `trackMenu`): Play next, Add to queue, Add to playlist,
 * Lyrics, Share link, Go to album. Shown as a bottom-anchored popup at the root so every list shares one.
 */
@Composable
private fun TrackMenu(nav: Nav, req: TrackMenuRequest, onDismiss: () -> Unit) {
    val t = req.track
    val player = nav.player
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        PopMenu(
            expanded = true,
            onDismiss = onDismiss,
            entries = listOf(
                MenuEntry("Play next", R.drawable.ic_skip_next) { player.playNext(t); Toasts.show("Plays next", "ok") },
                MenuEntry("Add to queue", R.drawable.ic_queue_music) { player.addToQueue(t); Toasts.show("Added to queue", "ok") },
                MenuEntry("Add to playlist", R.drawable.ic_playlist_add) { nav.addToPlaylist(listOf(t.id)) },
                MenuEntry("Lyrics", R.drawable.ic_lyrics) {
                    val d = player.menuActions
                    if (d != null && player.nowPlaying.value?.id == t.id) d.lyrics() else Toasts.show("Play the song, then open Lyrics from the player", "info")
                },
                MenuEntry("Share link", R.drawable.ic_share) {
                    val d = player.menuActions
                    if (d != null && player.nowPlaying.value?.id == t.id) d.shareLink() else Toasts.show("Play the song, then share it from the player", "info")
                },
                if (t.albumId.isNotEmpty() && !req.onAlbumPage) MenuEntry("Go to album", R.drawable.ic_album) { nav.open(Screen.Album(t.albumId)) } else null,
                if (t.albumArtist.isNotEmpty() || t.artist.isNotEmpty()) MenuEntry("Go to artist", R.drawable.ic_person) { app.encore.android.ui.screens.openTrackArtist(nav, t) } else null,
                MenuEntry("Report a problem", R.drawable.ic_flag) { ReportUi.track(t.id, t.title) },
            ),
        )
    }
}
