package app.encore.android.ui.components

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.graphics.PixelFormat
import android.os.Build
import android.view.Gravity
import android.view.View
import android.view.ViewTreeObserver
import android.view.WindowManager
import android.view.inspector.WindowInspector
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.slideInVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionContext
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCompositionContext
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.findViewTreeLifecycleOwner
import androidx.lifecycle.setViewTreeLifecycleOwner
import androidx.savedstate.findViewTreeSavedStateRegistryOwner
import androidx.savedstate.setViewTreeSavedStateRegistryOwner
import app.encore.android.R
import app.encore.android.features.settings.copyToClipboard
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import java.util.concurrent.atomic.AtomicLong

/**
 * Toasts like the web's `toast(msg, type)` (web-rs/src/ui.rs): panels with a coloured left border at the bottom of
 * the screen. Shared seam: any part calls [show] from any thread; [ToastHost] draws them, in a small window of their
 * own above every sheet, dialog and the full player (see there).
 *
 * `kind` is the web's type: "info" (the default, accent border), "ok" (green), "err" (red), "warn" (amber). A toast
 * goes after about four seconds; an error stays until its Close and has **Copy** (the whole message, for a bug
 * report), and new toasts stack below it ([ToastRules]).
 */
object Toasts {
    /** The words when the clipboard refuses (the toast says what to do instead, so it has no Copy). */
    const val COPY_FAILED = ToastRules.COPY_FAILED

    private val counter = AtomicLong()
    private val items = MutableStateFlow<List<ToastItem>>(emptyList())

    /** The toasts on screen, oldest first; drawn by [ToastHost]. */
    val stack: StateFlow<List<ToastItem>> = items.asStateFlow()

    fun show(message: String, kind: String = "info") {
        if (message.isBlank()) return
        val n = counter.incrementAndGet()
        items.update { ToastRules.add(it, n, n, message, kind) }
    }

    /** An error toast's Close. */
    fun close(id: Long) = items.update { ToastRules.close(it, id) }

    internal fun expire(id: Long, seq: Long) = items.update { ToastRules.expire(it, id, seq) }
}

/**
 * Draws [Toasts] at the bottom of the screen; put it once in the root, [bottomPadding] above the bottom of the screen
 * (above the mini player and the tabs).
 *
 * A ModalBottomSheet, a Dialog and an AlertDialog are windows of their own above the activity's window, so toasts
 * drawn in the activity's content were hidden behind them. The toasts get a small window instead, added to the
 * activity's windows while there are toasts and the activity is started (removed when it stops): not focusable
 * (the keyboard and Back stay with the sheet or dialog), not touch-modal (a touch beside the toasts goes to the window
 * below), no dim. An activity's windows are stacked in the order they were added, so the toasts' window is added
 * again on top when a sheet or dialog opened after it covers it: when a toast arrives, when the activity's window
 * loses focus to a sheet or dialog, and (Android 10 and later, which lists the app's windows) every 400 ms while
 * toasts show. Menus and popups are sub-windows of their own window, below the toasts.
 */
@Composable
fun ToastHost(bottomPadding: Dp) {
    val stack by Toasts.stack.collectAsState()
    // a toast that is not an error goes after a few seconds; a repeat starts its time again (a new seq)
    for (t in stack) {
        if (!t.stays) {
            key(t.id, t.seq) {
                LaunchedEffect(Unit) {
                    delay(ToastRules.SHOW_MS)
                    Toasts.expire(t.id, t.seq)
                }
            }
        }
    }

    val view = LocalView.current
    val parent = rememberCompositionContext()
    val window = remember(view, parent) { view.context.findActivity()?.let { ToastWindow(it, view, parent) } } ?: return
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    val density = LocalDensity.current
    // above the keyboard while it is open (the toasts' window is above the keyboard's)
    val liftPx = with(density) {
        val ime = WindowInsets.ime.getBottom(density).toDp().value
        val nav = WindowInsets.navigationBars.getBottom(density).toDp().value
        ToastRules.lift(bottomPadding.value, ime, nav).dp.roundToPx()
    }
    val any = stack.isNotEmpty()
    val newest = ToastRules.newest(stack)

    LaunchedEffect(window, liftPx) { window.setLift(liftPx) }
    LaunchedEffect(window, any, newest) {
        when {
            !any || !lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED) -> window.remove()
            // added now, so above every sheet and dialog open now
            !window.shown -> window.add()
            // a new toast: above a sheet or dialog that opened since
            newest > window.raisedFor -> window.raiseIfCovered()
        }
        window.raisedFor = maxOf(window.raisedFor, newest)
    }
    if (any && Build.VERSION.SDK_INT >= 29) {
        LaunchedEffect(window) {
            while (true) {
                delay(400)
                window.raiseIfCovered()
            }
        }
    }
    // the window goes when the activity stops and comes back when it starts. Done here, not by recomposing: the
    // root's composition does not recompose while the activity is stopped.
    DisposableEffect(window, lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_STOP -> window.remove()
                Lifecycle.Event.ON_START -> if (Toasts.stack.value.isNotEmpty()) window.add()
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    DisposableEffect(window) {
        val observer = view.viewTreeObserver
        // the activity's window loses focus when a sheet or dialog opens above it
        val focus = ViewTreeObserver.OnWindowFocusChangeListener { hasFocus -> if (!hasFocus) window.raiseIfCovered() }
        observer.addOnWindowFocusChangeListener(focus)
        onDispose {
            if (observer.isAlive) observer.removeOnWindowFocusChangeListener(focus) else view.viewTreeObserver.removeOnWindowFocusChangeListener(focus)
            window.remove()
        }
    }
}

/** The toasts' window: added to [activity]'s windows, composed as part of [host]'s composition ([parent]). */
private class ToastWindow(private val activity: Activity, private val host: View, private val parent: CompositionContext) {
    private var view: ComposeView? = null

    // one LayoutParams for every add and update: the activity's own WindowManager fills in its token on the first
    // add (so the window is stacked with the activity's sheets and dialogs), and an update keeps it
    private val params = WindowManager.LayoutParams(
        WindowManager.LayoutParams.MATCH_PARENT,
        WindowManager.LayoutParams.WRAP_CONTENT,
        WindowManager.LayoutParams.TYPE_APPLICATION,
        WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL,
        PixelFormat.TRANSLUCENT,
    ).apply {
        gravity = Gravity.BOTTOM
        // a name for window dumps (layout inspector, dumpsys window); not shown
        title = "encore.toasts"
    }

    /** The newest toast the window was put on top for. */
    var raisedFor = 0L

    /** Toasts drawn once already (a toast slides in only the first time, not each time the window is added again). */
    val seen = HashSet<Long>()

    val shown: Boolean get() = view != null

    fun add() {
        if (view != null || activity.isFinishing || activity.isDestroyed) return
        val v = ComposeView(activity)
        // the activity's owners, as Compose's own Popup and Dialog windows take them from the view they open from
        v.setViewTreeLifecycleOwner(host.findViewTreeLifecycleOwner())
        v.setViewTreeSavedStateRegistryOwner(host.findViewTreeSavedStateRegistryOwner())
        // the theme and the rest of the root's composition locals
        v.setParentCompositionContext(parent)
        v.setContent { ToastStack(seen) }
        try {
            activity.windowManager.addView(v, params)
            view = v
        } catch (_: WindowManager.BadTokenException) {
            // the activity's window is going away: nothing to show the toasts over
        } catch (_: IllegalStateException) {
        }
    }

    fun remove() {
        val v = view ?: return
        view = null
        try {
            activity.windowManager.removeViewImmediate(v)
        } catch (_: IllegalArgumentException) {
            // not attached any more
        }
    }

    /**
     * Adds the window again on top when a sheet or dialog covers it. Before Android 10 the app cannot list its
     * windows, so the window is added again each time.
     */
    fun raiseIfCovered() {
        val v = view ?: return
        if (Build.VERSION.SDK_INT >= 29 && !covered(v)) return
        remove()
        add()
    }

    /** How high above the bottom the toasts sit; the space below them (mini player, tabs) is outside the window. */
    fun setLift(px: Int) {
        if (px == params.y) return
        params.y = px
        val v = view ?: return
        try {
            activity.windowManager.updateViewLayout(v, params)
        } catch (_: IllegalArgumentException) {
        }
    }

    private fun covered(v: View): Boolean {
        if (Build.VERSION.SDK_INT < 29) return false
        val windows = try {
            WindowInspector.getGlobalWindowViews()
        } catch (_: RuntimeException) {
            return false
        }
        val token = params.token ?: return false
        return ToastRules.covered(
            windows.map { w ->
                val lp = w.layoutParams as? WindowManager.LayoutParams
                // a window of this activity (its token) that is showing (a hidden dialog is not)
                AppWindow(type = lp?.type ?: 0, shown = w.isShown && lp?.token == token, toasts = w === v)
            },
        )
    }
}

private fun Context.findActivity(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is Activity) return c
        c = c.baseContext
    }
    return null
}

/** The toasts, oldest first and the newest nearest the bottom; scrolls when they are taller than most of the screen. */
@Composable
private fun ToastStack(seen: MutableSet<Long>) {
    val stack by Toasts.stack.collectAsState()
    val maxHeight = (LocalConfiguration.current.screenHeightDp * 0.6f).dp
    Column(
        Modifier
            .fillMaxWidth()
            .heightIn(max = maxHeight)
            .verticalScroll(rememberScrollState(), reverseScrolling = true)
            // room for the shadows inside the window
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (t in stack) {
            key(t.id) {
                val appear = remember { MutableTransitionState(t.id in seen).apply { targetState = true } }
                SideEffect { seen += t.id }
                AnimatedVisibility(visibleState = appear, enter = fadeIn() + slideInVertically { it / 3 }) {
                    ToastCard(t)
                }
            }
        }
    }
}

@Composable
private fun ToastCard(t: ToastItem) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val edge: Color = when (t.kind) {
        "ok" -> EncoreColors.Ok
        "err" -> EncoreColors.Err
        "warn" -> EncoreColors.Warn
        else -> p.accent
    }
    Surface(
        color = p.panel,
        contentColor = p.text,
        shape = RoundedCornerShape(12.dp),
        shadowElevation = 6.dp,
        modifier = Modifier.fillMaxWidth().semantics { liveRegion = if (t.stays) LiveRegionMode.Assertive else LiveRegionMode.Polite },
    ) {
        Row(
            Modifier.padding(start = 12.dp, end = if (t.stays) 4.dp else 12.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(4.dp).height(22.dp).clip(RoundedCornerShape(2.dp)).background(edge))
            // the whole message; a long one scrolls
            Box(Modifier.weight(1f).padding(start = 12.dp, top = 8.dp, bottom = 8.dp).heightIn(max = 160.dp).verticalScroll(rememberScrollState())) {
                Text(t.message, style = MaterialTheme.typography.bodyMedium)
            }
            if (t.stays) {
                // Copy keeps the error on screen; the toast "Copied to clipboard" says it worked
                TextButton(onClick = { copyToClipboard(context, t.message) }) {
                    Text("Copy", color = p.text, fontWeight = FontWeight.SemiBold)
                }
                IconButton(onClick = { Toasts.close(t.id) }, modifier = Modifier.size(32.dp)) {
                    Icon(painterResource(R.drawable.ic_close), contentDescription = "Close", tint = p.muted, modifier = Modifier.size(16.dp))
                }
            }
        }
    }
}
