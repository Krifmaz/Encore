/**
 * "Screen off" for the TV player: an app can't switch a TV off (that takes the system, or the TV's own remote), so
 * this blacks the whole screen out while the music plays on. On an OLED TV black pixels are off; on others the
 * picture is at least dark. Any button on the remote brings the player back, except the playback buttons, which
 * keep controlling the music without lighting the screen up.
 */
package app.encore.android.ui.tv

import android.view.KeyEvent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.focusable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.nativeKeyCode
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.ui.Nav
import kotlinx.coroutines.delay

/** What one remote button does while the screen is off ([ScreenOffRules.actionOf]). */
enum class ScreenOffAction {
    /** Play or pause; the screen stays black. */
    TOGGLE_PLAY,

    /** Play; the screen stays black. */
    PLAY,

    /** Pause (or stop); the screen stays black. */
    PAUSE,

    /** The next song; the screen stays black. */
    NEXT,

    /** The previous song; the screen stays black. */
    PREVIOUS,

    /** Left to the system (volume, mute, power): the screen stays black and the TV handles the button. */
    SYSTEM,

    /** Any other button (OK, the arrows, Back, Home's neighbours…): the screen comes back on. */
    WAKE,
}

/** The plain rules of Screen off, tested without a device (TvRulesTest). */
object ScreenOffRules {
    /** How long the dim "press any button" hint shows after the screen goes black, in ms; then it is all black. */
    const val HINT_MS = 4_000L

    /**
     * What a remote button does while the screen is off.
     *
     * @param keyCode the button's Android key code (`KeyEvent.KEYCODE_…`).
     */
    fun actionOf(keyCode: Int): ScreenOffAction = when (keyCode) {
        KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE, KeyEvent.KEYCODE_HEADSETHOOK -> ScreenOffAction.TOGGLE_PLAY
        KeyEvent.KEYCODE_MEDIA_PLAY -> ScreenOffAction.PLAY
        KeyEvent.KEYCODE_MEDIA_PAUSE, KeyEvent.KEYCODE_MEDIA_STOP -> ScreenOffAction.PAUSE
        KeyEvent.KEYCODE_MEDIA_NEXT, KeyEvent.KEYCODE_MEDIA_FAST_FORWARD -> ScreenOffAction.NEXT
        KeyEvent.KEYCODE_MEDIA_PREVIOUS, KeyEvent.KEYCODE_MEDIA_REWIND -> ScreenOffAction.PREVIOUS
        KeyEvent.KEYCODE_VOLUME_UP, KeyEvent.KEYCODE_VOLUME_DOWN, KeyEvent.KEYCODE_VOLUME_MUTE,
        KeyEvent.KEYCODE_POWER, KeyEvent.KEYCODE_SLEEP, KeyEvent.KEYCODE_WAKEUP, KeyEvent.KEYCODE_HOME,
        -> ScreenOffAction.SYSTEM
        else -> ScreenOffAction.WAKE
    }
}

/**
 * The black screen of Screen off, over everything while [on]. It takes the remote's focus and keeps every button
 * from reaching the player underneath: playback buttons act on the music ([ScreenOffRules.actionOf]), volume and power
 * go to the TV, and any other button calls [onWake] when it is let go (so the same press doesn't also reach the
 * button that gets the focus back). For the first few seconds a dim line says how to get back; then it is all black.
 *
 * @param on whether the screen is off.
 * @param nav plays, pauses and skips for the playback buttons.
 * @param onWake turns the screen back on.
 */
@Composable
fun ScreenOffLayer(on: Boolean, nav: Nav, onWake: () -> Unit) {
    if (!on) return
    val focus = remember { FocusRequester() }
    var hint by remember { mutableStateOf(true) }
    LaunchedEffect(Unit) {
        try { focus.requestFocus() } catch (_: IllegalStateException) { /* not laid out yet */ }
        delay(ScreenOffRules.HINT_MS)
        hint = false
    }
    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            .focusRequester(focus)
            .onPreviewKeyEvent { e ->
                when (ScreenOffRules.actionOf(e.key.nativeKeyCode)) {
                    // the TV's own buttons: not ours
                    ScreenOffAction.SYSTEM -> false
                    ScreenOffAction.WAKE -> {
                        if (e.type == KeyEventType.KeyUp) onWake()
                        true
                    }
                    else -> {
                        // a playback button acts once, on its press; its release is swallowed too
                        if (e.type == KeyEventType.KeyDown) when (ScreenOffRules.actionOf(e.key.nativeKeyCode)) {
                            ScreenOffAction.TOGGLE_PLAY -> nav.player.togglePlay()
                            ScreenOffAction.PLAY -> if (!nav.player.isPlaying.value) nav.player.togglePlay()
                            ScreenOffAction.PAUSE -> if (nav.player.isPlaying.value) nav.player.togglePlay()
                            ScreenOffAction.NEXT -> nav.player.next()
                            ScreenOffAction.PREVIOUS -> nav.player.previous()
                            else -> {}
                        }
                        true
                    }
                }
            }
            .focusable(),
        contentAlignment = Alignment.BottomCenter,
    ) {
        AnimatedVisibility(hint, enter = fadeIn(tween(0)), exit = fadeOut(tween(1200))) {
            Text(
                "Screen off · press any button to turn it back on",
                color = Color(0xFF555555),
                fontSize = 14.sp,
                modifier = Modifier.padding(bottom = 32.dp),
            )
        }
    }
}
