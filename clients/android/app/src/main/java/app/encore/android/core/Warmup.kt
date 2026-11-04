package app.encore.android.core

import android.content.Context
import android.util.Log
import app.encore.android.cadenza.CadenzaEngine
import java.io.File

/**
 * Loads, on a background thread at app start, what the screens and the playback service would otherwise read from
 * disk on the main thread the first time they need it (PERFORMANCE.md, "Debug-build checks"): every
 * SharedPreferences file of the app and the Cadenza native library. Once a file is loaded, the main thread's first
 * `getString` finds it in memory instead of waiting for the disk, and opening Library, Up next or a TV panel no
 * longer stalls a frame on it.
 *
 * Nothing here changes what the app reads or when it writes: the same instances are loaded, only earlier and off
 * the main thread. A failure only means the main thread loads that file itself, as it did before.
 */
object Warmup {
    /** Logcat tag of the (rare) failures. */
    private const val TAG = "EncoreWarmup"

    /** Starts the background load and returns at once. Call first thing in `Application.onCreate`. */
    fun start(context: Context) {
        val app = context.applicationContext
        Thread({ run(app) }, "encore-warmup").apply {
            // never keeps the process alive, and yields to the main thread
            isDaemon = true
            priority = Thread.NORM_PRIORITY - 1
        }.start()
    }

    /** The background thread's work: each preferences file by the name its XML file has, then the native library. */
    private fun run(app: Context) {
        try {
            // shared_prefs/<name>.xml is where getSharedPreferences(<name>) keeps a file: listing the folder covers
            // every file the app has written, with no list of names here to keep up to date
            val names = File(app.dataDir, "shared_prefs").list().orEmpty()
                .filter { it.endsWith(".xml") }
                .map { it.removeSuffix(".xml") }
            // .all waits until the file is parsed, so the next reader finds it loaded
            for (name in names) app.getSharedPreferences(name, Context.MODE_PRIVATE).all
            // loads libcadenza.so (System.loadLibrary) if the APK has it; the player asks for it when it starts
            CadenzaEngine.available
        } catch (e: Exception) {
            Log.w(TAG, "warm-up stopped early", e)
        }
    }
}
