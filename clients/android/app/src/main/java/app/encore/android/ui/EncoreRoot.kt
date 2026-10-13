package app.encore.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
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
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import app.encore.android.AppGraph
import app.encore.android.R
import app.encore.android.data.session.AuthState
import app.encore.android.data.session.Session
import app.encore.android.ui.screens.AlbumScreen
import app.encore.android.ui.screens.ArtistScreen
import app.encore.android.ui.screens.AuthFlow
import app.encore.android.ui.screens.FullPlayer
import app.encore.android.ui.screens.HomeScreen
import app.encore.android.ui.screens.LibraryScreen
import app.encore.android.ui.screens.MiniPlayer
import app.encore.android.ui.screens.PlaylistScreen
import app.encore.android.ui.screens.PlaylistsScreen
import app.encore.android.ui.screens.SearchScreen
import app.encore.android.ui.screens.SettingsScreen

/** Places the app can go; tabs are the roots, the rest stack on top of the current tab. */
sealed interface Screen {
    data class Album(val id: String) : Screen
    data class Artist(val name: String) : Screen
    data class Playlist(val id: Long, val name: String) : Screen
    data object Settings : Screen
}

enum class Tab(val label: String, val icon: Int) {
    HOME("Home", R.drawable.ic_home),
    SEARCH("Search", R.drawable.ic_search),
    LIBRARY("Library", R.drawable.ic_library),
    PLAYLISTS("Playlists", R.drawable.ic_queue_music),
}

/** Everything a screen needs to move around and play. */
class Nav(
    val graph: AppGraph,
    val player: PlayerConnection,
    val open: (Screen) -> Unit,
    val back: () -> Unit,
)

@Composable
fun EncoreRoot(graph: AppGraph, player: PlayerConnection) {
    val auth by graph.store.auth.collectAsState()
    when (val a = auth) {
        is AuthState.Loading -> Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background))
        is AuthState.SignedOut -> AuthFlow(graph)
        is AuthState.SignedIn -> MainScreen(graph, player, a.session)
    }
}

@Composable
private fun MainScreen(graph: AppGraph, player: PlayerConnection, session: Session) {
    var tab by remember { mutableStateOf(Tab.HOME) }
    val stack = remember { mutableStateListOf<Screen>() }
    var showPlayer by remember { mutableStateOf(false) }
    val playerState by player.state.collectAsState()

    LaunchedEffect(session) { graph.likes.refresh(force = true) }

    val nav = remember(graph, player) {
        Nav(
            graph = graph,
            player = player,
            open = { s -> showPlayer = false; stack.add(s) },
            back = { if (stack.isNotEmpty()) stack.removeAt(stack.lastIndex) },
        )
    }

    BackHandler(enabled = showPlayer || stack.isNotEmpty() || tab != Tab.HOME) {
        when {
            showPlayer -> showPlayer = false
            stack.isNotEmpty() -> stack.removeAt(stack.lastIndex)
            else -> tab = Tab.HOME
        }
    }

    Box(Modifier.fillMaxSize()) {
        Scaffold(
            containerColor = MaterialTheme.colorScheme.background,
            bottomBar = {
                Column {
                    if (playerState.current != null) {
                        MiniPlayer(nav, playerState, onOpen = { showPlayer = true })
                    }
                    NavigationBar(containerColor = MaterialTheme.colorScheme.surface) {
                        for (t in Tab.entries) {
                            NavigationBarItem(
                                selected = t == tab && stack.isEmpty(),
                                onClick = {
                                    stack.clear()
                                    tab = t
                                },
                                icon = { Icon(painterResource(t.icon), contentDescription = null) },
                                label = { Text(t.label) },
                            )
                        }
                    }
                }
            },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding)) {
                val top = stack.lastOrNull()
                when (top) {
                    is Screen.Album -> AlbumScreen(nav, top.id)
                    is Screen.Artist -> ArtistScreen(nav, top.name)
                    is Screen.Playlist -> PlaylistScreen(nav, top.id, top.name)
                    is Screen.Settings -> SettingsScreen(nav, session)
                    null -> when (tab) {
                        Tab.HOME -> HomeScreen(nav, session)
                        Tab.SEARCH -> SearchScreen(nav)
                        Tab.LIBRARY -> LibraryScreen(nav)
                        Tab.PLAYLISTS -> PlaylistsScreen(nav)
                    }
                }
            }
        }
        if (showPlayer && playerState.current != null) {
            FullPlayer(nav, playerState, onClose = { showPlayer = false })
        }
    }
}
