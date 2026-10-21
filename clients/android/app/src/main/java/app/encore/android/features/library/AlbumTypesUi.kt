package app.encore.android.features.library

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBarScrollBehavior
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.OneOffs
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import coil3.ImageLoader
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject

/**
 * Select mode of the Library's albums list (0.36, admins; docs/contracts/library-types.md §6.1): a long press on a tile
 * or the Select chip starts it, a tap then picks or unpicks, the Library's top bar becomes the selection's bar ("N
 * selected", Delete…, ⋯ with Select all N, Clear selection, Done), Back leaves it. Delete… asks the server what would
 * go (a dry run) and shows its words; Move to Trash moves the albums 100 a call, a toast counting them, then one toast
 * with Open Trash. Kept here (not in the list) so the top bar and the dialog can reach it.
 */
object AlbumSelect {
    var selecting by mutableStateOf(false)
    var picked by mutableStateOf<List<String>>(emptyList())
    /** How many albums the list has (Select all N). */
    var total by mutableIntStateOf(0)
    /** What the list shows: Select all asks for every id of it (set by the list as it draws; not drawn itself). */
    @Volatile var query: Query = Query()
    /** A Move to Trash running. */
    var moving by mutableStateOf(false)
    /** The Delete… dialog of a dry run. */
    var dialog by mutableStateOf<AlbumTypesRules.TrashDialog?>(null)
    /** Moves on after albums went: the list, the Type counts and the side index load again. */
    var generation by mutableIntStateOf(0)

    /** The albums list's sort, direction, search and Type. */
    data class Query(val sort: String = "added", val dir: String = "", val q: String = "", val type: String = "")

    fun enter(first: String? = null) {
        selecting = true
        picked = listOfNotNull(first)
    }

    fun leave() {
        selecting = false
        picked = emptyList()
        dialog = null
    }

    /** Changing the Type, sort or search clears what was picked (the mode stays). */
    fun clear() {
        picked = emptyList()
    }

    fun toggle(id: String) {
        picked = AlbumTypesRules.toggle(picked, id)
    }

    /** Select all N: every album of the list as it is filtered (at most 5000). */
    fun selectAll(nav: Nav) {
        val q = query
        nav.graph.scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { nav.graph.api.albumIds(q.sort, q.dir, q.q, "", q.type) }
                val (ids, more) = AlbumTypesRules.selectAll(r.ids)
                if (selecting) picked = ids
                if (more || r.capped) Toasts.show(AlbumTypesRules.AT_MOST, "warn")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }

    /** Delete…: a dry run over the picked albums, then the dialog with the server's words. */
    fun delete(nav: Nav) {
        val ids = picked
        if (ids.isEmpty() || moving) return
        nav.graph.scope.launch {
            try {
                val d = withContext(Dispatchers.IO) { nav.graph.api.trashAlbums(ids, dryRun = true) }
                dialog = AlbumTypesRules.trashDialog(d)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }

    /** The dialog's Move to Trash: the real runs, 100 albums a call, a toast that counts; one toast at the end. */
    fun move(nav: Nav) {
        val ids = picked
        dialog = null
        if (ids.isEmpty() || moving) return
        moving = true
        val n = ids.size
        nav.graph.scope.launch {
            var sums = AlbumTypesRules.Moved()
            var done = 0
            var counter = Toasts.show(AlbumTypesRules.movingText(0, n), "info", showMs = 600_000L)
            for (chunk in AlbumTypesRules.chunks(ids)) {
                sums = try {
                    sums.plus(withContext(Dispatchers.IO) { nav.graph.api.trashAlbums(chunk, dryRun = false) })
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    sums.failed(e.message ?: "That didn't work")
                }
                done += chunk.size
                Toasts.close(counter)
                counter = if (done < n) Toasts.show(AlbumTypesRules.movingText(done, n), "info", showMs = 600_000L) else 0L
            }
            Toasts.show(sums.text(n), "ok", action = AlbumTypesRules.OPEN_TRASH, showMs = AlbumTypesRules.TOAST_MS) { nav.open(Screen.Files("trash")) }
            moving = false
            leave()
            generation += 1
        }
    }
}

/** The Library's top bar in select mode: "N selected", Delete…, and ⋯ with Select all N, Clear selection, Done. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AlbumSelectTopBar(nav: Nav, scrollBehavior: TopAppBarScrollBehavior?) {
    val n = AlbumSelect.picked.size
    val moving = AlbumSelect.moving
    EncoreTopBar(AlbumTypesRules.selectedText(n), scrollBehavior = scrollBehavior, onBack = { if (!AlbumSelect.moving) AlbumSelect.leave() }) {
        SmallButton(AlbumTypesRules.DELETE, iconRes = R.drawable.ic_delete, danger = true, enabled = n > 0 && !moving) { AlbumSelect.delete(nav) }
        MoreButton(
            entries = listOf(
                MenuEntry(AlbumTypesRules.selectAllText(AlbumSelect.total), R.drawable.ic_check) { if (!AlbumSelect.moving) AlbumSelect.selectAll(nav) },
                MenuEntry(AlbumTypesRules.CLEAR_SELECTION, R.drawable.ic_close) { if (!AlbumSelect.moving) AlbumSelect.clear() },
                MenuEntry(AlbumTypesRules.DONE, R.drawable.ic_check) { if (!AlbumSelect.moving) AlbumSelect.leave() },
            ),
        )
    }
}

/** The Delete… dialog of select mode, while one is open. */
@Composable
fun AlbumSelectDialog(nav: Nav) {
    val d = AlbumSelect.dialog ?: return
    TrashConfirm(d, onDismiss = { AlbumSelect.dialog = null }) { AlbumSelect.move(nav) }
}

/**
 * The Delete… / Move album to Trash dialog from a dry run: the server's title ("Delete 12 albums (3.4 GB)?"), the
 * Trash's words, Move to Trash / Cancel; when the Trash's disk has no room, the server's words and Close only.
 */
@Composable
fun TrashConfirm(d: AlbumTypesRules.TrashDialog, onDismiss: () -> Unit, onMove: () -> Unit) {
    val p = MaterialTheme.palette
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(d.title) },
        text = { Text(d.text) },
        confirmButton = {
            if (d.canGo) {
                TextButton(onClick = { onDismiss(); onMove() }) { Text(AlbumTypesRules.MOVE_TO_TRASH, color = EncoreColors.Err, fontWeight = FontWeight.SemiBold) }
            } else {
                TextButton(onClick = onDismiss) { Text("Close", color = p.text) }
            }
        },
        dismissButton = if (d.canGo) ({ TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } }) else null,
    )
}

/** The Type chip and its dropdown above the albums list ("All types", "Live (12)" …), and Select for admins. */
@Composable
fun AlbumTypeRow(types: List<AlbumTypesRules.TypeCount>, selected: String, admin: Boolean, onPick: (String) -> Unit) {
    if (types.isEmpty() && !admin) return
    Row(
        Modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, bottom = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (types.isNotEmpty()) TypeChip(types, selected, onPick)
        if (admin) SmallButton(AlbumTypesRules.SELECT, iconRes = R.drawable.ic_check) { AlbumSelect.enter() }
    }
}

@Composable
private fun TypeChip(types: List<AlbumTypesRules.TypeCount>, selected: String, onPick: (String) -> Unit) {
    val p = MaterialTheme.palette
    var open by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(12.dp)
    Box {
        Row(
            Modifier
                .clip(shape)
                .background(if (selected.isEmpty()) p.panel else p.accent.copy(alpha = 0.14f))
                .border(1.dp, if (selected.isEmpty()) p.border else p.accent.copy(alpha = 0.4f), shape)
                .clickable(onClickLabel = AlbumTypesRules.TYPE) { open = true }
                .padding(start = 12.dp, end = 6.dp, top = 7.dp, bottom = 7.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(AlbumTypesRules.typeChipText(types, selected), style = MaterialTheme.typography.labelLarge, color = p.text, maxLines = 1)
            Icon(painterResource(R.drawable.ic_expand_more), contentDescription = AlbumTypesRules.TYPE, tint = p.muted, modifier = Modifier.size(18.dp))
        }
        PopMenu(
            open,
            { open = false },
            AlbumTypesRules.typeOptions(types, selected).map { (key, text) ->
                MenuEntry(text, iconRes = if (key == selected) R.drawable.ic_check else null) { if (key != selected) onPick(key) }
            },
        )
    }
}

/** An album tile of the list: in select mode a tap picks it (a check mark on the picked); a long press starts it. */
@Composable
fun PickableAlbumCard(a: Album, coverUrl: String, imageLoader: ImageLoader, admin: Boolean, onOpen: () -> Unit) {
    val selecting = AlbumSelect.selecting
    val picked = selecting && a.id in AlbumSelect.picked
    val p = MaterialTheme.palette
    Box {
        AlbumCard(
            a, coverUrl, imageLoader,
            onLongClick = if (admin && !a.isSurround) ({ if (!AlbumSelect.selecting) AlbumSelect.enter(a.id) else AlbumSelect.toggle(a.id) }) else null,
        ) { if (AlbumSelect.selecting) AlbumSelect.toggle(a.id) else onOpen() }
        if (selecting) {
            val mark = Modifier.align(Alignment.TopEnd).padding(8.dp).size(26.dp).clip(CircleShape)
            if (picked) {
                Box(mark.background(p.accent), contentAlignment = Alignment.Center) {
                    Icon(painterResource(R.drawable.ic_check), contentDescription = null, tint = p.onAccent, modifier = Modifier.size(18.dp))
                }
            } else {
                Box(mark.background(p.bg.copy(alpha = 0.55f)).border(2.dp, p.text.copy(alpha = 0.8f), CircleShape))
            }
        }
    }
}

/**
 * Change type… (§6.2): Album, EP, Single, One-off (with what a One-off is), or Automatic with what the server guesses
 * and why; the current one is checked. Save gives the format ("" = Automatic).
 */
@Composable
fun ChangeTypeDialog(album: String, type: JsonObject, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    val p = MaterialTheme.palette
    var picked by remember(type) { mutableStateOf(AlbumTypesRules.chosen(type)) }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(AlbumTypesRules.CHANGE_TYPE_TITLE) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                Text(AlbumTypesRules.whatIs(album), color = p.muted, style = MaterialTheme.typography.bodyMedium)
                Spacer(Modifier.size(8.dp))
                for ((value, label) in AlbumTypesRules.typeChoices(type)) {
                    Row(Modifier.fillMaxWidth().clickable { picked = value }, verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = picked == value, onClick = { picked = value })
                        Text(label, color = p.text, style = MaterialTheme.typography.bodyMedium)
                    }
                    if (value == "one_off") {
                        Text(
                            AlbumTypesRules.ONE_OFF_HELP,
                            color = p.muted,
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.padding(start = 48.dp, bottom = 4.dp),
                        )
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = { onDismiss(); onSave(picked) }) { Text("Save", color = p.accent, fontWeight = FontWeight.SemiBold) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/**
 * The artist pages' One-offs shelf (§6.3): the heading with the count and "Show all N", the line, the songs (a tap
 * plays the shelf from that song, ⋯ is the song menu, the sub-line "album · release label"). Nothing when there are none.
 */
fun LazyListScope.oneOffsShelf(
    nav: Nav,
    shelf: OneOffs,
    loadingAll: Boolean,
    likedIds: Set<Long>,
    nowPlayingId: Long?,
    onShowAll: () -> Unit,
) {
    if (shelf.tracks.isEmpty() || shelf.total == 0) return
    val all = AlbumTypesRules.showAllText(shelf.total, shelf.tracks.size)
    item(key = "oneoffs-head") {
        SectionHead(
            AlbumTypesRules.ONE_OFFS,
            subtitle = "${shelf.total}",
            action = if (all.isEmpty()) null else if (loadingAll) "Loading…" else all,
            onAction = if (all.isEmpty() || loadingAll) null else onShowAll,
        )
    }
    item(key = "oneoffs-line") {
        Text(
            AlbumTypesRules.ONE_OFFS_LINE,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.palette.muted,
            modifier = Modifier.padding(start = Gutter, end = Gutter, bottom = 6.dp),
        )
    }
    items(shelf.tracks.size, key = { i -> "oneoff-$i-${shelf.tracks[i].id}" }) { i ->
        val t = shelf.tracks[i]
        TrackRow(
            track = t,
            imageLoader = nav.graph.imageLoader,
            coverUrl = if (t.albumId.isNotEmpty()) nav.graph.api.coverUrl(t.albumId, 96) else null,
            liked = t.id in likedIds,
            playing = nowPlayingId == t.id,
            onClick = { nav.player.play(shelf.tracks, i) },
            onLike = { nav.graph.likes.toggle(t.id) },
            subtitle = AlbumTypesRules.oneOffSub(t.shownAlbum(), shelf.labels.getOrElse(i) { "" }),
            onMore = { nav.trackMenu(t) },
        )
    }
}
