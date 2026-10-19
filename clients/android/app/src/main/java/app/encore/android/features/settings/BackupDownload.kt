package app.encore.android.features.settings

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.os.Build
import android.os.Environment
import android.os.SystemClock
import android.provider.MediaStore
import androidx.annotation.RequiresApi
import app.encore.android.AppGraph
import app.encore.android.R
import app.encore.android.core.parseJsonObject
import app.encore.android.core.str
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.SettingsApi
import app.encore.android.data.session.SessionStore
import app.encore.android.ui.components.Toasts
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.CacheControl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.Interceptor
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.concurrent.TimeUnit

/**
 * Settings → Diagnostics → Backups → Download: the file is fetched with the app's own HTTP client, so the session goes
 * only to the signed-in server, as for every other call. (The system's download manager would keep the session in
 * its database until the download is removed from its list, and send it again after a redirect to any other host.)
 * It is written to the phone's Downloads folder: through MediaStore from Android 10, as a plain file with the storage
 * permission on Android 8–9 ([needsPermission]: the caller asks for it first). A notification shows the progress; the
 * download goes on when the page is left.
 */
object BackupDownload {
    /** Backups being fetched (a second press on the same one does nothing). */
    private val running: MutableSet<String> = java.util.Collections.synchronizedSet(HashSet())

    /** Android 8–9 without the storage permission (the Downloads folder needs it there; MediaStore does not from 10). */
    fun needsPermission(context: Context): Boolean =
        Build.VERSION.SDK_INT < 29 && context.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) != PackageManager.PERMISSION_GRANTED

    /** The file name in Downloads: the server's name cleaned so it cannot point anywhere else. */
    fun fileName(name: String): String = name.replace(Regex("[^A-Za-z0-9._-]"), "_").trimStart('.').ifEmpty { "encore-backup.db" }

    /** Starts the download of backup [name] in the app's scope (toasts and the notification say how it went). */
    fun start(graph: AppGraph, api: SettingsApi, name: String) {
        val app = graph.app
        val file = fileName(name)
        if (!running.add(name)) return
        val id = NOTIFY_BASE + (name.hashCode() and 0xff)
        Toasts.show("Downloading $file…", "ok")
        graph.scope.launch {
            try {
                val saved = withContext(Dispatchers.IO) { fetch(app, graph, api, name, file, id) }
                Toasts.show("Saved in Downloads: $saved", "ok")
                post(app, id, builder(app).setContentTitle(saved).setContentText("Saved in Downloads").setAutoCancel(true).build())
            } catch (e: CancellationException) {
                cancel(app, id)
                throw e
            } catch (e: Exception) {
                val msg = e.message?.ifEmpty { null } ?: "Download failed"
                Toasts.show(msg, "err")
                post(app, id, builder(app).setContentTitle(file).setContentText("Download failed").setAutoCancel(true).build())
            } finally {
                running.remove(name)
            }
        }
    }

    /** GET /api/system/backups/{name} into Downloads; the name it was saved under. */
    private fun fetch(app: Context, graph: AppGraph, api: SettingsApi, name: String, file: String, id: Int): String {
        if (api.base.isEmpty()) throw IOException("Not signed in")
        val req = Request.Builder().url(api.backupUrl(name)).cacheControl(CacheControl.Builder().noStore().build()).get().build()
        client(graph).newCall(req).execute().use { res ->
            if (!res.isSuccessful) {
                val o = parseJsonObject(res.body.string())
                throw ApiException(res.code, o?.str("detail")?.ifEmpty { null } ?: "HTTP ${res.code}", o)
            }
            // a sign-in page of a proxy in front of the server is not the backup
            val host = res.request.url.host
            if (host != req.url.host) throw IOException("The download was sent on to another address ($host)")
            val progress = Progress(app, id, file, res.body.contentLength())
            progress.show(0L)
            val input = res.body.byteStream()
            return if (Build.VERSION.SDK_INT >= 29) toMediaStore(app, file, input, progress) else toFolder(app, file, input, progress)
        }
    }

    /**
     * The app's client without its disk cache (a database is read once, and can be gigabytes), and with the session
     * taken off any hop of a redirect that leaves the signed-in server (OkHttp itself only drops Authorization there).
     */
    private fun client(graph: AppGraph): OkHttpClient =
        graph.http.newBuilder()
            .cache(null)
            .readTimeout(STALL_SECONDS, TimeUnit.SECONDS)
            .addNetworkInterceptor(SameServerOnly(graph.store))
            .build()

    /**
     * Each hop, redirects included: the session headers go only to the signed-in server's host (a move from http to
     * https on the same host keeps them, as every other call does).
     */
    private class SameServerOnly(private val store: SessionStore) : Interceptor {
        override fun intercept(chain: Interceptor.Chain): okhttp3.Response {
            val r = chain.request()
            val home = store.current?.server?.toHttpUrlOrNull()
            val same = home != null && r.url.host == home.host
            return chain.proceed(if (same) r else r.newBuilder().removeHeader("Authorization").removeHeader("Cookie").build())
        }
    }

    /** Android 10 and later: a pending Downloads row while it is written (removed again when the download fails). */
    @RequiresApi(29)
    private fun toMediaStore(app: Context, file: String, input: InputStream, progress: Progress): String {
        val resolver = app.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, file)
            put(MediaStore.MediaColumns.MIME_TYPE, MIME)
            put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/")
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: throw IOException(NO_SAVE)
        try {
            val out = resolver.openOutputStream(uri) ?: throw IOException(NO_SAVE)
            out.use { copy(input, it, progress) }
            resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
        } catch (e: Throwable) {
            try { resolver.delete(uri, null, null) } catch (_: Exception) {}
            throw e
        }
        // the name it got: MediaStore numbers a name that is taken
        return try {
            resolver.query(uri, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
        } catch (_: Exception) {
            null
        } ?: file
    }

    /** Android 8–9: a file in the public Downloads folder (a taken name gets " (2)"), then shown to the media scanner. */
    @Suppress("DEPRECATION")
    private fun toFolder(app: Context, file: String, input: InputStream, progress: Progress): String {
        if (needsPermission(app)) throw IOException("Encore needs access to the phone's storage to save the backup in Downloads")
        val dir = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)
        if (!dir.isDirectory && !dir.mkdirs()) throw IOException(NO_SAVE)
        val target = unique(dir, file)
        try {
            FileOutputStream(target).use { copy(input, it, progress) }
        } catch (e: Throwable) {
            target.delete()
            throw e
        }
        MediaScannerConnection.scanFile(app, arrayOf(target.path), null, null)
        return target.name
    }

    private fun unique(dir: File, file: String): File {
        val first = File(dir, file)
        if (!first.exists()) return first
        val dot = file.lastIndexOf('.')
        val base = if (dot > 0) file.substring(0, dot) else file
        val ext = if (dot > 0) file.substring(dot) else ""
        for (n in 2..999) {
            val f = File(dir, "$base ($n)$ext")
            if (!f.exists()) return f
        }
        throw IOException(NO_SAVE)
    }

    private fun copy(input: InputStream, out: OutputStream, progress: Progress) {
        val buf = ByteArray(64 * 1024)
        var done = 0L
        while (true) {
            val n = input.read(buf)
            if (n < 0) break
            out.write(buf, 0, n)
            done += n
            progress.update(done)
        }
        out.flush()
    }

    /** The notification while the file comes in (redrawn at most twice a second). */
    private class Progress(private val app: Context, private val id: Int, private val file: String, private val total: Long) {
        private var shownAt = 0L

        fun update(done: Long) {
            val now = SystemClock.elapsedRealtime()
            if (now - shownAt < 500) return
            shownAt = now
            show(done)
        }

        fun show(done: Long) {
            val known = total > 0
            val pct = if (known) ((done * 100) / total).toInt().coerceIn(0, 100) else 0
            post(
                app,
                id,
                builder(app)
                    .setContentTitle(file)
                    .setContentText("Downloading…")
                    .setProgress(100, pct, !known)
                    .setOngoing(true)
                    .setOnlyAlertOnce(true)
                    .build(),
            )
        }
    }

    private fun builder(app: Context): Notification.Builder {
        val nm = app.getSystemService(NotificationManager::class.java)
        if (nm != null && nm.getNotificationChannel(CHANNEL) == null) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW))
        }
        return Notification.Builder(app, CHANNEL).setSmallIcon(R.drawable.ic_download).setShowWhen(false)
    }

    /** Without the notification permission (Android 13+) nothing shows; the toasts still say how it went. */
    private fun post(app: Context, id: Int, n: Notification) {
        try { app.getSystemService(NotificationManager::class.java)?.notify(id, n) } catch (_: SecurityException) {}
    }

    private fun cancel(app: Context, id: Int) {
        try { app.getSystemService(NotificationManager::class.java)?.cancel(id) } catch (_: Exception) {}
    }

    /** The offline songs' channel ("Downloads"). */
    private const val CHANNEL = "downloads"
    private const val NOTIFY_BASE = 7400
    /** No data for this long: the download stops. */
    private const val STALL_SECONDS = 60L
    /** Any name is kept as it is (a known type could get another extension added). */
    private const val MIME = "application/octet-stream"
    private const val NO_SAVE = "Couldn't save the file in Downloads"
}
