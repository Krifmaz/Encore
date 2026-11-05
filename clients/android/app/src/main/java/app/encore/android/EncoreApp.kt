package app.encore.android

import android.app.Application
import android.content.Context
import app.encore.android.data.api.ApiCache
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.USER_AGENT
import app.encore.android.data.api.buildHttpClient
import app.encore.android.data.library.LikesRepository
import app.encore.android.data.library.DislikesRepository
import app.encore.android.data.prefs.PrefsRepository
import app.encore.android.data.session.SessionStore
import coil3.ImageLoader
import dev.krifmaz.kit.compose.KitApp
import dev.krifmaz.kit.compose.KitRoutes
import dev.krifmaz.kit.compose.bugs.CrashLog
import dev.krifmaz.kit.compose.update.UpdateRules
import dev.krifmaz.kit.compose.update.UpdateSource
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import okhttp3.OkHttpClient

/**
 * Where Encore's server answers the calls the kit's parts make (KitApp.routes): the kit's defaults, except the
 * settings (admin routes), a settings card's Test, the diagnostics and the app crash reports, which Encore's server
 * keeps at its own paths.
 */
val KIT_ROUTES: KitRoutes = KitRoutes(
    diagnostics = "/api/system/diagnostics",
    clientCrash = "/api/diagnostics/client-crash",
    settingsSchema = "/api/admin/settings/schema",
    settings = "/api/admin/settings",
    settingsSecret = "/api/admin/settings/secret",
    settingsTest = "/api/system/test/{test}",
)

class EncoreApp : Application() {
    lateinit var graph: AppGraph
        private set

    override fun onCreate() {
        super.onCreate()
        // who the app is, for the kit's words and requests (kit-compose: crash log, updates, bug reports), and where
        // its releases are (in-app updates)
        KitApp.init(name = "Encore", versionName = BuildConfig.VERSION_NAME, versionCode = BuildConfig.VERSION_CODE, userAgent = USER_AGENT, id = "encore", routes = KIT_ROUTES)
        UpdateRules.source = UpdateSource("Krifmaz", "Encore", "Encore-android-", "app.encore.client")
        // the "Encore 1.2.3 is available" toast points to where the updater is
        UpdateRules.updatesPlace = "Settings → This device"
        // next, so a crash while building the graph is written too (the kit's CrashLog)
        CrashLog.install(this)
        app.encore.android.data.api.Translations.init(this)
        graph = AppGraph(this)
        // reports from earlier runs (our own, and native crashes / ANRs the system recorded) go to the server
        CrashLog.uploadPending(this, graph.crashHost)
    }
}

/** The app's singletons, built by hand (no DI framework in M1). */
class AppGraph(app: Application) {
    /** The application, for system services (connectivity) from plain functions outside composables. */
    val app: Application = app
    val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    val store: SessionStore = SessionStore(app, scope)

    /** The server's last answers on disk, for offline mode (data/api/ApiCache.kt); set up before anything calls the API. */
    init { ApiCache.init(app) }
    val http: OkHttpClient = buildHttpClient(app, store)
    val api: EncoreApi = EncoreApi(http, store)
    val likes: LikesRepository = LikesRepository(api, scope)
    /** The disliked songs (the player's thumbs-down), which the server's shuffles and mixes leave out. */
    val dislikes: DislikesRepository = DislikesRepository(api, likes, scope)

    /** Playback preferences (quality, effects, server processing), synced with the account. */
    val prefs: PrefsRepository = PrefsRepository(app, api, scope, store)

    /** Covers load through the same client, so they carry the session (CoverImages.kt: caches, WebP, headers). */
    val imageLoader: ImageLoader = buildCoverImageLoader(app, http)

    /** Fetches covers ahead of the scroll and the next song's cover. */
    val coverPreloader: CoverPreloader = CoverPreloader(app, api, imageLoader).also { p -> api.onAlbumPage = { p.albums(it) } }
}

val Context.graph: AppGraph get() = (applicationContext as EncoreApp).graph
