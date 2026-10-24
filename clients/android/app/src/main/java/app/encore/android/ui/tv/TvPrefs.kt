/**
 * This device's TV settings (Settings → TV) and whether the device is a TV at all. They belong to the TV, not to the
 * account, so they live in this device's SharedPreferences ("encore_ui", next to the player's lyrics mode) and never
 * reach the server, the web or a phone.
 */
package app.encore.android.ui.tv

import android.app.UiModeManager
import android.content.Context
import android.content.pm.PackageManager
import android.content.res.Configuration
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * The TV settings as flows the UI follows: [mode] (TV layout or phone layout), [side] (which side the TV player's
 * controls take), [panel] (what the player's other half shows, remembered as it is switched) and [zoom].
 *
 * Read once from SharedPreferences on first use ([init]); each setter updates its flow at once and writes in the
 * background (`apply`).
 */
object TvPrefs {
    /** The SharedPreferences file, shared with LyricsMode (the player's own UI state). */
    private const val FILE = "encore_ui"

    /** Key of [mode]. */
    private const val KEY_MODE = "encore.tv.layout"

    /** Key of [side]. */
    private const val KEY_SIDE = "encore.tv.playerSide"

    /** Key of [panel]. */
    private const val KEY_PANEL = "encore.tv.panel"

    /** Key of [zoom]. */
    private const val KEY_ZOOM = "encore.tv.zoom"

    /** Backing flow of [mode]. */
    private val modeFlow = MutableStateFlow(TvLayoutMode.AUTO)

    /** Backing flow of [side]. */
    private val sideFlow = MutableStateFlow(PlayerSide.LEFT)

    /** Backing flow of [panel]. */
    private val panelFlow = MutableStateFlow(SidePanel.LYRICS)

    /** Backing flow of [zoom]. */
    private val zoomFlow = MutableStateFlow(TvZoom.MEDIUM)

    /** Whether the stored values were read into the flows yet. */
    @Volatile private var loaded = false

    /** The app context, kept from [init] for the writes. */
    @Volatile private var app: Context? = null

    /** TV layout or phone layout (Settings → TV → Layout). */
    val mode: StateFlow<TvLayoutMode> = modeFlow.asStateFlow()

    /** The side the TV player's controls take (Settings → TV → Player side). */
    val side: StateFlow<PlayerSide> = sideFlow.asStateFlow()

    /** What the TV player's switchable half shows (switched in the player, remembered here). */
    val panel: StateFlow<SidePanel> = panelFlow.asStateFlow()

    /** How far the TV layout zooms out (Settings → TV → Size). */
    val zoom: StateFlow<TvZoom> = zoomFlow.asStateFlow()

    /**
     * Reads the stored values into the flows, once; later calls do nothing. Every composable entry point calls it, so
     * nothing else has to remember to.
     */
    @Synchronized
    fun init(context: Context) {
        if (loaded) return
        val c = context.applicationContext
        app = c
        try {
            val sp = c.getSharedPreferences(FILE, Context.MODE_PRIVATE)
            modeFlow.value = TvLayoutMode.of(sp.getString(KEY_MODE, null))
            sideFlow.value = PlayerSide.of(sp.getString(KEY_SIDE, null))
            panelFlow.value = SidePanel.of(sp.getString(KEY_PANEL, null))
            zoomFlow.value = TvZoom.of(sp.getString(KEY_ZOOM, null))
        } catch (_: Exception) {
            // unreadable preferences: the defaults stand
        }
        loaded = true
    }

    /** Sets [mode] and stores it. */
    fun setMode(v: TvLayoutMode) { modeFlow.value = v; write(KEY_MODE, v.key) }

    /** Sets [side] and stores it. */
    fun setSide(v: PlayerSide) { sideFlow.value = v; write(KEY_SIDE, v.key) }

    /** Sets [panel] and stores it. */
    fun setPanel(v: SidePanel) {
        if (panelFlow.value == v) return
        panelFlow.value = v
        write(KEY_PANEL, v.key)
    }

    /** Sets [zoom] and stores it. */
    fun setZoom(v: TvZoom) { zoomFlow.value = v; write(KEY_ZOOM, v.key) }

    /** Writes one value in the background; a failure only loses the value for the next start. */
    private fun write(key: String, value: String) {
        val c = app ?: return
        try {
            c.getSharedPreferences(FILE, Context.MODE_PRIVATE).edit().putString(key, value).apply()
        } catch (_: Exception) {
        }
    }

    /**
     * Whether this device is a TV: Android's television UI mode (Android TV, Google TV, a Shield), or failing that a
     * device that declares leanback (some TV boxes report the normal UI mode).
     */
    fun isTvDevice(context: Context): Boolean {
        val ui = context.getSystemService(Context.UI_MODE_SERVICE) as? UiModeManager
        if (ui?.currentModeType == Configuration.UI_MODE_TYPE_TELEVISION) return true
        val pm = context.packageManager
        return pm.hasSystemFeature(PackageManager.FEATURE_LEANBACK) || pm.hasSystemFeature("android.software.leanback_only")
    }
}

/**
 * Whether the app should use the TV layout now ([TvRules.useTvLayout]): on a TV, unless Settings → TV says
 * "Phone layout". Follows the setting as it changes.
 */
@Composable
fun rememberTvLayout(): Boolean {
    val context = LocalContext.current
    // the device never stops or starts being a TV while the app runs: asked once
    val isTv = remember(context) { TvPrefs.init(context); TvPrefs.isTvDevice(context) }
    val mode by TvPrefs.mode.collectAsState()
    return TvRules.useTvLayout(mode, isTv)
}

/** Whether this device is a TV ([TvPrefs.isTvDevice]), for Settings to show its TV page only there. */
@Composable
fun rememberIsTvDevice(): Boolean {
    val context = LocalContext.current
    return remember(context) { TvPrefs.init(context); TvPrefs.isTvDevice(context) }
}
