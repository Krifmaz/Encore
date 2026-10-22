package app.encore.android.ui.screens

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import app.encore.android.R
import app.encore.android.playback.CarButtonRules
import app.encore.android.playback.CarButtonRules.Group
import app.encore.android.playback.CarButtonRules.Id
import app.encore.android.playback.CarButtonRules.Layout
import app.encore.android.playback.CarPrefs
import app.encore.android.playback.CarRules
import app.encore.android.playback.LyricsStripRules
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette

// Settings → Android Auto (and Settings → Playback → In the car → Car buttons). The rules (what is sent, what a car shows, the tags and lines) are
// in playback/CarButtonRules.kt; the setting is stored on this phone (CarPrefs.carButtons / carSpots) and a connected
// car follows it at once (CarControls collects CarPrefs.carButtons). Below them, the Encore Lyrics screen's strip
// (LyricsStripRules, CarPrefs.lyricsStrip), which the open car app follows at once too.

private const val EXPLAIN =
    "Android Auto fills its spots in the order below and puts the rest behind ⋯, so while there are free spots the first buttons in the ⋯ menu show on the screen too."
private const val SPOTS_HELP =
    "Count the button spots beside Previous, Play and Next on your car's screen, with ⋯ as one. Only used for the preview below."
private const val DRAG_HELP = "Use the arrows or the ⋮ menu, or hold a button and drag it."
private const val FOOTER =
    "Only on this phone. While the phone is connected to Android Auto, the phone's own media controls show the same buttons."

/** The button's icon on this page (the car draws Media3's own icons or the same drawables). */
private fun iconOf(id: Id): Int = when (id) {
    Id.SHUFFLE -> R.drawable.ic_shuffle
    Id.REPEAT -> R.drawable.ic_repeat
    Id.LIKE -> R.drawable.ic_favorite
    Id.LYRICS -> R.drawable.ic_lyrics
    Id.ARTIST -> R.drawable.ic_person
    Id.SIGNAL -> R.drawable.ic_signal
}

/** The ⋮ menu's entry that moves a button into [g]. */
private fun moveText(g: Group): String = when (g) {
    Group.SCREEN -> "Move to the car screen"
    Group.MENU -> "Move to the ⋯ menu"
    Group.HIDDEN -> "Hide"
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
 * The "Car buttons" row of Settings → Playback → In the car: what the car shows now
 * ("Shuffle · Repeat · Like · Lyrics") and the way to [CarButtonsScreen].
 */
@Composable
fun CarButtonsRow(onOpen: () -> Unit) {
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val layout by remember { CarPrefs.carButtons(context) }.collectAsState()
    val spots by remember { CarPrefs.carSpots(context) }.collectAsState()
    val summary = CarButtonRules.summary(layout, spots)
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

/**
 * Settings → Playback → In the car → Car buttons: which buttons Android Auto gets beside Previous, Play and Next and
 * in what order, in three groups ("On the car screen", "In the ⋯ menu", "Hidden"). The listener's order is kept
 * exactly; the preview shows, for the car's number of spots, what the car will show and what lands behind ⋯.
 */
@Composable
fun CarButtonsScreen(onBack: () -> Unit, title: String = "Car buttons") {
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val layout by remember { CarPrefs.carButtons(context) }.collectAsState()
    val spots by remember { CarPrefs.carSpots(context) }.collectAsState()
    // what Reset to default replaced (CarButtonRules.encode), for "Undo": kept until the next change or until the
    // page is left, with no timer, so it can be reached with TalkBack as well
    var undo by rememberSaveable { mutableStateOf<String?>(null) }
    // the button moved last (its key): where it went is shown under the list and said by TalkBack
    var moved by rememberSaveable { mutableStateOf<String?>(null) }
    val set: (Layout, Id) -> Unit = { l, id ->
        if (l != layout) {
            CarPrefs.setCarButtons(context, l)
            moved = id.key
            undo = null
        }
    }
    BackHandler(onBack = onBack)
    val scroll = rememberScrollState()

    Column(Modifier.fillMaxSize()) {
        BackBar(title, onBack)
        Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(horizontal = 16.dp)) {
            HelpLine(EXPLAIN)
            SpotsRow(spots) { n -> CarPrefs.setCarSpots(context, n) }
            HelpLine(SPOTS_HELP)
            PreviewCard(layout, spots)
            HelpLine(DRAG_HELP)
            ButtonList(layout, spots, scroll, set)
            // the buttons only: "Spots in your car" describes the car, so Reset leaves it alone
            TextButton(
                onClick = {
                    undo = CarButtonRules.encode(layout)
                    moved = null
                    CarPrefs.setCarButtons(context, CarButtonRules.DEFAULT)
                },
                enabled = layout != CarButtonRules.DEFAULT,
                modifier = Modifier.padding(top = 12.dp),
            ) { Text("Reset to default") }
            // after Reset: "Back to the default buttons" and Undo; after a move: where the button went. One live
            // region, so TalkBack says each change; the arrows and the ⋮ menu otherwise move a button silently.
            val u = undo?.takeIf { layout == CarButtonRules.DEFAULT }
            val movedId = CarButtonRules.Id.entries.firstOrNull { it.key == moved }
            val line = when {
                u != null -> "Back to the default buttons"
                movedId != null -> CarButtonRules.movedLine(layout, movedId, spots)
                else -> null
            }
            if (line != null) {
                Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(line, Modifier.weight(1f).said(line, live = true), style = MaterialTheme.typography.bodySmall, color = p.muted)
                    if (u != null) {
                        TextButton(onClick = {
                            CarPrefs.setCarButtons(context, CarButtonRules.decode(u))
                            undo = null
                        }) { Text("Undo") }
                    }
                }
            }
            HelpLine(FOOTER)
            LyricsStripSection()
            Spacer(Modifier.height(32.dp))
        }
    }
}

@Composable
private fun HelpLine(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted, modifier = Modifier.padding(top = 4.dp, bottom = 4.dp).said(text))
}

/** "Spots in your car  [−] 4 [+]" (2 to 8). TalkBack: "Spots in your car: 4", said again when the count changes. */
@Composable
private fun SpotsRow(spots: Int, onChange: (Int) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("Spots in your car", Modifier.weight(1f).said("Spots in your car: $spots", live = true), style = MaterialTheme.typography.bodyLarge)
        StepButton("−", "Fewer spots", enabled = spots > CarButtonRules.SPOTS.first) { onChange(spots - 1) }
        Text(
            spots.toString(),
            Modifier.width(40.dp).clearAndSetSemantics {},
            textAlign = TextAlign.Center,
            style = MaterialTheme.typography.titleMedium,
            fontWeight = FontWeight.SemiBold,
        )
        StepButton("+", "More spots", enabled = spots < CarButtonRules.SPOTS.last) { onChange(spots + 1) }
    }
}

@Composable
private fun StepButton(text: String, description: String, enabled: Boolean, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    Box(
        Modifier
            .size(40.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(p.panel2)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = MaterialTheme.typography.titleMedium, color = if (enabled) p.text else p.faint)
    }
}

/**
 * What the car will show: Previous, Play and Next, then the buttons in its spots and ⋯ when some don't fit, as icons
 * and in words ([CarButtonRules.previewLine], [CarButtonRules.behindLine]); Like behind ⋯ is said plainly.
 */
@Composable
private fun PreviewCard(layout: Layout, spots: Int) {
    val p = MaterialTheme.palette
    val pv = CarButtonRules.preview(layout, spots)
    Column(
        Modifier
            .fillMaxWidth()
            .padding(vertical = 10.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(p.panel)
            .border(1.dp, p.border, RoundedCornerShape(12.dp))
            .padding(12.dp),
    ) {
        // the icons are a picture of the row; the lines below say the same in words
        Row(
            Modifier.horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            for (icon in listOf(R.drawable.ic_skip_previous, R.drawable.ic_play, R.drawable.ic_skip_next)) Chip(icon, transport = true)
            Spacer(Modifier.width(6.dp))
            for (id in pv.shown) Chip(iconOf(id))
            if (pv.behind.isNotEmpty()) Chip(R.drawable.ic_more_horiz)
        }
        val previewLine = CarButtonRules.previewLine(pv)
        Text(
            previewLine,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.padding(top = 10.dp).said(previewLine),
        )
        val behind = CarButtonRules.behindLine(pv)
        if (behind.isNotEmpty()) Text(behind, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.said(behind))
        // a live region: a move further down the page that pushes Like behind ⋯ is said at once
        CarButtonRules.likeLine(pv)?.let {
            Text(it, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn, modifier = Modifier.padding(top = 6.dp).said(it, live = true))
        }
    }
}

@Composable
private fun Chip(icon: Int, transport: Boolean = false) {
    val p = MaterialTheme.palette
    Box(
        Modifier.size(28.dp).clip(RoundedCornerShape(8.dp)).background(if (transport) p.bg2 else p.panel2),
        contentAlignment = Alignment.Center,
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = if (transport) p.muted else p.text, modifier = Modifier.size(18.dp))
    }
}

/**
 * The three groups and their buttons ([CarButtonRules.flatten]). Each button moves with ▲ / ▼ (across a group's edge
 * into the next group), with its ⋮ menu, or by holding it and dragging it ([CarButtonRules.dropAt] on the row it is
 * let go over). [scroll] is the page's: a drag held near the top or bottom of the visible page scrolls it.
 */
@Composable
private fun ButtonList(layout: Layout, spots: Int, scroll: ScrollState, set: (Layout, Id) -> Unit) {
    val rows = CarButtonRules.flatten(layout)
    val tags = CarButtonRules.tags(layout, spots)
    val current by rememberUpdatedState(layout)
    val onSet by rememberUpdatedState(set)
    val haptics = LocalHapticFeedback.current
    // each row's top and height in this column (px), by its place in [rows]; only read when a drag ends
    val tops = remember { HashMap<Int, Float>() }
    val heights = remember { HashMap<Int, Int>() }
    var dragging by remember { mutableStateOf<Id?>(null) }
    var dy by remember { mutableFloatStateOf(0f) }
    var dragTop by remember { mutableFloatStateOf(0f) }
    // this column's top in the page's scrolling content, and the finger's height in the visible page while dragging
    var listTop by remember { mutableFloatStateOf(0f) }
    var finger by remember { mutableFloatStateOf(Float.NaN) }
    val density = LocalDensity.current
    val edge = with(density) { 56.dp.toPx() }
    val step = with(density) { 12.dp.toPx() }

    // while a drag holds the finger near the top or bottom of the visible page, the page scrolls, faster nearer the
    // edge; the held row stays under the finger (dy grows by what was scrolled), so a button reaches groups off screen
    LaunchedEffect(dragging) {
        if (dragging == null) return@LaunchedEffect
        while (true) {
            withFrameNanos { }
            val f = finger
            val h = scroll.viewportSize.toFloat()
            if (f.isNaN() || h <= 0f) continue
            val delta = when {
                f < edge -> -step * ((edge - f) / edge).coerceIn(0f, 1f)
                f > h - edge -> step * ((f - (h - edge)) / edge).coerceIn(0f, 1f)
                else -> 0f
            }
            if (delta != 0f) dy += scroll.dispatchRawDelta(delta)
        }
    }

    /** Where [id] goes when let go: the row under its middle (its own place counts as where it started). */
    fun drop(id: Id) {
        val now = current
        val flat = CarButtonRules.flatten(now)
        val from = flat.indexOf(CarButtonRules.Row.Button(id))
        if (from < 0) return
        val h = heights[from] ?: return
        val middle = dragTop + h / 2f + dy
        var target = -1
        for (k in flat.indices) {
            val top = if (k == from) dragTop else tops[k] ?: continue
            val height = heights[k] ?: continue
            if (middle >= top && middle < top + height) { target = k; break }
        }
        if (target < 0) target = if (middle < (tops[0] ?: 0f)) 0 else flat.size - 1
        onSet(CarButtonRules.dropAt(now, id, target), id)
    }

    Column(Modifier.fillMaxWidth().onGloballyPositioned { c -> listTop = c.positionInParent().y }) {
        rows.forEachIndexed { i, row ->
            key(row) {
                val measured = Modifier.onGloballyPositioned { c ->
                    tops[i] = c.positionInParent().y
                    heights[i] = c.size.height
                }
                when (row) {
                    is CarButtonRules.Row.Header -> GroupHeader(row.group, layout, spots, measured)
                    is CarButtonRules.Row.Button -> {
                        val id = row.id
                        val held = dragging == id
                        ButtonRow(
                            id, layout, tags[id], held, { l -> set(l, id) },
                            measured
                                .zIndex(if (held) 1f else 0f)
                                .graphicsLayer { translationY = if (held) dy else 0f }
                                .pointerInput(id) {
                                    detectDragGesturesAfterLongPress(
                                        onDragStart = {
                                            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                            val at = CarButtonRules.flatten(current).indexOf(CarButtonRules.Row.Button(id))
                                            dragTop = tops[at] ?: 0f
                                            dy = 0f
                                            dragging = id
                                        },
                                        onDragEnd = {
                                            drop(id)
                                            dragging = null
                                            dy = 0f
                                            finger = Float.NaN
                                        },
                                        onDragCancel = {
                                            dragging = null
                                            dy = 0f
                                            finger = Float.NaN
                                        },
                                        onDrag = { change, amount ->
                                            change.consume()
                                            // the position is in the held row, which is drawn dy lower
                                            finger = listTop + dragTop + dy + change.position.y - scroll.value
                                            dy += amount.y
                                        },
                                    )
                                },
                        )
                    }
                }
            }
        }
    }
}

/** A group's title; the screen group's with the spots it fills ("4 of 4 spots", "3 of 3 spots + ⋯"). */
@Composable
private fun GroupHeader(g: Group, layout: Layout, spots: Int, modifier: Modifier) {
    val p = MaterialTheme.palette
    val capacity = if (g == Group.SCREEN) CarButtonRules.capacityLine(layout, spots) else null
    Column(modifier.fillMaxWidth().padding(top = 16.dp, bottom = 4.dp)) {
        // one heading for TalkBack, so its users can jump from group to group
        Row(
            Modifier.clearAndSetSemantics {
                heading()
                contentDescription = CarButtonRules.spoken(listOfNotNull(g.title, capacity).joinToString(", "))
            },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(g.title, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
            if (capacity != null) {
                Text(" · $capacity", style = MaterialTheme.typography.bodySmall, color = p.muted)
            }
        }
        if (layout.of(g).isEmpty()) {
            Text("None", style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(top = 6.dp, bottom = 4.dp))
        }
    }
}

/** One button: its icon, name and where it lands in the car (its tag), ▲ / ▼ and the ⋮ menu. Hidden ones greyed. */
@Composable
private fun ButtonRow(id: Id, layout: Layout, tag: CarButtonRules.Tag?, held: Boolean, set: (Layout) -> Unit, modifier: Modifier) {
    val p = MaterialTheme.palette
    val group = layout.groupOf(id)
    var menu by remember { mutableStateOf(false) }
    Row(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(if (held) p.panel2 else Color.Transparent)
            .padding(start = 4.dp, top = 2.dp, bottom = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Row(
            Modifier
                .weight(1f)
                .alpha(if (group == Group.HIDDEN) 0.5f else 1f)
                .said(listOfNotNull(id.label, tag?.text).joinToString(", ")),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(painterResource(iconOf(id)), contentDescription = null, tint = p.text, modifier = Modifier.size(22.dp))
            Spacer(Modifier.width(12.dp))
            Column {
                Text(id.label, style = MaterialTheme.typography.bodyLarge)
                if (tag != null) {
                    Text(
                        tag.text,
                        style = MaterialTheme.typography.bodySmall,
                        color = if (tag == CarButtonRules.Tag.LIKE_BEHIND) EncoreColors.Warn else p.muted,
                    )
                }
            }
        }
        val canUp = CarButtonRules.canUp(layout, id)
        val canDown = CarButtonRules.canDown(layout, id)
        IconButton(onClick = { set(CarButtonRules.up(layout, id)) }, enabled = canUp) {
            Icon(painterResource(R.drawable.ic_arrow_upward), contentDescription = "Move ${id.label} up", tint = if (canUp) p.muted else p.border)
        }
        IconButton(onClick = { set(CarButtonRules.down(layout, id)) }, enabled = canDown) {
            Icon(painterResource(R.drawable.ic_arrow_downward), contentDescription = "Move ${id.label} down", tint = if (canDown) p.muted else p.border)
        }
        Box {
            IconButton(onClick = { menu = true }) {
                Icon(painterResource(R.drawable.ic_more_vert), contentDescription = "Move ${id.label} to another group", tint = p.muted)
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                for (g in Group.entries) {
                    if (g == group) continue
                    DropdownMenuItem(text = { Text(moveText(g), Modifier.said(moveText(g))) }, onClick = {
                        menu = false
                        set(CarButtonRules.move(layout, id, g))
                    })
                }
            }
        }
    }
}

private const val LYRICS_HELP =
    "Encore Lyrics (in the car's app list) draws the lyrics over the whole screen itself. Android Auto still adds a song card and one strip of up to 4 buttons, so pick them and their order here."
private const val LYRICS_MEDIA_HELP =
    "The player Android Auto shows for Encore (above) is drawn by Android Auto, not by Encore: only its buttons can be changed."

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

/**
 * "Encore Lyrics screen": which of [LyricsStripRules.Btn] the Encore Lyrics car app's strip has (up to
 * [LyricsStripRules.MAX]) and in what order. A tick turns one on or off; ▲ / ▼ move the ones that are on.
 */
@Composable
private fun LyricsStripSection() {
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val strip by remember { CarPrefs.lyricsStrip(context) }.collectAsState()
    val set: (List<LyricsStripRules.Btn>) -> Unit = { CarPrefs.setLyricsStrip(context, it) }

    Text(
        "Encore Lyrics screen",
        style = MaterialTheme.typography.titleMedium,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier.padding(top = 24.dp, bottom = 4.dp).semantics { heading() },
    )
    HelpLine(LYRICS_HELP)
    HelpLine(LYRICS_MEDIA_HELP)
    // a picture of the strip; the summary says the same in words
    Column(
        Modifier
            .fillMaxWidth()
            .padding(vertical = 10.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(p.panel)
            .border(1.dp, p.border, RoundedCornerShape(12.dp))
            .padding(12.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
            if (strip.isEmpty()) Text("No buttons", style = MaterialTheme.typography.bodySmall, color = p.faint)
            for (b in strip) Chip(stripIconOf(b))
        }
        val summary = LyricsStripRules.summary(strip)
        Text(summary, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 10.dp).said(summary, live = true))
    }
    for (b in LyricsStripRules.rows(strip)) {
        key(b) {
            val on = b in strip
            val canToggle = LyricsStripRules.canAdd(strip, b)
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable(enabled = canToggle, role = Role.Checkbox) { set(LyricsStripRules.toggle(strip, b)) }
                    .padding(vertical = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Checkbox(checked = on, onCheckedChange = null, enabled = canToggle)
                Row(
                    Modifier.weight(1f).alpha(if (on) 1f else 0.5f).said(b.label + if (on) ", on the strip" else ""),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(painterResource(stripIconOf(b)), contentDescription = null, tint = p.text, modifier = Modifier.size(22.dp))
                    Spacer(Modifier.width(12.dp))
                    Text(b.label, style = MaterialTheme.typography.bodyLarge)
                }
                if (on) {
                    val i = strip.indexOf(b)
                    val canUp = i > 0
                    val canDown = i < strip.lastIndex
                    IconButton(onClick = { set(LyricsStripRules.up(strip, b)) }, enabled = canUp) {
                        Icon(painterResource(R.drawable.ic_arrow_upward), contentDescription = "Move ${b.label} left", tint = if (canUp) p.muted else p.border)
                    }
                    IconButton(onClick = { set(LyricsStripRules.down(strip, b)) }, enabled = canDown) {
                        Icon(painterResource(R.drawable.ic_arrow_downward), contentDescription = "Move ${b.label} right", tint = if (canDown) p.muted else p.border)
                    }
                }
            }
        }
    }
    HelpLine(LyricsStripRules.roomLine(strip))
    TextButton(onClick = { set(LyricsStripRules.DEFAULT) }, enabled = strip != LyricsStripRules.DEFAULT) { Text("Reset to default") }
}
