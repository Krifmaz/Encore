/**
 * "Screen off" for the TV player: an app can't switch a TV off (that takes the system, or the TV's own remote), so
 * this blacks the whole screen out while the music plays on. On an OLED TV black pixels are off; on others the
 * picture is at least dark. Any button on the remote brings the player back, except the playback buttons, which
 * keep controlling the music without lighting the screen up.
 *
 * Screen off is turned on by its button in the player's top bar, or by itself after the remote has sat untouched for
 * the time Settings → TV → When idle gives ([TvIdleOff], never by default), fully black or only dimmed
 * ([TvIdleStyle], black by default).
 *
 * While the screen is fully off this also goes as far as an app can towards a really dark panel:
 * - the mouse / air-mouse pointer is hidden over the black layer (a pointer drawn by a separate "mouse toggle" app is
 *   that app's own overlay and can't be hidden from here);
 * - the window's brightness is set to off ([WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_OFF]), which turns the
 *   backlight down or off on TVs and boxes that honour it (many TVs ignore it and only the black picture remains);
 * - the window never holds the screen on ([WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON] is cleared), so the TV's own
 *   "turn off display when inactive" setting can switch the panel off. Playback runs in its foreground service with a
 *   CPU-only wake lock, so the music keeps playing either way.
 * Actually switching the panel off on command takes system rights (HDMI-CEC standby, the power manager) that a store
 * app doesn't get, so that is left to the TV's own power button and settings.
 */
package app.encore.android.ui.tv

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.view.KeyEvent
import android.view.WindowManager
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
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
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.nativeKeyCode
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.PointerIcon
import androidx.compose.ui.input.pointer.pointerHoverIcon
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.ui.Nav
import kotlinx.coroutines.delay

/**
 * How long the TV player waits, with no remote button pressed while the music plays, before the screen goes black (or
 * dim, [TvIdleStyle]) by itself (Settings → TV → When idle). Never unless that setting is changed: by default only the player's Screen off button
 * blacks the screen out.
 *
 * @property key the stored form.
 * @property label what Settings → TV shows for it.
 * @property ms the wait in milliseconds; null for [NEVER] (only the player's Screen off button does it).
 */
enum class TvIdleOff(val key: String, val label: String, val ms: Long?) {
    /** Never by itself: the default. */
    NEVER("never", "Never", null),

    /** After a minute. */
    MIN_1("1m", "1 min", 60_000L),

    /** After two minutes. */
    MIN_2("2m", "2 min", 120_000L),

    /** After five minutes. */
    MIN_5("5m", "5 min", 300_000L),

    /** After ten minutes. */
    MIN_10("10m", "10 min", 600_000L),
    ;

    companion object {
        /** The wait stored as [key]; anything unknown or missing is [NEVER]. */
        fun of(key: String?): TvIdleOff = entries.firstOrNull { it.key == key } ?: NEVER
    }
}

/**
 * What the screen does when the TV player has sat idle ([TvIdleOff]): go fully black (the default) or only dim, so the
 * cover and the song still show faintly. The Screen off button in the player always goes fully black.
 *
 * @property key the stored form.
 * @property label what Settings → TV shows for it.
 * @property alpha how dark the layer over the player is: 1 is all black.
 */
enum class TvIdleStyle(val key: String, val label: String, val alpha: Float) {
    /** All black: on an OLED TV the pixels are off. The default. */
    OFF("off", "Screen off", 1f),

    /** Darkened, the player still faintly showing. */
    DIM("dim", "Dim", 0.80f),
    ;

    companion object {
        /** The style stored as [key]; anything unknown or missing is [OFF]. */
        fun of(key: String?): TvIdleStyle = entries.firstOrNull { it.key == key } ?: OFF
    }
}

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

    /**
     * How long from now until Screen off should turn on by itself, or null when it shouldn't (the setting is off, the
     * screen is already off, or nothing is playing: a paused player is left lit, as it is likely being looked at).
     *
     * @param idle the setting (Settings → TV → When idle).
     * @param screenOff whether the screen is already off.
     * @param playing whether the music is playing.
     * @param sinceInputMs how long ago the remote was last pressed, in ms.
     */
    fun idleDelay(idle: TvIdleOff, screenOff: Boolean, playing: Boolean, sinceInputMs: Long): Long? {
        val wait = idle.ms ?: return null
        if (screenOff || !playing) return null
        return (wait - sinceInputMs).coerceAtLeast(0L)
    }
}

/**
 * The black (or dimmed, [alpha] under 1) screen of Screen off, over everything while [on]. It takes the remote's focus and keeps every button
 * from reaching the player underneath: playback buttons act on the music ([ScreenOffRules.actionOf]), volume and power
 * go to the TV, and any other button calls [onWake] when it is let go (so the same press doesn't also reach the
 * button that gets the focus back). For the first few seconds a dim line says how to get back; then it is all black.
 *
 * @param on whether the screen is off.
 * @param alpha how dark the layer is: 1 (the default) is all black, less lets the player show faintly ([TvIdleStyle]).
 * @param nav plays, pauses and skips for the playback buttons.
 * @param onWake turns the screen back on.
 */
@Composable
fun ScreenOffLayer(on: Boolean, nav: Nav, onWake: () -> Unit, alpha: Float = 1f) {
    if (!on) return
    val context = LocalContext.current
    // an empty pointer, so a mouse or air-mouse cursor doesn't float over the black screen
    val noPointer = remember(context) {
        PointerIcon(android.view.PointerIcon.getSystemIcon(context, android.view.PointerIcon.TYPE_NULL))
    }
    // fully off: the window's backlight down too, put back as it was when the screen comes back on
    if (alpha >= 1f) {
        DisposableEffect(context) {
            val window = context.findActivity()?.window
            val before = window?.attributes?.screenBrightness
            window?.let { w ->
                w.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                w.attributes = w.attributes.apply { screenBrightness = WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_OFF }
            }
            onDispose {
                if (window != null && before != null) {
                    window.attributes = window.attributes.apply { screenBrightness = before }
                }
            }
        }
    }
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
            .background(Color.Black.copy(alpha = alpha))
            .pointerHoverIcon(noPointer, overrideDescendants = true)
            // a mouse click wakes the screen too, rather than pressing an unseen button of the player underneath
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = onWake)
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
                if (alpha < 1f) "Dimmed · press any button to light it back up" else "Screen off · press any button to turn it back on",
                color = Color(0xFF555555),
                fontSize = 14.sp,
                modifier = Modifier.padding(bottom = 32.dp),
            )
        }
    }
}

/** The activity this context belongs to (a Compose context may be wrapped), or null when there is none. */
private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}
