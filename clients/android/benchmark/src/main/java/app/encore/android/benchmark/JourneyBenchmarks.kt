/** Frame timing of the app's journeys (PERFORMANCE.md, "Frame timing"). */
package app.encore.android.benchmark

import androidx.benchmark.macro.FrameTimingMetric
import androidx.benchmark.macro.MacrobenchmarkScope
import androidx.benchmark.macro.junit4.MacrobenchmarkRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.uiautomator.UiObject2
import app.encore.android.journeys.ENCORE_PACKAGE
import app.encore.android.journeys.JourneyArgs
import app.encore.android.journeys.editUpNext
import app.encore.android.journeys.openAlbumAndScroll
import app.encore.android.journeys.openFullPlayer
import app.encore.android.journeys.openLibraryAlbums
import app.encore.android.journeys.playFirstAlbum
import app.encore.android.journeys.prepare
import app.encore.android.journeys.scrollLibraryAlbums
import app.encore.android.journeys.watchFullPlayer
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Frame duration (P50 / P90 / P95 / P99) and frame overrun (a frame over 0 ms of overrun missed its deadline: a
 * janky frame) of each journey. Every iteration starts the app in a new task on Home ([prepare]) and gets to the
 * journey's first screen outside the measured part.
 */
@RunWith(AndroidJUnit4::class)
class JourneyBenchmarks {
    /** Runs each measurement and writes its results (JSON and traces) to the test's output folder. */
    @get:Rule
    val rule = MacrobenchmarkRule()

    /** Library → Albums, flung from the top to the bottom. */
    @Test
    fun scrollLibraryAlbums() {
        var grid: UiObject2? = null
        frames(setup = { prepare(); grid = openLibraryAlbums() }) { scrollLibraryAlbums(grid!!) }
    }

    /** From the albums grid: open the first album and fling its page to the end. */
    @Test
    fun openAlbumAndScrollTracks() {
        var grid: UiObject2? = null
        frames(setup = { prepare(); grid = openLibraryAlbums() }) { openAlbumAndScroll(grid!!) }
    }

    /** With a song playing: open the full player and let it play for 10 seconds. */
    @Test
    fun fullPlayerPlaying() {
        frames(setup = { prepare(); playFirstAlbum() }) { watchFullPlayer(seconds = 10) }
    }

    /** With an album queued: open Up next from the full player, scroll it, remove a song. */
    @Test
    fun upNextQueue() {
        frames(setup = { prepare(); playFirstAlbum(); openFullPlayer() }) { editUpNext() }
    }

    /** Measures [journey] with [FrameTimingMetric], after [setup] (not measured), [JourneyArgs.iterations] times. */
    private fun frames(setup: MacrobenchmarkScope.() -> Unit, journey: MacrobenchmarkScope.() -> Unit) =
        rule.measureRepeated(
            packageName = ENCORE_PACKAGE,
            metrics = listOf(FrameTimingMetric()),
            compilationMode = JourneyArgs.compilationMode,
            iterations = JourneyArgs.iterations,
            // the journeys start the app themselves (prepare): a fresh task, the process kept (music keeps playing)
            startupMode = null,
            setupBlock = setup,
            measureBlock = journey,
        )
}
