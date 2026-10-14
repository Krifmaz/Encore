package app.encore.android

import android.app.Application
import android.content.Context
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.buildHttpClient
import app.encore.android.data.library.LikesRepository
import app.encore.android.data.prefs.PrefsRepository
import app.encore.android.data.session.SessionStore
import coil3.ImageLoader
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import okhttp3.OkHttpClient

class EncoreApp : Application() {
    lateinit var graph: AppGraph
        private set

    override fun onCreate() {
        super.onCreate()
        // first, so a crash while building the graph is written too (CrashLog.kt)
        CrashLog.install(this)
        graph = AppGraph(this)
        // reports from earlier runs (our own, and native crashes / ANRs the system recorded) go to the server
        CrashLog.uploadPending(this, graph)
    }
}

/** The app's singletons, built by hand (no DI framework in M1). */
class AppGraph(app: Application) {
    val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    val store: SessionStore = SessionStore(app, scope)
    val http: OkHttpClient = buildHttpClient(app, store)
    val api: EncoreApi = EncoreApi(http, store)
    val likes: LikesRepository = LikesRepository(api, scope)

    /** Playback preferences (quality, effects, server processing), synced with the account. */
    val prefs: PrefsRepository = PrefsRepository(app, api, scope, store)

    /** Covers load through the same client, so they carry the session (CoverImages.kt: caches, WebP, headers). */
    val imageLoader: ImageLoader = buildCoverImageLoader(app, http)

    /** Fetches covers ahead of the scroll and the next song's cover. */
    val coverPreloader: CoverPreloader = CoverPreloader(app, api, imageLoader).also { p -> api.onAlbumPage = { p.albums(it) } }
}

val Context.graph: AppGraph get() = (applicationContext as EncoreApp).graph
