package app.encore.android

import android.app.Application
import android.content.Context
import android.net.ConnectivityManager
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.platform.LocalContext
import app.encore.android.data.api.EncoreApi
import coil3.ImageLoader
import coil3.annotation.ExperimentalCoilApi
import coil3.disk.DiskCache
import coil3.disk.directory
import coil3.memory.MemoryCache
import coil3.network.DeDupeConcurrentRequestStrategy
import coil3.network.cachecontrol.CacheControlCacheStrategy
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import coil3.request.CachePolicy
import coil3.request.ImageRequest
import coil3.request.crossfade
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import java.io.File
import kotlin.math.abs

/**
 * The cover loader: covers go through the app's client (so they carry the session), ask for WebP, keep
 * ~20% of the app's memory for decoded covers and 250 MB on disk, and follow the server's cache headers
 * (versioned cover URLs are immutable, the rest revalidate with ETag after a day).
 */
@OptIn(ExperimentalCoilApi::class)
fun buildCoverImageLoader(app: Application, http: OkHttpClient): ImageLoader {
    // no OkHttp cache underneath: Coil's disk cache holds the covers
    val images = http.newBuilder()
        .cache(null)
        .addInterceptor { chain ->
            val r = chain.request()
            chain.proceed(if (r.header("Accept") == null) r.newBuilder().header("Accept", "image/webp,image/*;q=0.8").build() else r)
        }
        .build()
    return ImageLoader.Builder(app)
        .components {
            add(
                OkHttpNetworkFetcherFactory(
                    callFactory = { images },
                    cacheStrategy = { CacheControlCacheStrategy() },
                    // a preload and the tile on screen asking for the same cover share one download
                    concurrentRequestStrategy = { DeDupeConcurrentRequestStrategy() },
                ),
            )
        }
        .memoryCache { MemoryCache.Builder().maxSizePercent(app, 0.20).build() }
        // a folder of its own: CoverProvider keeps its files in cacheDir/covers
        .diskCache { DiskCache.Builder().directory(File(app.cacheDir, "covers/images")).maxSizeBytes(250L * 1024 * 1024).build() }
        .crossfade(120)
        .build()
}

/**
 * Fetches covers ahead of the scroll: each page of albums a list loads, 2–3 screens ahead in the direction the
 * list moves (and around the spot after a scrubber jump), the first page of each Library tab when the app starts
 * (to disk only, so on-screen covers keep the memory), and the next song's big cover. Skipped on metered networks.
 */
class CoverPreloader(private val context: Context, private val api: EncoreApi, private val loader: ImageLoader) {
    private val done = java.util.Collections.synchronizedSet(LinkedHashSet<String>())

    private fun metered(): Boolean = try {
        (context.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager)?.isActiveNetworkMetered ?: true
    } catch (_: Exception) {
        true
    }

    /** Covers of a page of albums at the grid size. */
    fun albums(ids: List<String>) {
        if (ids.isEmpty() || metered()) return
        ids.take(MAX_PER_PAGE).forEach { enqueue(api.coverUrl(it, 400)) }
    }

    /** Covers of these albums at one size (a list row's 96, a grid's 400). */
    fun covers(ids: List<String>, size: Int) {
        if (ids.isEmpty() || metered()) return
        ids.take(MAX_AHEAD).forEach { if (it.isNotEmpty()) enqueue(api.coverUrl(it, size)) }
    }

    /**
     * The first page of each Library tab, in the sort the tab had last (LibraryScreen keeps it in the
     * "encore_library" preferences as "key:dir"), so the covers are on disk before the tab is opened.
     */
    suspend fun firstPages() {
        if (metered()) return
        val prefs = context.getSharedPreferences("encore_library", Context.MODE_PRIVATE)
        fun saved(tab: String, key: String, dir: String): Pair<String, String> {
            val s = try { prefs.getString("sort.$tab", null) } catch (_: Exception) { null } ?: return key to dir
            return s.substringBefore(':') to s.substringAfter(':', "")
        }
        withContext(Dispatchers.IO) {
            try {
                val (ak, ad) = saved("albums", "added", "desc")
                // api.albums tells onAlbumPage, which fetches the page's covers
                api.albums(sort = ak, limit = 60, dir = ad)
                val (rk, rd) = saved("artists", "name", "asc")
                covers(api.artists(sort = rk, dir = rd).take(60).map { it.coverAlbum }, 400)
                val (tk, td) = saved("tracks", "added", "desc")
                covers(api.tracks(limit = 60, sort = tk, dir = td).map { it.albumId }.distinct(), 96)
            } catch (_: Exception) {
                // offline or signed out: the lists load them when opened
            }
        }
    }

    /** The next song's covers: the full player's and the mini player's. */
    fun nextSong(albumId: String) {
        if (albumId.isEmpty()) return
        enqueue(api.coverUrl(albumId, 1000))
        enqueue(api.coverUrl(albumId, 96))
    }

    private fun enqueue(url: String) {
        if (!url.startsWith("http") || !done.add(url)) return
        if (done.size > 2000) synchronized(done) { done.iterator().let { it.next(); it.remove() } }
        loader.enqueue(
            ImageRequest.Builder(context)
                .data(url)
                .memoryCachePolicy(CachePolicy.DISABLED)
                .build(),
        )
    }

    private companion object {
        const val MAX_PER_PAGE = 60
        const val MAX_AHEAD = 120
    }
}

/**
 * Fetches the covers of the rows 3 screens ahead of a grid in the direction it scrolls, and 2 screens both ways
 * after a jump (the scrubber). [albumIdAt] gives a row's album (null while the row isn't loaded: it is fetched
 * once the row loads).
 */
@Composable
fun PreloadCoversAhead(state: LazyGridState, count: Int, size: Int = 400, albumIdAt: (Int) -> String?) =
    PreloadAhead({ state.firstVisibleItemIndex }, { state.layoutInfo.visibleItemsInfo.size }, count, size, albumIdAt)

/** [PreloadCoversAhead] for a list. */
@Composable
fun PreloadCoversAhead(state: LazyListState, count: Int, size: Int = 96, albumIdAt: (Int) -> String?) =
    PreloadAhead({ state.firstVisibleItemIndex }, { state.layoutInfo.visibleItemsInfo.size }, count, size, albumIdAt)

@Composable
private fun PreloadAhead(first: () -> Int, visible: () -> Int, count: Int, size: Int, albumIdAt: (Int) -> String?) {
    val preloader = LocalContext.current.graph.coverPreloader
    val total by rememberUpdatedState(count)
    val idAt by rememberUpdatedState(albumIdAt)
    LaunchedEffect(preloader, size) {
        var prev = 0
        var dir = 1   // 1 down, -1 up, 0 just jumped
        snapshotFlow {
            val f = first()
            val v = visible().coerceAtLeast(1)
            if (f != prev) {
                dir = if (abs(f - prev) > v * 3) 0 else if (f > prev) 1 else -1
                prev = f
            }
            val n = total
            val range = when (dir) {
                1 -> (f + v) until minOf(n, f + v + v * 3)
                -1 -> maxOf(0, f - v * 3) until f
                else -> maxOf(0, f - v * 2) until minOf(n, f + v * 3)
            }
            range.mapNotNull { i -> idAt(i)?.takeIf { it.isNotEmpty() } }.distinct()
        }
            .distinctUntilChanged()
            .collect { ids -> preloader.covers(ids, size) }
    }
}
