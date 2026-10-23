/**
 * The focus ring: a visible outline on whatever has the focus when the app is driven by a D-pad (an Android TV /
 * Google TV remote) or a keyboard instead of touch.
 *
 * Touch never focuses a clickable, so phones look exactly as before; the ring only appears once a key moves the
 * focus. Two ways in:
 *
 * - [FocusRingIndication]: installed as `LocalIndication` by [EncoreTheme], so every `Modifier.clickable` /
 *   `combinedClickable` (song rows, album tiles, the mini player…) draws the ring on top of its usual ripple.
 * - [focusRing]: a modifier for Material components that pick their own ripple and ignore `LocalIndication`
 *   (the bottom navigation bar's items).
 */
package app.encore.android.ui.theme

import androidx.compose.foundation.Indication
import androidx.compose.foundation.IndicationNodeFactory
import androidx.compose.foundation.interaction.FocusInteraction
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.node.DelegatableNode
import androidx.compose.ui.node.DelegatingNode
import androidx.compose.ui.node.DrawModifierNode
import androidx.compose.ui.node.invalidateDraw
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

/** Width of the ring's stroke: thick enough to read from a sofa across the room. */
private val RingWidth: Dp = 3.dp

/** Corner radius of the ring, the app's 10 dp button / input radius (`EncoreShapes.small`). */
private val RingCorner: Dp = 10.dp

/**
 * Draws the focus ring inside this scope's bounds: a rounded outline of [color], inset by half the stroke so the
 * whole stroke stays inside the element (a parent that clips its children would otherwise cut the outer half).
 */
private fun DrawScope.drawFocusRing(color: Color) {
    val stroke = RingWidth.toPx()
    val half = stroke / 2
    drawRoundRect(
        color = color,
        topLeft = Offset(half, half),
        size = Size(size.width - stroke, size.height - stroke),
        cornerRadius = CornerRadius(RingCorner.toPx()),
        style = Stroke(width = stroke),
    )
}

/**
 * The app-wide indication: [inner] (Material's ripple, as `MaterialTheme` provides it) plus the focus ring in [color].
 *
 * @property inner the indication being wrapped; its pressed / hovered / focused effects are kept as they are. An
 *   old-style [Indication] that is not an [IndicationNodeFactory] is dropped (Material 3 only provides factories).
 * @property color the ring's colour, the theme's accent.
 */
class FocusRingIndication(private val inner: Indication, private val color: Color) : IndicationNodeFactory {
    /** Makes one element's node: the ring, with the wrapped indication's node delegated to it. */
    override fun create(interactionSource: InteractionSource): DelegatableNode =
        FocusRingNode(interactionSource, (inner as? IndicationNodeFactory)?.create(interactionSource), color)

    /** Equal when both wrap the same indication with the same colour, so Compose can keep the existing nodes. */
    override fun equals(other: Any?): Boolean =
        other is FocusRingIndication && other.inner == inner && other.color == color

    /** Consistent with [equals]. */
    override fun hashCode(): Int = 31 * inner.hashCode() + color.hashCode()
}

/**
 * One clickable element's indication node: follows the element's focus through its [interactionSource] and draws
 * the ring while it is focused, after (on top of) the content and the delegated ripple.
 *
 * @param interactionSource the element's interactions; only [FocusInteraction]s matter here.
 * @param inner the wrapped indication's node (the ripple), or null when there is none.
 * @param color the ring's colour.
 */
private class FocusRingNode(
    private val interactionSource: InteractionSource,
    inner: DelegatableNode?,
    private val color: Color,
) : DelegatingNode(), DrawModifierNode {
    /** Whether the element has the focus right now (drives [draw]). */
    private var focused = false

    init {
        // the ripple runs as our delegate: it gets the same lifecycle and draws in the same place as before
        inner?.let { delegate(it) }
    }

    /** Starts following the focus; the node's scope is cancelled when the element leaves the composition. */
    override fun onAttach() {
        coroutineScope.launch {
            // a count, not a flag: a Focus can be emitted again before its Unfocus (as Material's own ripple assumes)
            var focusCount = 0
            interactionSource.interactions.collect { interaction ->
                when (interaction) {
                    is FocusInteraction.Focus -> focusCount++
                    is FocusInteraction.Unfocus -> focusCount--
                }
                val now = focusCount > 0
                if (now != focused) {
                    focused = now
                    invalidateDraw()
                }
            }
        }
    }

    /** The content, then the ring while focused. */
    override fun ContentDrawScope.draw() {
        drawContent()
        if (focused) drawFocusRing(color)
    }
}

/**
 * The focus ring for a component that ignores `LocalIndication` (Material 3's navigation bar items, which always use
 * their own ripple): watches the focus of the focusable inside this modifier and outlines it in [color].
 *
 * Put it before (outside) the component's own clickable, i.e. pass it as the component's `modifier`.
 *
 * @param color the ring's colour, normally the palette's accent.
 */
@Composable
fun Modifier.focusRing(color: Color): Modifier {
    var focused by remember { mutableStateOf(false) }
    return this
        .onFocusChanged { focused = it.hasFocus }
        .drawWithContent {
            drawContent()
            if (focused) drawFocusRing(color)
        }
}
