/** Records Encore's Baseline Profile and startup profile from the journeys (PERFORMANCE.md). */
package app.encore.android.baselineprofile

import androidx.benchmark.macro.junit4.BaselineProfileRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import app.encore.android.journeys.ENCORE_PACKAGE
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
 * Runs the journeys with ART method tracing on and writes what they used as profile rules. `gradle
 * :app:generateBaselineProfile` runs it and copies the result into app/src/main/generated/baselineProfiles.
 */
@RunWith(AndroidJUnit4::class)
class BaselineProfileGenerator {
    /** Collects the profile over a few runs of each journey, until the rules stop changing. */
    @get:Rule
    val rule = BaselineProfileRule()

    /**
     * Startup, signed in, to Home: these rules also go into the startup profile, which lays out the DEX files so
     * the classes a launch needs are read together (dexLayoutOptimization in app/build.gradle.kts).
     */
    @Test
    fun startup() = rule.collect(
        packageName = ENCORE_PACKAGE,
        includeInStartupProfile = true,
        maxIterations = 5,
        stableIterations = 2,
    ) {
        prepare()
    }

    /** Library albums, an album page, the full player playing and Up next: the rest of the app's hot paths. */
    @Test
    fun journeys() = rule.collect(
        packageName = ENCORE_PACKAGE,
        maxIterations = 4,
        stableIterations = 2,
    ) {
        prepare()
        scrollLibraryAlbums(openLibraryAlbums())
        prepare()
        openAlbumAndScroll(openLibraryAlbums())
        prepare()
        playFirstAlbum()
        watchFullPlayer(seconds = 3)
        prepare()
        playFirstAlbum()
        openFullPlayer()
        editUpNext()
    }
}
