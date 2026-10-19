package app.encore.android.features.library

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
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
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.screens.features
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette

/**
 * Admins: the songs whose audio doesn't match their tags (the web's Library → Wrong audio, library.rs `render_issues`;
 * GET /api/library/mismatches). "Fix" (or the row) opens the song's album, where its ⋯ and the album's ⋯ correct it;
 * the row's own ⋯ has "This is the right song", "Edit song details…" and "Delete song from disk". [q] filters the
 * list by title, artist or album (the Library's search box).
 */
@Composable
fun WrongAudioList(nav: Nav, q: String) {
    val p = MaterialTheme.palette
    val loader = rememberLoad("mismatches") { nav.features.mismatches() }
    // a song confirmed, corrected or deleted from its ⋯: it leaves the list
    OnSongFixed { loader.refresh() }
    LoadableContent(loader) { all ->
        val rows = if (q.isEmpty()) all else all.filter { t -> listOf(t.title, t.artist, t.album).any { it.contains(q, ignoreCase = true) } }
        if (rows.isEmpty()) {
            EmptyState(R.drawable.ic_check, "Nothing flagged", "Run “Verify audio” to fingerprint your library.")
            return@LoadableContent
        }
        LazyColumn(Modifier.fillMaxSize()) {
            item {
                Text(
                    "${rows.size} file(s) whose audio doesn't match their tags",
                    style = MaterialTheme.typography.bodySmall, color = p.muted,
                    modifier = Modifier.padding(horizontal = Gutter, vertical = 4.dp),
                )
            }
            items(rows, key = { it.id }) { t -> WrongAudioRow(nav, t) }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }
}

@Composable
private fun WrongAudioRow(nav: Nav, t: Track) {
    val p = MaterialTheme.palette
    fun openAlbum() { if (t.albumId.isNotEmpty()) nav.open(Screen.Album(t.albumId)) }
    Row(
        Modifier.fillMaxWidth().clickable(onClickLabel = "Fix") { openAlbum() }.padding(start = Gutter, end = 4.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(
                listOf(t.artist, t.title).filter { it.isNotEmpty() }.joinToString(" – "),
                style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
            if (t.album.isNotEmpty()) Text(t.album, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (t.verifyDetail.isNotEmpty()) Text(t.verifyDetail, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err, maxLines = 3, overflow = TextOverflow.Ellipsis)
        }
        Spacer(Modifier.width(8.dp))
        SmallButton("Fix", iconRes = R.drawable.ic_edit, enabled = t.albumId.isNotEmpty()) { openAlbum() }
        IconButton(onClick = { nav.trackMenu(t) }) {
            Icon(painterResource(R.drawable.ic_more_vert), contentDescription = "More", tint = p.muted)
        }
    }
}
