package app.encore.android.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.int
import app.encore.android.core.str
import app.encore.android.data.api.Album
import app.encore.android.data.api.Track
import app.encore.android.data.api.User
import app.encore.android.data.session.Session
import app.encore.android.features.files.FilesAccess
import app.encore.android.features.group.ListeningTogetherRow
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.Tab
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.Avatar
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Collage
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MenuSeparator
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.Tile
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import java.util.Calendar

/** A ListenBrainz "Made for you" playlist (GET /api/discover/made-for-you `playlists[]`). */
private class MadeForYou(val title: String, val kind: String, val covers: List<String>, val owned: Int, val total: Int, val tracks: List<Track>)

private data class HomeData(val recent: List<Album>, val played: List<Track>, val madeForYou: List<MadeForYou>, val me: User?)

/** web/js/pages/foryou.js `KIND` */
private val MFY_KIND = mapOf(
    "weekly-exploration" to "New music picked for you every Monday",
    "daily-jams" to "Songs you love, refreshed daily",
    "weekly-jams" to "Your favourites from the past weeks",
    "lastfm" to "Similar to your most played songs on Last.fm",
)

private fun parseMadeForYou(rows: List<JsonObject>, base: String): List<MadeForYou> = rows.map { p ->
    val covers = (p["covers"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }?.filter { it.isNotEmpty() }
        ?.map { if (it.startsWith("/")) base + it else it } ?: emptyList()
    val tracks = (p["tracks"] as? JsonArray)?.mapNotNull { (it as? JsonObject)?.get("library") as? JsonObject }?.mapNotNull { Track.from(it) } ?: emptyList()
    MadeForYou(p.str("title").ifEmpty { "Made for you" }, p.str("kind"), covers, p.int("owned"), p.int("total"), tracks)
}

/** web/js/pages/home.js: the greeting by the hour. */
private fun greeting(): String {
    val hour = Calendar.getInstance().get(Calendar.HOUR_OF_DAY)
    return if (hour < 12) "Good morning" else if (hour < 18) "Good afternoon" else "Good evening"
}

@Composable
fun HomeScreen(nav: Nav, session: Session) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()
    val scope = rememberCoroutineScope()
    val p = MaterialTheme.palette
    val loader = rememberLoad(session.server, session.username) {
        HomeData(
            recent = api.recentAlbums(12),
            played = try { api.recentPlays(20) } catch (_: Exception) { emptyList() },
            madeForYou = try { parseMadeForYou(api.madeForYou(), api.base) } catch (_: Exception) { emptyList() },
            me = try { api.me() } catch (_: Exception) { null },
        )
    }

    LoadableContent(loader) { data ->
        // admins: the file manager in the menu, once the server says it has one (0.33)
        val filesHere by FilesAccess.available.collectAsState()
        androidx.compose.runtime.LaunchedEffect(data.me?.isAdmin) { if (data.me?.isAdmin == true) FilesAccess.probe(nav.graph) }
        LazyColumn(Modifier.fillMaxSize()) {
            // .hero: avatar, "Good evening, <name>" and the line under it
            item {
                var menu by remember { mutableStateOf(false) }
                Row(
                    Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 14.dp, bottom = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text("${greeting()}, ${session.username}", style = MaterialTheme.typography.headlineMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            "Find any artist, album or song – or paste a link from anywhere.",
                            style = MaterialTheme.typography.bodyMedium,
                            color = p.muted,
                            modifier = Modifier.padding(top = 6.dp),
                        )
                    }
                    Spacer(Modifier.width(12.dp))
                    Box {
                        val me = data.me
                        Avatar(
                            name = session.username,
                            url = if (me != null && me.id > 0) api.url("/api/users/${me.id}/avatar") else null,
                            imageLoader = nav.graph.imageLoader,
                            size = 40.dp,
                            modifier = Modifier.clip(CircleShape).clickable { menu = true },
                        )
                        PopMenu(
                            expanded = menu,
                            onDismiss = { menu = false },
                            entries = listOf(
                                MenuEntry("For you", R.drawable.ic_spark) { nav.open(Screen.ForYou) },
                                MenuEntry("Requests", R.drawable.ic_inbox) { nav.open(Screen.Requests) },
                                MenuEntry("Leaderboard", R.drawable.ic_leaderboard) { nav.open(Screen.Leaderboard) },
                                MenuSeparator,
                                MenuEntry("Account", R.drawable.ic_person) { nav.open(Screen.Account) },
                                MenuEntry("Settings", R.drawable.ic_settings) { nav.open(Screen.Settings) },
                                if (data.me?.isAdmin == true && filesHere == true) MenuEntry("Files", R.drawable.ic_folder) { nav.open(Screen.Files) } else null,
                                MenuEntry("Bug reports", R.drawable.ic_bug) { nav.open(Screen.BugReports("Home")) },
                            ),
                        )
                    }
                }
            }
            item {
                Row(Modifier.padding(horizontal = Gutter, vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    EncoreButton("Shuffle library", iconRes = R.drawable.ic_shuffle, primary = true) {
                        scope.launch {
                            val songs = try { withContext(Dispatchers.IO) { api.shuffle(50, emptyList()) } } catch (_: Exception) { emptyList() }
                            if (songs.isEmpty()) Toasts.show("Your library is empty", "err") else nav.player.play(songs)
                        }
                    }
                    if (data.played.isNotEmpty()) {
                        EncoreButton("Play recent", iconRes = R.drawable.ic_play) { nav.player.play(data.played) }
                    }
                }
            }

            // group listens on this server (0.31): nothing when there are none, or on an older server
            item { ListeningTogetherRow(nav) }

            if (data.madeForYou.isNotEmpty()) {
                item { SectionHead("Made for you", subtitle = "ListenBrainz builds these from what you listen to") }
                item {
                    LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        items(data.madeForYou) { pl ->
                            Tile(
                                title = pl.title,
                                subtitle = MFY_KIND[pl.kind] ?: "From ListenBrainz",
                                modifier = Modifier.width(160.dp),
                                onClick = {
                                    if (pl.tracks.isEmpty()) Toasts.show("Nothing from this playlist is in your library yet", "info")
                                    else nav.player.play(pl.tracks)
                                },
                            ) { m ->
                                Box(m) {
                                    Collage(pl.covers, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 12.dp, label = pl.title)
                                    Badge("${pl.owned}/${pl.total} in library", modifier = Modifier.padding(8.dp))
                                }
                            }
                        }
                    }
                }
            }

            item { SectionHead("Recently added", action = "Library") { nav.showTab(Tab.LIBRARY) } }
            if (data.recent.isEmpty()) {
                item { EmptyState(R.drawable.ic_library, "Your library is empty", "Search above or paste a link to get started.") }
            } else {
                item {
                    LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        items(data.recent, key = { it.id }) { a ->
                            AlbumCard(a, api.coverUrl(a.id, 400), nav.graph.imageLoader, Modifier.width(150.dp)) {
                                nav.open(Screen.Album(a.id))
                            }
                        }
                    }
                }
            }

            if (data.played.isNotEmpty()) {
                item { SectionHead("Recently played") }
                itemsIndexed(data.played, key = { i, t -> "${t.id}-$i" }) { i, t ->
                    TrackRow(
                        track = t,
                        imageLoader = nav.graph.imageLoader,
                        coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                        liked = t.id in liked,
                        playing = nowPlaying?.id == t.id,
                        onClick = { nav.player.play(data.played, i) },
                        onLike = { nav.graph.likes.toggle(t.id) },
                        subtitleLinks = rememberSongLinks(nav, t, withAlbum = false),
                        onMore = { nav.trackMenu(t) },
                    )
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }
}
