package app.encore.android

import android.app.Application
import android.content.Context
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.buildHttpClient
import app.encore.android.data.library.LikesRepository
import app.encore.android.data.prefs.PrefsRepository
import app.encore.android.data.session.SessionStore
import coil3.ImageLoader
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import okhttp3.OkHttpClient

class EncoreApp : Application() {
    lateinit var graph: AppGraph
        private set

    override fun onCreate() {
        super.onCreate()
        graph = AppGraph(this)
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

    /** Covers load through the same client, so they carry the session. */
    val imageLoader: ImageLoader = ImageLoader.Builder(app)
        .components { add(OkHttpNetworkFetcherFactory(http)) }
        .build()
}

val Context.graph: AppGraph get() = (applicationContext as EncoreApp).graph
