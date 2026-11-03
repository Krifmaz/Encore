/** Startup time of the release app, cold and warm (PERFORMANCE.md, "Startup"). */
package app.encore.android.benchmark

import androidx.benchmark.macro.StartupMode
import androidx.benchmark.macro.StartupTimingMetric
import androidx.benchmark.macro.junit4.MacrobenchmarkRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import app.encore.android.journeys.ENCORE_PACKAGE
import app.encore.android.journeys.JourneyArgs
import app.encore.android.journeys.prepare
import app.encore.android.journeys.prepareCold
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Time to initial display (and to full display, where reported) of MainActivity, signed in, opening on Home.
 */
@RunWith(AndroidJUnit4::class)
class StartupBenchmark {
    /** Runs each measurement and writes its results (JSON and traces) to the test's output folder. */
    @get:Rule
    val rule = MacrobenchmarkRule()

    /** The process is not running: Application, the graph, the first frame of Home. */
    @Test
    fun coldStartup() = rule.measureRepeated(
        packageName = ENCORE_PACKAGE,
        metrics = listOf(StartupTimingMetric()),
        compilationMode = JourneyArgs.compilationMode,
        iterations = JourneyArgs.startupIterations,
        startupMode = StartupMode.COLD,
        // signs in once (then stops the app again): the measured start must find the process gone
        setupBlock = { prepareCold() },
    ) {
        startActivityAndWait()
    }

    /** The process is alive, the activity is made again (after Back or the system closing it). */
    @Test
    fun warmStartup() = rule.measureRepeated(
        packageName = ENCORE_PACKAGE,
        metrics = listOf(StartupTimingMetric()),
        compilationMode = JourneyArgs.compilationMode,
        iterations = JourneyArgs.startupIterations,
        startupMode = StartupMode.WARM,
        setupBlock = {
            prepare()
            pressHome()
        },
    ) {
        startActivityAndWait()
    }
}
