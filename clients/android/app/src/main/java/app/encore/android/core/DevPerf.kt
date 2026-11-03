package app.encore.android.core

import android.app.Activity
import android.os.StrictMode
import android.util.Log
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.platform.LocalView
import androidx.metrics.performance.JankStats
import androidx.metrics.performance.PerformanceMetricsState
import app.encore.android.BuildConfig

/**
 * Debug-build performance checks (PERFORMANCE.md): StrictMode for disk and network work on the main thread, and
 * JankStats logging every janky frame with the screen it happened on. Every entry point returns at once in other
 * builds (`BuildConfig.DEBUG` is a constant, so R8 drops the calls and the library from a release APK).
 *
 * Read the logs with `adb logcat -s StrictMode EncoreJank`.
 */
object DevPerf {
    /** Logcat tag of the janky-frame lines. */
    private const val TAG = "EncoreJank"

    /** The JankStats state key the current screen's name is kept under. */
    private const val SCREEN = "screen"

    /**
     * Turns on StrictMode's thread policy for the main thread: disk reads, disk writes and network calls are logged
     * (tag `StrictMode`, with the stack that made them). Call first thing in `Application.onCreate`.
     */
    fun strictMode() {
        if (!BuildConfig.DEBUG) return
        StrictMode.setThreadPolicy(
            StrictMode.ThreadPolicy.Builder()
                .detectDiskReads()
                .detectDiskWrites()
                .detectNetwork()
                .penaltyLog()
                .build(),
        )
    }

    /**
     * Starts JankStats on [activity]'s window: each frame the system flags as janky is logged with its duration and
     * the states put by [ScreenName]. Returns the tracker, so the activity can pause it while stopped (null outside
     * debug builds).
     */
    fun trackJank(activity: Activity): JankStats? {
        if (!BuildConfig.DEBUG) return null
        return JankStats.createAndTrack(activity.window) { frame ->
            if (frame.isJank) {
                val ms = frame.frameDurationUiNanos / 1_000_000.0
                val states = frame.states.joinToString { "${it.key}=${it.value}" }
                Log.w(TAG, "janky frame %.1f ms [%s]".format(ms, states))
            }
        }
    }

    /**
     * Names the screen on top for JankStats while this is in the composition (debug builds only): janky frames are
     * logged with `screen=<name>`. [name] is only called in debug builds, so other builds don't work out the name.
     */
    @Composable
    fun ScreenName(name: () -> String) {
        if (!BuildConfig.DEBUG) return
        val shown = name()
        val view = LocalView.current
        DisposableEffect(view, shown) {
            val holder = PerformanceMetricsState.getHolderForHierarchy(view)
            holder.state?.putState(SCREEN, shown)
            onDispose { holder.state?.removeState(SCREEN) }
        }
    }
}
