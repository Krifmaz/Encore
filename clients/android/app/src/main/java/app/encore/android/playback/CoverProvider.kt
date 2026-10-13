package app.encore.android.playback

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import app.encore.android.data.api.EncoreApi
import app.encore.android.graph
import kotlinx.coroutines.runBlocking
import java.io.File
import java.io.FileNotFoundException

/**
 * Album covers as `content://<applicationId>.covers/album/<albumId>?s=600`.
 *
 * Android Auto and the system media UI load artwork in their own process and can't send our session, so the
 * session's items point here; this provider fetches the cover with the app's authenticated client once and
 * serves it from the cache directory. Only the fixed sizes the server keeps thumbnails for are allowed.
 */
class CoverProvider : ContentProvider() {
    override fun onCreate(): Boolean = true

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor? {
        if (mode != "r") throw FileNotFoundException("read-only")
        val segments = uri.pathSegments
        if (segments.size != 2 || segments[0] != "album") throw FileNotFoundException(uri.toString())
        val albumId = segments[1]
        val requested = uri.getQueryParameter("s")?.toIntOrNull() ?: 600
        val size = SIZES.firstOrNull { it >= requested } ?: SIZES.last()
        val ctx = context ?: throw FileNotFoundException("no context")
        val dir = File(ctx.cacheDir, "covers")
        dir.mkdirs()
        val graph = ctx.graph
        // a cold start for the provider alone (Auto / System UI asking for art): wait for the saved session.
        // openFile runs on a binder thread, so blocking here is fine.
        try { runBlocking { graph.store.awaitLoaded() } } catch (_: Exception) {}
        val api = graph.api
        if (api.base.isEmpty()) throw FileNotFoundException("not signed in")
        // keyed by server too: album ids are only unique within one library
        val file = File(dir, Integer.toHexString(api.base.hashCode()) + "_" + safeName(albumId) + "_" + size + ".jpg")
        val fresh = file.isFile && System.currentTimeMillis() - file.lastModified() < MAX_AGE_MS
        if (!fresh) download(api, albumId, size, file)
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    private fun download(api: EncoreApi, albumId: String, size: Int, file: File) {
        val tmp = File(file.parentFile, file.name + "." + System.nanoTime() + ".part")
        try {
            api.rawGet("/api/library/albums/${EncoreApi.enc(albumId)}/cover?s=$size").use { res ->
                if (!res.isSuccessful) throw FileNotFoundException("HTTP ${res.code}")
                tmp.outputStream().use { out -> res.body.byteStream().use { it.copyTo(out) } }
            }
            if (!tmp.renameTo(file)) throw FileNotFoundException("cache write failed")
        } catch (e: FileNotFoundException) {
            tmp.delete()
            if (!file.isFile) throw e
        } catch (e: Exception) {
            tmp.delete()
            // offline: an older copy is better than nothing
            if (!file.isFile) throw FileNotFoundException(e.message ?: "cover unavailable")
        }
    }

    override fun getType(uri: Uri): String = "image/jpeg"

    override fun query(uri: Uri, projection: Array<String>?, selection: String?, selectionArgs: Array<String>?, sortOrder: String?): Cursor? = null

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<String>?): Int = 0

    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<String>?): Int = 0

    private fun safeName(s: String): String = s.map { c -> if (c.isLetterOrDigit() || c == '-' || c == '_') c else '_' }.joinToString("").take(80) +
        "_" + Integer.toHexString(s.hashCode())

    private companion object {
        val SIZES = listOf(96, 300, 600, 1200)
        const val MAX_AGE_MS = 7L * 24 * 3600 * 1000
    }
}
