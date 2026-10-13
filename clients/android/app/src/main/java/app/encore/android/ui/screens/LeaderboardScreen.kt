package app.encore.android.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.R
import app.encore.android.data.repo.Board
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.formatLongDuration
import app.encore.android.ui.components.formatNumber
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.palette

private val MEDAL = listOf("🥇", "🥈", "🥉")

private fun rank(i: Int): String = MEDAL.getOrNull(i) ?: "${i + 1}."

/** Leaderboard (web/js/pages/leaderboard.js): who requests, plays and likes the most, plus fun library facts. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun LeaderboardScreen(nav: Nav) {
    val p = MaterialTheme.palette
    val loader = rememberLoad("leaderboard") { nav.features.leaderboard() }
    val scope = rememberCoroutineScope()
    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("Leaderboard", scrollBehavior = scrollBehavior, onBack = nav.back) },
    ) { padding ->
        LoadableContent(loader) { d ->
            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                item { NoteText("Plays count after 30 seconds in the web player. Bragging rights only.", Modifier.padding(bottom = 6.dp)) }
                items(d.boards.size, key = { i -> "board-" + d.boards[i].key }) { i -> BoardCard(d.boards[i]) }
                if (d.thisWeek.isNotEmpty()) {
                    item { SectionHead("This week's top listener") }
                    item {
                        Box(Modifier.padding(horizontal = Gutter)) {
                            CardBox {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Text("👑 " + d.thisWeek[0].first, fontSize = 20.sp, fontWeight = FontWeight.Bold)
                                    Text(" · ${formatNumber(d.thisWeek[0].second)} songs played this week", style = MaterialTheme.typography.bodyMedium, color = p.muted)
                                }
                            }
                        }
                    }
                }
                item { SectionHead("Most played songs") }
                item {
                    Box(Modifier.padding(horizontal = Gutter)) {
                        CardBox {
                            if (d.topSongs.isEmpty()) {
                                EmptyState(R.drawable.ic_play, "No plays yet")
                            } else {
                                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                    d.topSongs.forEachIndexed { i, s ->
                                        Row(verticalAlignment = Alignment.CenterVertically) {
                                            Text("${i + 1}", color = p.faint, modifier = Modifier.width(22.dp))
                                            Cover(if (s.albumId.isNotEmpty()) nav.graph.api.coverUrl(s.albumId, 96) else null, nav.graph.imageLoader, size = 36.dp, corner = 6.dp, label = s.title)
                                            Spacer(Modifier.width(10.dp))
                                            Column(Modifier.weight(1f)) {
                                                Text(s.title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                                Text(s.artist, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                            }
                                            Text(formatNumber(s.plays), fontWeight = FontWeight.Bold)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                item { SectionHead("Most played artists") }
                item {
                    Box(Modifier.padding(horizontal = Gutter)) {
                        CardBox {
                            if (d.topArtists.isEmpty()) {
                                Text("No plays yet.", style = MaterialTheme.typography.bodySmall, color = p.muted)
                            } else {
                                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                    d.topArtists.forEachIndexed { i, a ->
                                        Row(verticalAlignment = Alignment.CenterVertically) {
                                            Text(rank(i) + " ", color = if (i < 3) p.text else p.faint)
                                            Text(a.artist, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.clickable { openCatalogArtist(nav, "", a.artist, scope) })
                                            Text(" · ${a.listeners} listener" + if (a.listeners == 1) "" else "s", style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 1, modifier = Modifier.weight(1f))
                                            Text(formatNumber(a.plays), fontWeight = FontWeight.Bold)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                item { SectionHead("Biggest in the library") }
                item {
                    Box(Modifier.padding(horizontal = Gutter)) {
                        CardBox {
                            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                d.libraryArtists.forEachIndexed { i, a ->
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text(rank(i) + " ", color = if (i < 3) p.text else p.faint)
                                        Text(a.artist, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f).clickable { openCatalogArtist(nav, "", a.artist, scope) })
                                        Text("${a.albums} albums · ", style = MaterialTheme.typography.bodySmall, color = p.muted)
                                        Text(formatNumber(a.songs), fontWeight = FontWeight.Bold)
                                        Text(" songs", style = MaterialTheme.typography.bodySmall, color = p.muted)
                                    }
                                }
                                if (d.libraryArtists.isEmpty()) Text("Nothing in the library yet.", style = MaterialTheme.typography.bodySmall, color = p.muted)
                            }
                        }
                    }
                }
                if (d.facts.isNotEmpty()) {
                    item { SectionHead("Fun facts") }
                    tileRows(d.facts, 2, key = "facts") { f ->
                        CardBox {
                            Column {
                                Text(f.value, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                Text(f.label, style = MaterialTheme.typography.bodySmall, color = p.muted)
                            }
                        }
                    }
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
}

/** One board (`board(k, b)`): medals, "(you)" in the accent, the value as a count or as listening time. */
@Composable
private fun BoardCard(b: Board) {
    val p = MaterialTheme.palette
    Box(Modifier.padding(horizontal = Gutter, vertical = 6.dp)) {
        CardBox {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(b.label, style = MaterialTheme.typography.titleMedium)
                if (b.rows.isEmpty()) {
                    Text("Nobody yet – be the first!", style = MaterialTheme.typography.bodySmall, color = p.muted)
                }
                b.rows.forEachIndexed { i, r ->
                    val color = if (r.me) p.accent else p.text
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(rank(i), color = if (i < 3) p.text else p.faint, modifier = Modifier.width(30.dp))
                        Text(r.username + if (r.me) " (you)" else "", color = color, fontWeight = if (r.me) FontWeight.Bold else FontWeight.Normal, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        Text(
                            if (b.key == "listened") formatLongDuration(r.value) else formatNumber(r.value.toLong()),
                            color = color, fontWeight = FontWeight.Bold,
                        )
                    }
                }
            }
        }
    }
}
