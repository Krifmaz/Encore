package app.encore.android.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.EncoreColors

/** Playlists, Liked Songs first (read-only in M1; editing comes in M3). */
@Composable
fun PlaylistsScreen(nav: Nav) {
    val api = nav.graph.api
    val loader = rememberLoad("playlists") { api.playlists() }
    Column(Modifier.fillMaxSize()) {
        Text(
            "Playlists",
            style = MaterialTheme.typography.headlineSmall,
            modifier = Modifier.padding(16.dp),
        )
        LoadableContent(loader) { playlists ->
            val sorted = playlists.sortedByDescending { it.isLiked }
            LazyColumn(Modifier.fillMaxSize()) {
                items(sorted, key = { it.id }) { p ->
                    Row(
                        Modifier.fillMaxWidth().clickable { nav.open(Screen.Playlist(p.id, p.name)) }.padding(horizontal = 16.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        if (p.isLiked) {
                            Box(
                                Modifier.size(56.dp).clip(RoundedCornerShape(8.dp)).background(EncoreColors.AccentGradient),
                                contentAlignment = Alignment.Center,
                            ) {
                                Icon(painterResource(R.drawable.ic_favorite), contentDescription = null, tint = EncoreColors.Background)
                            }
                        } else {
                            Cover(p.coverUrl.ifEmpty { null }, nav.graph.imageLoader, size = 56.dp)
                        }
                        Spacer(Modifier.width(12.dp))
                        Column(Modifier.weight(1f)) {
                            Text(p.name, maxLines = 1, overflow = TextOverflow.Ellipsis, fontWeight = FontWeight.Medium)
                            Text(
                                if (p.matched in 1 until p.total) "${p.matched} of ${p.total} songs in your library" else "${p.total} songs",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
fun PlaylistScreen(nav: Nav, id: Long, name: String) {
    val api = nav.graph.api
    val liked by nav.graph.likes.liked.collectAsState()
    val playerState by nav.player.state.collectAsState()
    // Liked Songs changes as you like things: reload when the set changes
    val loader = rememberLoad(id, if (name == "Liked Songs") liked.size else 0) { api.playlist(id) }
    Column(Modifier.fillMaxSize()) {
        BackBar(name, nav.back)
        LoadableContent(loader) { detail ->
            val tracks = detail.tracks
            LazyColumn(Modifier.fillMaxSize()) {
                item {
                    Column(Modifier.padding(horizontal = 16.dp)) {
                        Text(detail.playlist.name, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.SemiBold)
                        Text(
                            "${tracks.size} playable of ${detail.rows.size}",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                item { PlayButtons(nav, tracks) }
                val rows = detail.rows
                items(rows.size) { i ->
                    val row = rows[i]
                    val t = row.track
                    if (t != null) {
                        val at = rows.subList(0, i).count { it.track != null }
                        TrackRow(
                            track = t,
                            imageLoader = nav.graph.imageLoader,
                            coverUrl = if (t.albumId.isNotEmpty()) api.coverUrl(t.albumId, 96) else null,
                            liked = t.id in liked,
                            playing = playerState.current?.id == t.id,
                            onClick = { nav.player.play(tracks, at) },
                            onLike = { nav.graph.likes.toggle(t.id) },
                        )
                    } else {
                        // not in the library (yet): shown greyed, like the web's "missing" rows
                        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
                            Text(row.title, color = MaterialTheme.colorScheme.outlineVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(
                                "${row.artist} · not in your library",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.outlineVariant,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                }
                item { Spacer(Modifier.size(24.dp)) }
            }
        }
    }
}
