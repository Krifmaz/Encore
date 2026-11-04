package app.encore.android.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.LongState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.unit.LayoutDirection
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.flow.collectLatest

/**
 * Where the song was at one moment and how it moves on from there: [PlayerConnection] publishes one on every
 * half-second tick and player event ([PlayerConnection.clock]), and the progress bars work out the position of each
 * frame from it ([positionAt]), so they move continuously instead of in half-second jumps.
 *
 * @property positionMs the position (song time) when it was read.
 * @property atNanos when it was read, on the [System.nanoTime] clock (the clock of Compose's frame times).
 * @property playing whether the song moves on (false while paused, stopped or buffering).
 * @property speed the playback speed (1 = normal).
 */
data class PlaybackClock(val positionMs: Long, val atNanos: Long, val playing: Boolean, val speed: Float) {
    /**
     * The position at [nanos] (a [System.nanoTime] time, e.g. a frame's): [positionMs] while not playing, moved on by
     * the time since [atNanos] at [speed] while playing. Never before [positionMs] (a frame time older than the read).
     */
    fun positionAt(nanos: Long): Long {
        if (!playing) return positionMs
        val elapsedMs = (nanos - atNanos).coerceAtLeast(0L) / 1_000_000.0
        return positionMs + (elapsedMs * speed).toLong()
    }
}

/**
 * The song's position as a state that changes on every frame while the song plays and [running] is true, and
 * otherwise only on the player's ticks and events. Read it in the draw phase (`drawBehind`) or through
 * `derivedStateOf`: reading it in a composable recomposes that composable on every frame.
 *
 * The frame loop runs only while the caller is in the composition and the window draws frames (Compose pauses its
 * frame clock while the app is in the background), so nothing animates off screen.
 *
 * @param running false while the caller is covered (the mini player under the full player): the position then only
 *   follows the ticks, with no frame loop.
 */
@Composable
fun rememberLivePosition(player: PlayerConnection, running: Boolean = true): LongState {
    val live = remember(player) { mutableLongStateOf(player.clock.value.positionMs) }
    LaunchedEffect(player, running) {
        player.clock.collectLatest { c ->
            live.longValue = c.positionMs
            if (!running || !c.playing) return@collectLatest
            // until the next tick (collectLatest cancels this loop when a new clock comes)
            while (true) withFrameNanos { now -> live.longValue = c.positionAt(now) }
        }
    }
    return live
}

/**
 * The time played ("1:23"), recomposed only when its shown second changes although [positionMs] changes on every
 * frame.
 */
@Composable
internal fun ElapsedTime(positionMs: () -> Long) {
    val p = MaterialTheme.palette
    val read by rememberUpdatedState(positionMs)
    val seconds by remember { derivedStateOf { read() / 1000L } }
    Text(formatDuration(seconds * 1000L).ifEmpty { "0:00" }, style = MaterialTheme.typography.bodySmall, color = p.faint)
}

/** How much of a song of [durationMs] is played at [positionMs] (0–1; 0 while the length is unknown). */
internal fun fractionOf(positionMs: Long, durationMs: Long): Float =
    if (durationMs > 0) (positionMs.toFloat() / durationMs).coerceIn(0f, 1f) else 0f

/**
 * Draws the played part of a progress line: [fraction] of the width from the start side (the right in a right-to-left
 * language), the full height, in [brush].
 */
internal fun DrawScope.drawPlayed(brush: Brush, fraction: Float) {
    val w = size.width * fraction
    val left = if (layoutDirection == LayoutDirection.Rtl) size.width - w else 0f
    drawRect(brush, topLeft = Offset(left, 0f), size = Size(w, size.height))
}
