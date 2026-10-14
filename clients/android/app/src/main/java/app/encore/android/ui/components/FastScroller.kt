package app.encore.android.ui.components

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.int
import app.encore.android.data.api.objects
import app.encore.android.data.api.str
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.ceil
import kotlin.math.max

/** One section of a library list: its label ("A", "2026", "Oct 2026", "<2 min") and its first row's position. */
data class IndexSection(val label: String, val offset: Int)

/**
 * GET /api/library/index?kind=&sort=&dir=&artist=: the sections of a library list in list order, where each
 * offset is the row's position in the same list endpoint with the same sort (see server routes/library_index.rs).
 * kind: albums | artists | tracks. An empty list when the server is older than the index.
 */
fun EncoreApi.libraryIndex(kind: String, sort: String, dir: String = "", artist: String = ""): List<IndexSection> {
    val qs = StringBuilder("/api/library/index?kind=").append(EncoreApi.enc(kind)).append("&sort=").append(EncoreApi.enc(sort))
    if (dir.isNotEmpty()) qs.append("&dir=").append(EncoreApi.enc(dir))
    if (artist.isNotEmpty()) qs.append("&artist=").append(EncoreApi.enc(artist))
    return get(qs.toString()).objects().mapNotNull { o ->
        val label = o.str("label")
        if (label.isEmpty()) null else IndexSection(label, o.int("offset").coerceAtLeast(0))
    }
}

/** The sections for a list, loaded again when [kind], [sort] or [artist] change; empty while loading or on errors. */
@Composable
fun rememberLibraryIndex(api: EncoreApi, kind: String, sort: String, artist: String = ""): List<IndexSection> {
    var sections by remember(kind, sort, artist) { mutableStateOf<List<IndexSection>>(emptyList()) }
    LaunchedEffect(kind, sort, artist) {
        sections = try {
            withContext(Dispatchers.IO) { api.libraryIndex(kind, sort, artist = artist) }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            emptyList()
        }
    }
    return sections
}

/**
 * What a [FastScroller] reads from and moves: a lazy list or grid, with [header] items before the first row
 * (positions in the scroller are row positions, as the index gives them).
 */
class ScrollerTarget(
    val firstVisible: () -> Int,
    val visibleCount: () -> Int,
    val scrolling: () -> Boolean,
    val scrollTo: suspend (Int) -> Unit,
)

@Composable
fun rememberScrollerTarget(state: LazyGridState, header: Int = 0): ScrollerTarget = remember(state, header) {
    ScrollerTarget(
        firstVisible = { (state.firstVisibleItemIndex - header).coerceAtLeast(0) },
        visibleCount = { state.layoutInfo.visibleItemsInfo.size },
        scrolling = { state.isScrollInProgress },
        scrollTo = { state.scrollToItem(it + header) },
    )
}

@Composable
fun rememberScrollerTarget(state: LazyListState, header: Int = 0): ScrollerTarget = remember(state, header) {
    ScrollerTarget(
        firstVisible = { (state.firstVisibleItemIndex - header).coerceAtLeast(0) },
        visibleCount = { state.layoutInfo.visibleItemsInfo.size },
        scrolling = { state.isScrollInProgress },
        scrollTo = { state.scrollToItem(it + header) },
    )
}

/** The section a row is in. */
private fun sectionAt(sections: List<IndexSection>, row: Int): Int {
    var at = 0
    for (i in sections.indices) {
        if (sections[i].offset <= row) at = i else break
    }
    return at
}

/**
 * The library's side scrubber: a slim rounded rail of section labels on the right edge that fades in while
 * the list scrolls and stays while it is dragged. Touch or drag → a large bubble with the label beside the
 * finger, a haptic tick per section, and a jump to the section's first row ([onJump] first, so a paged list
 * can make room for it, then the list scrolls there). When there are more sections than fit, every Nth label
 * is shown and the others still jump precisely. Hidden for lists shorter than about two screens.
 *
 * Place it in a Box over the list: `Modifier.align(Alignment.CenterEnd)`.
 */
@Composable
fun FastScroller(
    sections: List<IndexSection>,
    target: ScrollerTarget,
    extent: Int,
    modifier: Modifier = Modifier,
    onJump: (Int) -> Unit = {},
) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    val density = LocalDensity.current
    val currentTarget by rememberUpdatedState(target)
    val currentJump by rememberUpdatedState(onJump)
    val scrolling by remember(target) { derivedStateOf { target.scrolling() } }
    val visibleCount by remember(target) { derivedStateOf { target.visibleCount() } }
    val first by remember(target) { derivedStateOf { target.firstVisible() } }
    var dragging by remember { mutableStateOf(false) }
    var shown by remember { mutableStateOf(false) }
    var picked by remember(sections) { mutableIntStateOf(-1) }
    var touchY by remember { mutableFloatStateOf(0f) }
    var railHeight by remember { mutableIntStateOf(0) }
    var railWidth by remember { mutableIntStateOf(0) }

    LaunchedEffect(scrolling, dragging) {
        if (scrolling || dragging) {
            shown = true
        } else {
            delay(1500)
            shown = false
        }
    }
    val fade by animateFloatAsState(if (shown) 1f else 0f, tween(250), label = "scroller")

    val rows = max(extent, (sections.lastOrNull()?.offset ?: 0) + 1)
    val enabled = sections.size >= 2 && visibleCount > 0 && rows >= visibleCount * 2
    if (!enabled) return

    val current = if (dragging && picked in sections.indices) picked else sectionAt(sections, first)
    val labelPx = with(density) { 16.dp.toPx() }
    val step = if (railHeight > 0) max(1, ceil(sections.size * labelPx / railHeight).toInt()) else max(1, sections.size)
    val pillShape = RoundedCornerShape(50)

    Box(modifier.fillMaxHeight().padding(top = 8.dp, bottom = 8.dp, end = 2.dp).graphicsLayer { alpha = fade }) {
        Box(
            Modifier
                .align(Alignment.CenterEnd)
                .fillMaxHeight()
                .onSizeChanged { railHeight = it.height; railWidth = it.width }
                // a wider touch area than the rail; no touches while hidden, so taps reach the list
                .then(
                    if (shown || dragging) {
                        Modifier.pointerInput(sections) {
                            fun pick(y: Float) {
                                val h = size.height.coerceAtLeast(1)
                                touchY = y.coerceIn(0f, h.toFloat())
                                val i = ((touchY / h) * sections.size).toInt().coerceIn(0, sections.lastIndex)
                                if (i != picked) {
                                    picked = i
                                    haptics.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                                    val off = sections[i].offset
                                    currentJump(off)
                                    val t = currentTarget
                                    scope.launch { t.scrollTo(off) }
                                }
                            }
                            awaitEachGesture {
                                val down = awaitFirstDown(requireUnconsumed = false)
                                down.consume()
                                dragging = true
                                picked = -1
                                try {
                                    pick(down.position.y)
                                    while (true) {
                                        val ev = awaitPointerEvent()
                                        val ch = ev.changes.firstOrNull { it.id == down.id } ?: break
                                        if (!ch.pressed) break
                                        ch.consume()
                                        pick(ch.position.y)
                                    }
                                } finally {
                                    dragging = false
                                }
                            }
                        }
                    } else Modifier,
                )
                .padding(start = 14.dp),
        ) {
            // the rail: one slot per shown label, weighted by the sections it covers, so a label sits where its section starts
            Column(
                Modifier
                    .fillMaxHeight()
                    .widthIn(min = 22.dp)
                    .background(p.panel.copy(alpha = 0.9f), pillShape)
                    .border(1.dp, p.border, pillShape)
                    .padding(horizontal = 6.dp, vertical = 10.dp),
                horizontalAlignment = Alignment.End,
                verticalArrangement = Arrangement.Top,
            ) {
                var start = 0
                while (start < sections.size) {
                    val end = minOf(sections.size, start + step)
                    val here = current in start until end
                    val label = if (here) sections[current].label else sections[start].label
                    Box(Modifier.weight((end - start).toFloat()), contentAlignment = Alignment.TopEnd) {
                        Text(
                            label,
                            color = if (here) p.accent else p.muted,
                            fontSize = if (here) 11.sp else 10.sp,
                            fontWeight = if (here) FontWeight.Bold else FontWeight.SemiBold,
                            maxLines = 1,
                            softWrap = false,
                        )
                    }
                    start = end
                }
            }
        }
        // the bubble beside the finger
        if (dragging && picked in sections.indices) {
            val bubble = 64.dp
            Box(
                Modifier
                    .align(Alignment.TopEnd)
                    .offset {
                        IntOffset(
                            -railWidth,   // the touch area's 14dp gap is the space to the rail
                            (touchY - bubble.toPx() / 2).toInt(),
                        )
                    }
                    .heightIn(min = bubble)
                    .widthIn(min = bubble)
                    .shadow(8.dp, RoundedCornerShape(topStart = 32.dp, topEnd = 32.dp, bottomStart = 32.dp, bottomEnd = 8.dp))
                    .background(p.gradient, RoundedCornerShape(topStart = 32.dp, topEnd = 32.dp, bottomStart = 32.dp, bottomEnd = 8.dp))
                    .padding(horizontal = 16.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(sections[picked].label, color = p.onAccent, fontSize = 24.sp, fontWeight = FontWeight.Bold, maxLines = 1, softWrap = false)
            }
        }
    }
}
