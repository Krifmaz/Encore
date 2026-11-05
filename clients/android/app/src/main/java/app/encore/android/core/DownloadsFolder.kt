package app.encore.android.core

import android.Manifest
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import androidx.annotation.RequiresApi
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream

/**
 * Saves a stream (the Downloader's MP4, straight from the server's response) into the phone's public Downloads
 * folder, so the file is simply there, in Files → Downloads, with no share sheet or picker.
 *
 * Exports of a finished file (bug reports, crash logs) use kit-compose's `dev.krifmaz.kit.compose.core.DownloadsFolder`;
 * this copy only adds [saveStream], which the kit does not have yet, and goes once kit-compose saves a stream
 * (docs/DECISIONS.md).
 *
 * Android 10 and later write through MediaStore (no permission needed for the app's own rows); Android 8–9 write a
 * plain file, which needs the storage permission there.
 */
object DownloadsFolder {
    /** The words of the [IOException] when the folder refuses the file. */
    const val NO_SAVE = "Couldn't save the file in Downloads"

    /** Whether a save can write here: always from Android 10 (MediaStore), on Android 8–9 only with the storage permission. */
    private fun canSave(context: Context): Boolean =
        Build.VERSION.SDK_INT >= 29 || context.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED

    /**
     * Copies [input] (read to its end, not closed) into Downloads as [name] with type [mime]; gives the name it was
     * saved under (a name already taken gets a number, "name (1).mp4"). A copy that fails part way leaves nothing
     * behind. Blocking: call it off the main thread.
     */
    fun saveStream(context: Context, name: String, mime: String, input: InputStream): String {
        val write: (OutputStream) -> Unit = { o -> input.copyTo(o) }
        return if (Build.VERSION.SDK_INT >= 29) toMediaStore(context, name, mime, write) else toFolder(context, name, write)
    }

    /** Android 10 and later: a pending Downloads row while it is written, removed again if the copy fails. */
    @RequiresApi(29)
    private fun toMediaStore(context: Context, name: String, mime: String, write: (OutputStream) -> Unit): String {
        val resolver = context.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, mime)
            put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/")
            // hidden from other apps until the copy is complete
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: throw IOException(NO_SAVE)
        try {
            val out = resolver.openOutputStream(uri) ?: throw IOException(NO_SAVE)
            out.use(write)
            resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
        } catch (e: Throwable) {
            try { resolver.delete(uri, null, null) } catch (_: Exception) {}
            throw e
        }
        // MediaStore numbers a name that is taken, so read back the name it really got
        return try {
            resolver.query(uri, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
        } catch (_: Exception) {
            null
        } ?: name
    }

    /** Android 8–9: a plain file in the public Downloads folder, then shown to the media scanner (Files lists it). */
    @Suppress("DEPRECATION")
    private fun toFolder(context: Context, name: String, write: (OutputStream) -> Unit): String {
        if (!canSave(context)) throw IOException("Encore needs access to the phone's storage to save in Downloads")
        val dir = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)
        if (!dir.isDirectory && !dir.mkdirs()) throw IOException(NO_SAVE)
        val target = unique(dir, name)
        try {
            FileOutputStream(target).use(write)
        } catch (e: Throwable) {
            target.delete()
            throw e
        }
        MediaScannerConnection.scanFile(context, arrayOf(target.path), null, null)
        return target.name
    }

    /** [file] in [dir], or "name (2).ext", "name (3).ext"… when it is taken (as MediaStore numbers on Android 10+). */
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
}
