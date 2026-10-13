package app.encore.android.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.Track
import app.encore.android.data.session.Session
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.SectionTitle
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.rememberLoad
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private data class HomeData(val recent: List<Album>, val played: List<Track>)

@Composable
fun HomeScreen(nav: Nav, session: Session) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val playerState by nav.player.state.collectAsState()
    val scope = rememberCoroutineScope()
    val loader = rememberLoad(session.server, session.username) {
        HomeData(api.recentAlbums(24), try { api.recentPlays(20) } catch (_: Exception) { emptyList() })
    }

    LoadableContent(loader) { data ->
        LazyColumn(Modifier.fillMaxWidth()) {
            item {
                Row(
                    Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp, top = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        "Hi, ${session.username}",
                        style = MaterialTheme.typography.headlineSmall,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.weight(1f),
                    )
                    IconButton(onClick = { nav.open(Screen.Settings) }) {
                        Icon(painterResource(R.drawable.ic_settings), contentDescription = "Settings")
                    }
                }
            }
            item {
                Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = {
                        scope.launch {
                            val songs = try { withContext(Dispatchers.IO) { api.shuffle(50, emptyList()) } } catch (_: Exception) { emptyList() }
                            nav.player.play(songs)
                        }
                    }) {
                        Icon(painterResource(R.drawable.ic_shuffle), contentDescription = null, modifier = Modifier.size(18.dp))
                        Spacer(Modifier.width(8.dp))
                        Text("Shuffle library")
                    }
                    if (data.played.isNotEmpty()) {
                        OutlinedButton(onClick = { nav.player.play(data.played) }) { Text("Play recent") }
                    }
                }
            }
            if (data.recent.isNotEmpty()) {
                item { SectionTitle("Recently added") }
                item {
                    LazyRow(Modifier.fillMaxWidth(), contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 8.dp)) {
                        items(data.recent, key = { it.id }) { a ->
                            AlbumCard(a, api.coverUrl(a.id, 300), nav.graph.imageLoader, Modifier.width(150.dp)) {
                                nav.open(Screen.Album(a.id))
                            }
                        }
                    }
                }
            }
            if (data.played.isNotEmpty()) {
                item { SectionTitle("Recently played") }
                itemsIndexed(data.played) { i, t ->
                    TrackRow(
                        track = t,
                        imageLoader = nav.graph.imageLoader,
                        coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                        liked = t.id in liked,
                        playing = playerState.current?.id == t.id,
                        onClick = { nav.player.play(data.played, i) },
                        onLike = { nav.graph.likes.toggle(t.id) },
                    )
                }
            }
            item { Spacer(Modifier.size(24.dp)) }
        }
    }
}
