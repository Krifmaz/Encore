package app.encore.android

import android.app.Application
import android.content.Context
import android.net.ConnectivityManager
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
import okhttp3.OkHttpClient
import java.io.File

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
 * Fetches covers ahead of the scroll: each page of albums a list loads (to disk only, so on-screen covers
 * keep the memory), and the next song's big cover. Skipped on metered networks.
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
    }
}
