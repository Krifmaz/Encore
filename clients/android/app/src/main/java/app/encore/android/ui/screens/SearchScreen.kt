package app.encore.android.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.Artist
import app.encore.android.data.api.Track
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.SectionTitle
import app.encore.android.ui.components.TrackRow
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

private data class SearchResults(val tracks: List<Track>, val albums: List<Album>, val artists: List<Artist>) {
    val empty: Boolean get() = tracks.isEmpty() && albums.isEmpty() && artists.isEmpty()
}

/** Library search: songs, albums and artists whose names match (MusicBrainz search comes in M3). */
@Composable
fun SearchScreen(nav: Nav) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val playerState by nav.player.state.collectAsState()
    var query by remember { mutableStateOf("") }
    var results by remember { mutableStateOf<SearchResults?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(query) {
        val q = query.trim()
        if (q.length < 2) {
            results = null
            error = null
            return@LaunchedEffect
        }
        delay(300)
        busy = true
        try {
            results = withContext(Dispatchers.IO) {
                SearchResults(api.tracks(q = q, limit = 30), api.albums(q = q, limit = 20), api.artists(q).take(10))
            }
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Search failed"
        } finally {
            busy = false
        }
    }

    Column(Modifier.fillMaxSize()) {
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            placeholder = { Text("Songs, albums, artists") },
            singleLine = true,
            leadingIcon = { Icon(painterResource(R.drawable.ic_search), contentDescription = null) },
            trailingIcon = {
                if (query.isNotEmpty()) {
                    IconButton(onClick = { query = "" }) { Icon(painterResource(R.drawable.ic_close), contentDescription = "Clear") }
                }
            },
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            modifier = Modifier.fillMaxWidth().padding(16.dp),
        )
        val r = results
        when {
            busy && r == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            error != null -> Text(error ?: "", color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(16.dp))
            r == null -> Text(
                "Search your library",
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(16.dp),
            )
            r.empty -> Text("Nothing in your library matches", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(16.dp))
            else -> LazyColumn(Modifier.fillMaxSize()) {
                if (r.artists.isNotEmpty()) {
                    item { SectionTitle("Artists") }
                    items(r.artists, key = { "artist:" + it.name }) { a ->
                        Row(
                            Modifier.fillMaxWidth().clickable { nav.open(Screen.Artist(a.name)) }.padding(horizontal = 16.dp, vertical = 6.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Cover(if (a.coverAlbum.isNotEmpty()) api.coverUrl(a.coverAlbum, 96) else null, nav.graph.imageLoader, size = 44.dp, corner = 22.dp)
                            Spacer(Modifier.width(12.dp))
                            Text(a.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
                if (r.albums.isNotEmpty()) {
                    item { SectionTitle("Albums") }
                    item {
                        LazyRow(contentPadding = PaddingValues(horizontal = 8.dp)) {
                            items(r.albums, key = { it.id }) { a ->
                                AlbumCard(a, api.coverUrl(a.id, 300), nav.graph.imageLoader, Modifier.width(140.dp)) { nav.open(Screen.Album(a.id)) }
                            }
                        }
                    }
                }
                if (r.tracks.isNotEmpty()) {
                    item { SectionTitle("Songs") }
                    itemsIndexed(r.tracks, key = { _, t -> "track:" + t.id }) { i, t ->
                        TrackRow(
                            track = t,
                            imageLoader = nav.graph.imageLoader,
                            coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                            liked = t.id in liked,
                            playing = playerState.current?.id == t.id,
                            onClick = { nav.player.play(r.tracks, i) },
                            onLike = { nav.graph.likes.toggle(t.id) },
                        )
                    }
                }
                item { Spacer(Modifier.size(24.dp)) }
            }
        }
    }
}
