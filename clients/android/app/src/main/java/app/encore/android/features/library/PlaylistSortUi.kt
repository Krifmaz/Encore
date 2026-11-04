package app.encore.android.features.library

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MenuSeparator
import app.encore.android.ui.components.PopMenu
import app.encore.android.ui.theme.palette

/**
 * Each playlist's sort, kept on this phone (like the web's `encore.plSort`): one preference per playlist id, holding
 * [PlaylistSortRules.encode]'s text. The default is removed instead of stored.
 */
class PlaylistSorts(context: Context) {
    /** The preferences file the sorts live in. */
    private val prefs = context.applicationContext.getSharedPreferences("encore_playlist_sort", Context.MODE_PRIVATE)

    /** Playlist [id]'s sort, or the default when it was never sorted. */
    fun get(id: Long): PlSort = PlaylistSortRules.decode(try { prefs.getString("sort.$id", null) } catch (_: Exception) { null })

    /** Keeps [s] for playlist [id] (the default removes the entry). */
    fun set(id: Long, s: PlSort) {
        try {
            prefs.edit().apply { if (s == PlSort()) remove("sort.$id") else putString("sort.$id", PlaylistSortRules.encode(s)) }.apply()
        } catch (_: Exception) { /* kept for this visit */ }
    }
}

/**
 * The Sort chip of a playlist (like the Library's): the current sort with its arrow; the menu picks another sort
 * (each starts in its own direction, see [PlaylistSortRules.descFirst]) or reverses this one.
 */
@Composable
fun PlaylistSortChip(sort: PlSort, onChange: (PlSort) -> Unit, modifier: Modifier = Modifier) {
    val p = MaterialTheme.palette
    val label = PlaylistSortRules.label(sort.key)
    var open by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(12.dp)
    Box(modifier) {
        Row(
            Modifier
                .clip(shape)
                .background(p.panel)
                .border(1.dp, p.border, shape)
                .clickable(onClickLabel = "Sort: $label") { open = true }
                .padding(start = 10.dp, end = 6.dp, top = 9.dp, bottom = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                painterResource(if (sort.desc) R.drawable.ic_arrow_downward else R.drawable.ic_arrow_upward),
                contentDescription = PlaylistSortRules.dirLabel(sort),
                tint = p.muted,
                modifier = Modifier.size(14.dp),
            )
            Spacer(Modifier.width(4.dp))
            Text(label, style = MaterialTheme.typography.labelLarge, color = p.text, maxLines = 1)
            Icon(painterResource(R.drawable.ic_expand_more), contentDescription = "Sort", tint = p.muted, modifier = Modifier.size(18.dp))
        }
        PopMenu(
            open,
            { open = false },
            PlaylistSortRules.SORTS.map { (key, l) ->
                MenuEntry(l, iconRes = if (key == sort.key) R.drawable.ic_check else null) {
                    if (key != sort.key) onChange(PlaylistSortRules.picked(key))
                }
            } + listOf(
                MenuSeparator,
                MenuEntry(PlaylistSortRules.REVERSE, iconRes = if (sort.desc) R.drawable.ic_arrow_upward else R.drawable.ic_arrow_downward) { onChange(sort.reversed()) },
            ),
        )
    }
}
