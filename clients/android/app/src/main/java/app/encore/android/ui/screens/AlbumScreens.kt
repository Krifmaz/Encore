package app.encore.android.ui.screens

import androidx.compose.foundation.layout.Arrangement
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
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Track
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.components.rememberLoad

/** A back arrow and a title, for pages stacked on a tab. */
@Composable
fun BackBar(title: String, onBack: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(top = 4.dp, end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = onBack) { Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back") }
        Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** Play and Shuffle for a list of songs. */
@Composable
fun PlayButtons(nav: Nav, tracks: List<Track>) {
    Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = { nav.player.play(tracks) }, enabled = tracks.isNotEmpty()) {
            Icon(painterResource(R.drawable.ic_play), contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(6.dp))
            Text("Play")
        }
        OutlinedButton(onClick = { nav.player.play(tracks, shuffle = true) }, enabled = tracks.isNotEmpty()) {
            Icon(painterResource(R.drawable.ic_shuffle), contentDescription = null, modifier = Modifier.size(18.dp))
            Spacer(Modifier.width(6.dp))
            Text("Shuffle")
        }
    }
}

@Composable
fun AlbumScreen(nav: Nav, albumId: String) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val playerState by nav.player.state.collectAsState()
    val loader = rememberLoad(albumId) { api.album(albumId) }
    Column(Modifier.fillMaxSize()) {
        BackBar("", nav.back)
        LoadableContent(loader) { detail ->
            val multiDisc = detail.tracks.map { it.disc }.distinct().size > 1
            LazyColumn(Modifier.fillMaxSize()) {
                item {
                    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                        Cover(api.coverUrl(albumId, 600), nav.graph.imageLoader, size = 240.dp, corner = 12.dp)
                        Spacer(Modifier.size(12.dp))
                        Text(detail.album.title, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
                        Text(
                            detail.album.artist,
                            style = MaterialTheme.typography.bodyLarge,
                            color = MaterialTheme.colorScheme.secondary,
                            modifier = Modifier.padding(top = 2.dp),
                        )
                        Text(
                            listOf(detail.album.year, "${detail.tracks.size} songs", formatDuration((detail.durationSec * 1000).toLong()),
                                detail.album.formats.uppercase()).filter { it.isNotEmpty() }.joinToString(" · "),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                item {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) { PlayButtons(nav, detail.tracks) }
                }
                itemsIndexed(detail.tracks, key = { _, t -> t.id }) { i, t ->
                    TrackRow(
                        track = t,
                        imageLoader = null,
                        coverUrl = null,
                        liked = t.id in liked,
                        playing = playerState.current?.id == t.id,
                        onClick = { nav.player.play(detail.tracks, i) },
                        onLike = { nav.graph.likes.toggle(t.id) },
                        number = if (multiDisc) t.disc * 100 + t.trackNo else t.trackNo,
                        subtitle = if (t.artist != detail.album.artist) t.artist else t.format.uppercase(),
                    )
                }
                item { Spacer(Modifier.size(24.dp)) }
            }
        }
    }
}

/** An artist in the library: their albums (the web's library artist view). */
@Composable
fun ArtistScreen(nav: Nav, name: String) {
    val api = nav.graph.api
    val loader = rememberLoad(name) { api.albums(sort = "year", artist = name, limit = 200) }
    Column(Modifier.fillMaxSize()) {
        BackBar(name, nav.back)
        LoadableContent(loader) { albums ->
            LazyVerticalGrid(
                columns = GridCells.Adaptive(150.dp),
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(8.dp),
            ) {
                item(span = { GridItemSpan(maxLineSpan) }) {
                    Text(
                        name,
                        style = MaterialTheme.typography.headlineSmall,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.padding(8.dp),
                    )
                }
                items(albums, key = { it.id }) { a ->
                    AlbumCard(a, api.coverUrl(a.id, 300), nav.graph.imageLoader) { nav.open(Screen.Album(a.id)) }
                }
            }
        }
    }
}
