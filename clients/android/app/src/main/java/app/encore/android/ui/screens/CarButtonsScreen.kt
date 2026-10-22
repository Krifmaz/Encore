package app.encore.android.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.car.CarNowPlaying
import app.encore.android.playback.ButtonShelfRules
import app.encore.android.playback.CarButtonRules
import app.encore.android.playback.CarButtonRules.Id
import app.encore.android.playback.CarButtonRules.Layout
import app.encore.android.playback.CarPrefs
import app.encore.android.playback.LyricsStripRules
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.theme.palette

// Settings → Android Auto (also Settings → Playback → In the car → Car buttons): the car's buttons are picked here on
// the phone, as horizontal rows like Home's Recently added (ButtonShelfRules): the car player's buttons
// (CarButtonRules, CarPrefs.carButtons; a connected car follows at once through CarControls) and the Encore Lyrics
// strip (LyricsStripRules, CarPrefs.lyricsStrip; the open car app follows at once too).

/** The button's icon on this page (the car draws Media3's own icons or the same drawables). */
private fun iconOf(id: Id): Int = when (id) {
    Id.SHUFFLE -> R.drawable.ic_shuffle
    Id.REPEAT -> R.drawable.ic_repeat
    Id.LIKE -> R.drawable.ic_favorite
    Id.LYRICS -> R.drawable.ic_lyrics
    Id.ARTIST -> R.drawable.ic_person
    Id.SIGNAL -> R.drawable.ic_signal
}

/**
 * What TalkBack says for a text of this page, in place of the text: ⋯ and ⋮ in words ([CarButtonRules.spoken]).
 * [live]: a polite live region, said again when it appears or changes.
 */
private fun Modifier.said(text: String, live: Boolean = false): Modifier = clearAndSetSemantics {
    contentDescription = CarButtonRules.spoken(text)
    if (live) liveRegion = LiveRegionMode.Polite
}

/**
 * The "Car buttons" row of Settings → Playback → In the car: the car's buttons in order
 * ("Shuffle · Repeat · Like · Lyrics") and the way to [CarButtonsScreen].
 */
@Composable
fun CarButtonsRow(onOpen: () -> Unit) {
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val layout by remember { CarPrefs.carButtons(context) }.collectAsState()
    val summary = (layout.screen + layout.menu).joinToString(" · ") { it.label }.ifEmpty { "No buttons – only Previous, Play and Next" }
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onOpen).said("Car buttons, $summary").padding(vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text("Car buttons", style = MaterialTheme.typography.bodyLarge)
            Text(summary, style = MaterialTheme.typography.bodySmall, color = p.muted)
        }
        Icon(painterResource(R.drawable.ic_expand_more), contentDescription = null, tint = p.muted, modifier = Modifier.rotate(-90f))
    }
}

private const val CAR_HELP =
    "Beside Previous, Play and Next, in this order. Tap a button to hide it, tap a hidden one to add it, and use the arrows to move one. If your car has fewer spots, Android Auto puts the last ones behind ⋯."
private const val LYRICS_HELP =
    "Encore Lyrics (in the car's app list) draws the lyrics over the whole screen. Its strip holds up to ${LyricsStripRules.MAX} buttons."
private const val FOOTER = "Only on this phone. The car follows a change at once."

/**
 * Settings → Android Auto: the car's buttons, picked on the phone. Two sets, each a Shown row (in the car's order)
 * and a Hidden row, both scrolling sideways like Home's Recently added: the car player's buttons and the Encore
 * Lyrics strip.
 */
@Composable
fun CarButtonsScreen(onBack: () -> Unit, title: String = "Android Auto") {
    val context = LocalContext.current
    val layout by remember { CarPrefs.carButtons(context) }.collectAsState()
    val strip by remember { CarPrefs.lyricsStrip(context) }.collectAsState()
    val spots by remember { CarPrefs.carSpots(context) }.collectAsState()
    val now by CarNowPlaying.state.collectAsState()
    // the previews show the playing song when there is one, a sample otherwise
    val song = now?.title?.takeIf { it.isNotEmpty() } ?: "Song title"
    val artist = now?.artist?.takeIf { it.isNotEmpty() } ?: "Artist"
    BackHandler(onBack = onBack)

    // the car gets the buttons in this one order; Android Auto itself decides how many fit before ⋯
    val shown = layout.screen + layout.menu
    val setCar: (List<Id>) -> Unit = { list ->
        CarPrefs.setCarButtons(context, Layout(list, emptyList(), ButtonShelfRules.hidden(Id.entries, list)))
    }
    val setStrip: (List<LyricsStripRules.Btn>) -> Unit = { CarPrefs.setLyricsStrip(context, it) }
    val carDefault = CarButtonRules.DEFAULT.screen + CarButtonRules.DEFAULT.menu

    Column(Modifier.fillMaxSize()) {
        BackBar(title, onBack)
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            SectionHead("Car player buttons", subtitle = CAR_HELP)
            CarPlayerPreview(CarButtonRules.preview(shown, spots), song, artist)
            Shelf(
                shown = shown,
                all = Id.entries,
                max = null,
                label = { it.label },
                icon = ::iconOf,
                set = setCar,
            )
            ResetButton(enabled = shown != carDefault || layout.menu.isNotEmpty()) { setCar(carDefault) }

            SectionHead("Encore Lyrics buttons", subtitle = LYRICS_HELP)
            LyricsPreview(strip, song, artist)
            Shelf(
                shown = strip,
                all = LyricsStripRules.Btn.entries,
                max = LyricsStripRules.MAX,
                label = { it.label },
                icon = ::stripIconOf,
                set = setStrip,
            )
            ResetButton(enabled = strip != LyricsStripRules.DEFAULT) { setStrip(LyricsStripRules.DEFAULT) }

            HelpLine(FOOTER, Modifier.padding(horizontal = Gutter))
            Spacer(Modifier.height(32.dp))
        }
    }
}

@Composable
private fun ResetButton(enabled: Boolean, onClick: () -> Unit) {
    TextButton(onClick = onClick, enabled = enabled, modifier = Modifier.padding(start = Gutter - 12.dp, top = 4.dp)) { Text("Reset to default") }
}

@Composable
private fun HelpLine(text: String, modifier: Modifier = Modifier) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted, modifier = modifier.padding(top = 4.dp, bottom = 4.dp).said(text))
}

/**
 * One set of buttons: a Shown row and a Hidden row, each scrolling sideways ([ButtonShelfRules]). A tap on a shown
 * tile hides it, a tap on a hidden one adds it at the end (while there is room under [max]); ◀ / ▶ move a shown one.
 */
@Composable
private fun <T> Shelf(shown: List<T>, all: List<T>, max: Int?, label: (T) -> String, icon: (T) -> Int, set: (List<T>) -> Unit) {
    val p = MaterialTheme.palette
    val hidden = ButtonShelfRules.hidden(all, shown)
    val full = max != null && shown.size >= max

    RowTitle(ButtonShelfRules.shownTitle(shown.size, max), live = true)
    if (shown.isEmpty()) {
        Text("None – tap a hidden button to add it", style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(horizontal = Gutter, vertical = 12.dp))
    } else {
        LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            items(shown, key = { label(it) }) { x ->
                val i = shown.indexOf(x)
                Tile(
                    icon = icon(x),
                    label = label(x),
                    hint = "Tap to hide",
                    dim = false,
                    enabled = true,
                    onTap = { set(ButtonShelfRules.hide(shown, x)) },
                    onLeft = if (i > 0) ({ set(ButtonShelfRules.left(shown, x)) }) else null,
                    onRight = if (i < shown.lastIndex) ({ set(ButtonShelfRules.right(shown, x)) }) else null,
                )
            }
        }
    }

    RowTitle("Hidden · ${hidden.size}")
    if (hidden.isEmpty()) {
        Text("None", style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(horizontal = Gutter, vertical = 12.dp))
    } else {
        LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            items(hidden, key = { label(it) }) { x ->
                Tile(
                    icon = icon(x),
                    label = label(x),
                    hint = if (full) "Full" else "Tap to add",
                    dim = true,
                    enabled = !full,
                    onTap = { set(ButtonShelfRules.show(shown, x, max ?: Int.MAX_VALUE)) },
                    onLeft = null,
                    onRight = null,
                )
            }
        }
    }
}

@Composable
private fun RowTitle(text: String, live: Boolean = false) {
    Text(
        text,
        style = MaterialTheme.typography.titleSmall,
        modifier = Modifier.padding(start = Gutter, end = Gutter, top = 8.dp, bottom = 6.dp).said(text, live),
    )
}

/** A button as a small card (icon, name, what a tap does) with ◀ / ▶ under it on the Shown row. */
@Composable
private fun Tile(icon: Int, label: String, hint: String, dim: Boolean, enabled: Boolean, onTap: () -> Unit, onLeft: (() -> Unit)?, onRight: (() -> Unit)?) {
    val p = MaterialTheme.palette
    Column(Modifier.width(96.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(14.dp))
                .background(if (dim) p.panel else p.panel2)
                .clickable(enabled = enabled, role = Role.Button, onClick = onTap)
                .said("$label, $hint")
                .alpha(if (dim) 0.55f else 1f)
                .padding(vertical = 12.dp, horizontal = 6.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Icon(painterResource(icon), contentDescription = null, tint = p.text, modifier = Modifier.size(28.dp))
            Spacer(Modifier.height(6.dp))
            Text(label, style = MaterialTheme.typography.bodySmall, textAlign = TextAlign.Center, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text(hint, style = MaterialTheme.typography.labelSmall, color = p.muted, textAlign = TextAlign.Center, maxLines = 1)
        }
        if (onLeft != null || onRight != null) {
            Row(horizontalArrangement = Arrangement.Center, modifier = Modifier.fillMaxWidth()) {
                IconButton(onClick = { onLeft?.invoke() }, enabled = onLeft != null, modifier = Modifier.size(40.dp)) {
                    Icon(painterResource(R.drawable.ic_expand_more), contentDescription = "Move $label left", tint = if (onLeft != null) p.muted else p.border, modifier = Modifier.rotate(90f))
                }
                IconButton(onClick = { onRight?.invoke() }, enabled = onRight != null, modifier = Modifier.size(40.dp)) {
                    Icon(painterResource(R.drawable.ic_expand_more), contentDescription = "Move $label right", tint = if (onRight != null) p.muted else p.border, modifier = Modifier.rotate(-90f))
                }
            }
        } else {
            Spacer(Modifier.height(40.dp))
        }
    }
}

/** The Encore Lyrics strip's buttons on this page. */
private fun stripIconOf(b: LyricsStripRules.Btn): Int = when (b) {
    LyricsStripRules.Btn.PREVIOUS -> R.drawable.ic_skip_previous
    LyricsStripRules.Btn.PLAY -> R.drawable.ic_play
    LyricsStripRules.Btn.NEXT -> R.drawable.ic_skip_next
    LyricsStripRules.Btn.ARTIST -> R.drawable.ic_person
    LyricsStripRules.Btn.LIKE -> R.drawable.ic_favorite
    LyricsStripRules.Btn.SHUFFLE -> R.drawable.ic_shuffle
    LyricsStripRules.Btn.REPEAT -> R.drawable.ic_repeat
}

// ---- previews: roughly what the car screen looks like, drawn on the phone (no car needed) -------------------------

private val CarBg = Color(0xFF101114)
private val CarPanel = Color(0xFF202227)
private val CarText = Color(0xFFF2F2F2)
private val CarMuted = Color(0xFF9A9CA3)

/** A wide car screen (about 2.2 : 1, like most Android Auto displays) with a "Preview" caption under it. */
@Composable
private fun CarScreen(caption: String, content: @Composable () -> Unit) {
    Column(Modifier.padding(horizontal = Gutter).fillMaxWidth()) {
        Box(
            Modifier
                .fillMaxWidth()
                .aspectRatio(2.2f)
                .clip(RoundedCornerShape(16.dp))
                .background(CarBg)
                .clearAndSetSemantics { contentDescription = caption },
        ) { content() }
        Text(caption, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted, modifier = Modifier.padding(top = 6.dp, bottom = 8.dp))
    }
}

@Composable
private fun CarGlyph(res: Int, size: Int = 22, tint: Color = CarText) {
    Icon(painterResource(res), contentDescription = null, tint = tint, modifier = Modifier.size(size.dp))
}

/**
 * Android Auto's media player: the cover, the song, a progress bar, then Previous, Play, Next and the shown buttons
 * in [p] (with ⋯ when some don't fit the car's spots).
 */
@Composable
private fun CarPlayerPreview(p: CarButtonRules.Preview, song: String, artist: String) {
    val words = CarButtonRules.previewLine(p) + if (p.behind.isEmpty()) "" else ". " + CarButtonRules.behindLine(p)
    CarScreen("Preview – your car may fit more or fewer before ⋯. $words") {
        Row(Modifier.fillMaxSize().padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier.fillMaxHeight(0.8f).aspectRatio(1f).clip(RoundedCornerShape(10.dp)).background(CarPanel),
                contentAlignment = Alignment.Center,
            ) { CarGlyph(R.drawable.ic_album, 36, CarMuted) }
            Spacer(Modifier.width(14.dp))
            Column(Modifier.weight(1f)) {
                Text(song, color = CarText, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(artist, color = CarMuted, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.height(8.dp))
                Box(Modifier.fillMaxWidth().height(3.dp).clip(RoundedCornerShape(2.dp)).background(CarPanel)) {
                    Box(Modifier.fillMaxWidth(0.35f).height(3.dp).background(CarText))
                }
                Spacer(Modifier.height(10.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    CarGlyph(R.drawable.ic_skip_previous)
                    CarGlyph(R.drawable.ic_play, 26)
                    CarGlyph(R.drawable.ic_skip_next)
                    for (id in p.shown) CarGlyph(iconOf(id), 20, CarMuted)
                    if (p.behind.isNotEmpty()) CarGlyph(R.drawable.ic_more_horiz, 20, CarMuted)
                }
            }
        }
    }
}

/** Encore Lyrics: the lyrics over the whole screen, Android Auto's song card at the top left, the strip at the top right. */
@Composable
private fun LyricsPreview(strip: List<LyricsStripRules.Btn>, song: String, artist: String) {
    CarScreen("Preview – Encore Lyrics with " + LyricsStripRules.summary(strip)) {
        Box(Modifier.fillMaxSize().padding(10.dp)) {
            Column(Modifier.align(Alignment.CenterStart).padding(start = 8.dp, top = 28.dp).fillMaxWidth(0.9f)) {
                Text("The line before", color = CarMuted, fontSize = 11.sp, maxLines = 1)
                Text("The line playing now, large", color = CarText, fontSize = 17.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text("The next line", color = CarMuted, fontSize = 11.sp, maxLines = 1)
            }
            Row(
                Modifier.align(Alignment.TopStart).clip(RoundedCornerShape(8.dp)).background(CarPanel).padding(horizontal = 8.dp, vertical = 5.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                CarGlyph(R.drawable.ic_album, 16, CarMuted)
                Spacer(Modifier.width(6.dp))
                Column(Modifier.width(96.dp)) {
                    Text(song, color = CarText, fontSize = 10.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(artist, color = CarMuted, fontSize = 9.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                CarGlyph(R.drawable.ic_favorite_border, 14, CarMuted)
            }
            if (strip.isNotEmpty()) {
                Row(
                    Modifier.align(Alignment.TopEnd).clip(RoundedCornerShape(8.dp)).background(CarPanel).padding(horizontal = 6.dp, vertical = 5.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    for (b in strip) CarGlyph(stripIconOf(b), 16)
                }
            }
        }
    }
}
