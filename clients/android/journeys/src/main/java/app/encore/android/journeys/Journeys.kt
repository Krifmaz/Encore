/**
 * The user journeys of `clients/android/PERFORMANCE.md`, as UiAutomator steps on a [MacrobenchmarkScope]:
 * sign in once, start the app, scroll the Library's albums, open an album and scroll its songs, let the full player
 * play, and open and change Up next. `:benchmark` measures them (frame timing, startup time) and `:baselineprofile`
 * records them into the Baseline Profile, so both always exercise the same code.
 *
 * The app tags the nodes these steps look for with Compose test tags ([Tags]); MainActivity turns on
 * `testTagsAsResourceId`, so each tag is a resource id that `By.res(tag)` finds. Everything else is found by the text
 * or content description a person sees ("Library", "Albums", "Play", "Up next", "Remove").
 */
package app.encore.android.journeys

import android.os.Build
import android.util.Log
import androidx.benchmark.macro.BaselineProfileMode
import androidx.benchmark.macro.CompilationMode
import androidx.benchmark.macro.MacrobenchmarkScope
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.BySelector
import androidx.test.uiautomator.Direction
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.UiObject2
import androidx.test.uiautomator.Until
import java.io.File

/** The release application id (the debug build adds `.debug`); every journey targets this package. */
const val ENCORE_PACKAGE = "app.encore.client"

/**
 * The Compose test tags the app puts on the nodes the journeys need. They must match the strings in the app
 * (`app/src/main/java/app/encore/android/ui/PerfTags.kt`): the two modules can't share code.
 */
object Tags {
    /** The sign-in flow's server address field. */
    const val SERVER_FIELD = "server_field"
    /** The sign-in screen's username field. */
    const val USERNAME_FIELD = "username_field"
    /** The sign-in screen's password field. */
    const val PASSWORD_FIELD = "password_field"
    /** The Library's paged albums grid. */
    const val LIBRARY_ALBUMS = "library_albums"
    /** An album page's list (header, then its songs). */
    const val ALBUM_TRACKS = "album_tracks"
    /** The mini player above the bottom tabs (a tap opens the full player). */
    const val MINI_PLAYER = "mini_player"
    /** The full-screen player. */
    const val FULL_PLAYER = "full_player"
    /** The Up next list (in its bottom sheet). */
    const val UP_NEXT = "up_next"
}

/**
 * The instrumentation arguments the journeys read (`-Pandroid.testInstrumentationRunnerArguments.<name>=<value>`,
 * see PERFORMANCE.md): the server to sign in to when the app is signed out, and how many iterations to run.
 */
object JourneyArgs {
    private val args get() = InstrumentationRegistry.getArguments()

    /**
     * The package to drive: the release app ([ENCORE_PACKAGE]) unless `encore.package` names another, such as the debug
     * build (`app.encore.client.debug`) for the StrictMode and JankStats pass of `benchmark/ci/run.sh`.
     */
    val packageName: String get() = args.getString("encore.package")?.takeIf { it.isNotBlank() } ?: ENCORE_PACKAGE

    /** The Encore server's address, as typed on the sign-in screen (e.g. `http://10.0.2.2:8080` from an emulator). */
    val server: String? get() = args.getString("encore.server")?.takeIf { it.isNotBlank() }

    /** The account to sign in with. */
    val user: String? get() = args.getString("encore.user")?.takeIf { it.isNotBlank() }

    /** The account's password. */
    val password: String? get() = args.getString("encore.password")?.takeIf { it.isNotBlank() }

    /** Iterations for each frame-timing journey (default 5). */
    val iterations: Int get() = args.getString("encore.iterations")?.toIntOrNull()?.coerceAtLeast(1) ?: 5

    /** Iterations for each startup measurement (default 10). */
    val startupIterations: Int get() = args.getString("encore.startupIterations")?.toIntOrNull()?.coerceAtLeast(1) ?: 10

    /**
     * How the app is compiled before it is measured: `partial` (default) installs the app's Baseline Profile the way
     * profileinstaller does on a fresh install, which is what a person who just installed or updated the APK gets;
     * `none` measures it with no ahead-of-time compilation at all (the app before it shipped a profile).
     */
    val compilationMode: CompilationMode
        get() = when (args.getString("encore.compilation")) {
            "none" -> CompilationMode.None()
            else -> CompilationMode.Partial(baselineProfileMode = BaselineProfileMode.UseIfAvailable)
        }

    /** Where Macrobenchmark collects extra outputs (traces, and our failure dumps); null outside an instrumentation run. */
    val outputDir: File? get() = args.getString("additionalTestOutputDir")?.let(::File)
}

/** How long to wait for a screen or a list to show (a cold start on an emulator with a server on the host). */
private const val SCREEN_TIMEOUT_MS = 20_000L

/** Flings at most this many times to reach the end of a list (a large library would otherwise scroll for minutes). */
private const val MAX_FLINGS = 30

/** Log tag of the journeys' own lines (logcat). */
private const val TAG = "EncoreJourneys"

/** True once this test process saw the app signed in, so later iterations don't look for the sign-in screen again. */
@Volatile
private var signedIn = false

/** Waits for [selector] to show, or fails the journey with [what] (dumping the screen first, see [fail]). */
private fun UiDevice.need(selector: BySelector, what: String, timeoutMs: Long = SCREEN_TIMEOUT_MS): UiObject2 =
    wait(Until.findObject(selector), timeoutMs) ?: fail(this, "not on screen after ${timeoutMs / 1000} s: $what")

/**
 * Fails the journey with [message], first saving the window hierarchy and a screenshot to the test's output folder
 * (they come back with the results as `journey-failure-<time>.xml` and `.png`; benchmark/ci/failures.py prints what
 * was on screen).
 */
private fun fail(device: UiDevice, message: String): Nothing {
    JourneyArgs.outputDir?.let { dir ->
        try {
            dir.mkdirs()
            // one pair per failure (several tests run into the same folder)
            val name = "journey-failure-${System.currentTimeMillis()}"
            File(dir, "$name.xml").outputStream().use { device.dumpWindowHierarchy(it) }
            device.takeScreenshot(File(dir, "$name.png"))
        } catch (e: Exception) {
            Log.w(TAG, "could not save the failure dump", e)
        }
    }
    throw AssertionError(message)
}

/**
 * Flings [list] towards its end until it stops moving (or [MAX_FLINGS] flings), waiting briefly between flings so
 * pages and covers load as a person scrolling would see them.
 */
private fun UiDevice.flingToEnd(list: UiObject2) {
    // keep the fling off the system gesture areas at the screen's edges
    list.setGestureMargin(displayWidth / 5)
    var flings = 0
    while (flings < MAX_FLINGS && list.fling(Direction.DOWN)) {
        flings++
        waitForIdle(300)
    }
}

/**
 * Gets the app ready for a journey: allows its notifications (Android 13+ would otherwise ask on top of the first
 * screen), starts it in a new task (the previous iteration's pages and open player are gone, it opens on Home) and,
 * when it is signed out, signs in with [JourneyArgs]. Ends with the app open on Home with its bottom tabs.
 */
fun MacrobenchmarkScope.prepare() {
    if (Build.VERSION.SDK_INT >= 33) {
        device.executeShellCommand("pm grant $packageName android.permission.POST_NOTIFICATIONS")
    }
    startActivityAndWait()
    if (!signedIn) {
        val server = device.wait(Until.findObject(By.res(Tags.SERVER_FIELD)), 5_000)
        if (server != null) signIn(server)
    }
    device.need(By.text("Library"), "the bottom tabs")
    signedIn = true
}

/**
 * For a cold start: signs in the first time (the app is then stopped again, so the measured start is cold) and
 * otherwise only allows the notifications. Never leaves the app running.
 */
fun MacrobenchmarkScope.prepareCold() {
    if (signedIn) {
        if (Build.VERSION.SDK_INT >= 33) {
            device.executeShellCommand("pm grant $packageName android.permission.POST_NOTIFICATIONS")
        }
        return
    }
    prepare()
    killProcess()
}

/** Fills the sign-in flow (server address, then username and password) from [JourneyArgs]. */
private fun MacrobenchmarkScope.signIn(serverField: UiObject2) {
    val server = JourneyArgs.server ?: fail(device, "the app is signed out and no encore.server argument was given")
    val user = JourneyArgs.user ?: fail(device, "the app is signed out and no encore.user argument was given")
    val password = JourneyArgs.password ?: fail(device, "the app is signed out and no encore.password argument was given")
    serverField.text = server
    device.need(By.text("Continue").enabled(true), "Continue").click()
    device.need(By.res(Tags.USERNAME_FIELD), "the username field").text = user
    device.need(By.res(Tags.PASSWORD_FIELD), "the password field").text = password
    device.need(By.text("Sign in").enabled(true), "Sign in").click()
}

/** Opens the Library tab on its Albums list and returns the albums grid once its first cards are on screen. */
fun MacrobenchmarkScope.openLibraryAlbums(): UiObject2 {
    device.need(By.text("Library"), "the Library tab").click()
    // the Library remembers its last list (Albums, Artists or Songs): pick Albums
    device.wait(Until.findObject(By.text("Albums")), 5_000)?.click()
    val grid = device.need(By.res(Tags.LIBRARY_ALBUMS), "the albums grid")
    grid.wait(Until.hasObject(By.clickable(true)), SCREEN_TIMEOUT_MS)
    return grid
}

/** Journey: scrolls the Library's albums grid from the top to the bottom (at most [MAX_FLINGS] flings). */
fun MacrobenchmarkScope.scrollLibraryAlbums(grid: UiObject2) {
    device.flingToEnd(grid)
}

/** Journey: opens the first album in [grid] and scrolls its page (cover, details, songs) to the end. */
fun MacrobenchmarkScope.openAlbumAndScroll(grid: UiObject2) {
    openFirstAlbum(grid)
    val list = device.need(By.res(Tags.ALBUM_TRACKS), "the album page")
    device.need(By.text("Play"), "the album's Play button")
    device.flingToEnd(list)
}

/** Opens the first album card of [grid]. */
private fun MacrobenchmarkScope.openFirstAlbum(grid: UiObject2) {
    val card = grid.findObject(By.clickable(true)) ?: fail(device, "no album card in the albums grid")
    card.click()
}

/**
 * Starts the first album of the Library from its first song (a fresh queue of the album's songs every time, so the
 * Up next journey always has songs to remove), and waits for the mini player.
 */
fun MacrobenchmarkScope.playFirstAlbum() {
    val grid = openLibraryAlbums()
    openFirstAlbum(grid)
    device.need(By.text("Play"), "the album's Play button").click()
    device.need(By.res(Tags.MINI_PLAYER), "the mini player")
    // the stream starting (the mini player's button turns to Pause)
    device.wait(Until.hasObject(By.res(Tags.MINI_PLAYER).hasDescendant(By.desc("Pause"))), SCREEN_TIMEOUT_MS)
}

/** Opens the full player from the mini player and waits for it. */
fun MacrobenchmarkScope.openFullPlayer() {
    device.need(By.res(Tags.MINI_PLAYER), "the mini player").click()
    device.need(By.res(Tags.FULL_PLAYER), "the full player")
}

/** Journey: opens the full player and lets the song play for [seconds] seconds (the seek bar and lyrics move). */
fun MacrobenchmarkScope.watchFullPlayer(seconds: Int = 10) {
    openFullPlayer()
    Thread.sleep(seconds * 1000L)
}

/**
 * Journey: opens Up next from the full player, scrolls it to the end and back, and removes the first queued song
 * (the phone's Up next has no drag to reorder; removing a song moves every song after it up, which is the change of
 * order the list animates).
 */
fun MacrobenchmarkScope.editUpNext() {
    device.need(By.desc("Up next"), "the Up next button").click()
    val list = device.need(By.res(Tags.UP_NEXT), "the Up next list")
    list.setGestureMargin(device.displayWidth / 5)
    list.fling(Direction.DOWN)
    device.waitForIdle(300)
    list.fling(Direction.UP)
    device.waitForIdle(300)
    device.need(By.desc("Remove"), "a queued song's Remove button").click()
    // the rows below move up (animateItem)
    Thread.sleep(1_000)
}
