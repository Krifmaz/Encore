package app.encore.android.data.api

import android.content.Context
import app.encore.android.CrashLog
import app.encore.android.data.session.Session
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicInteger

/**
 * The server's last answers, kept on this phone so the app stays usable while the server can't be reached
 * (offline mode, features/offline/OfflineMode.kt).
 *
 * Every successful `GET /api/…` that [EncoreApi] makes for the signed-in server is written here as the JSON text the
 * server sent, one file per account and URL. When the server can't be reached the same call is answered from this
 * copy instead, so Home, Library, Playlists, an album or a playlist the user opened before, and the Settings pages
 * still show what they showed last time. Writes (POST, PUT, DELETE) are never kept or replayed.
 *
 * Kept in the app's files (not its cache folder, which Android may empty exactly when the phone is short of space
 * and offline matters most), one folder per account so another account signed in on the same phone never sees these
 * answers, and trimmed to [MAX_BYTES], oldest-used first.
 */
object ApiCache {
    /** At most this much is kept over all accounts (the oldest-used answers go first). */
    private const val MAX_BYTES = 40L * 1024L * 1024L

    /** The trim runs after this many writes (listing the folder on every write would cost more than it saves). */
    private const val TRIM_EVERY = 40

    /**
     * Paths never kept: sign-in and the health check (they must ask the server), live state that is wrong the moment
     * it is old (other devices, group listens, the event stream, playback rendering), and anything that is not JSON.
     */
    private val SKIP_PREFIXES = listOf(
        "/api/health",
        "/api/auth/info",
        "/api/auth/login",
        "/api/remote/",
        "/api/group",
        "/api/events",
        "/api/playback/",
    )

    /** `<files>/api-cache`; null until [init] ran (then nothing is kept or read). */
    @Volatile private var root: File? = null

    /** Writes since the last trim. */
    private val writes = AtomicInteger(0)

    /** Called once from [app.encore.android.AppGraph]: where the answers live. */
    fun init(context: Context) {
        root = File(context.applicationContext.filesDir, "api-cache")
    }

    /**
     * Whether the answer to `GET fullUrl` may be kept: the URL is on the signed-in server ([base]), is an `/api/` path,
     * and is not one of [SKIP_PREFIXES]. Song streams (`…/stream`) and covers never pass through [EncoreApi.call], but
     * are refused here too in case a caller ever routes them there.
     */
    fun cacheable(fullUrl: String, base: String): Boolean {
        if (base.isEmpty() || !fullUrl.startsWith(base)) return false
        val path = fullUrl.substring(base.length).substringBefore('?')
        if (!path.startsWith("/api/")) return false
        if (path.endsWith("/stream") || path.endsWith("/cover")) return false
        return SKIP_PREFIXES.none { path.startsWith(it) }
    }

    /** The answer kept for [owner] (`server|username`) and [url], or null when there is none. Any thread. */
    fun get(owner: String, url: String): String? {
        val f = fileOf(owner, url) ?: return null
        return try {
            if (!f.isFile) return null
            // "used now": the trim keeps what is read, not only what was written lately
            f.setLastModified(System.currentTimeMillis())
            f.readText()
        } catch (e: Exception) {
            CrashLog.log("api cache: not read: ${e.message}")
            null
        }
    }

    /** Keeps [text] as the answer for [owner] and [url]; written to a side file first so a crash never leaves half a file. */
    fun put(owner: String, url: String, text: String) {
        val f = fileOf(owner, url) ?: return
        try {
            f.parentFile?.mkdirs()
            val tmp = File(f.parentFile, f.name + ".tmp")
            tmp.writeText(text)
            if (!tmp.renameTo(f)) {
                f.delete()
                tmp.renameTo(f)
            }
        } catch (e: Exception) {
            CrashLog.log("api cache: not written: ${e.message}")
            return
        }
        if (writes.incrementAndGet() >= TRIM_EVERY) {
            writes.set(0)
            trim()
        }
    }

    /** Whose answers these are: the server and the user name, so two accounts on one phone never share them. */
    fun ownerOf(session: Session): String = session.server + "|" + session.username

    /** Removes everything kept for [owner] (`server|username`), e.g. when that account signs out. */
    fun forget(owner: String) {
        val r = root ?: return
        try { File(r, hash(owner)).deleteRecursively() } catch (_: Exception) { }
    }

    /** `<root>/<hash of owner>/<hash of url>.json`; null before [init] or without an owner. */
    private fun fileOf(owner: String, url: String): File? {
        val r = root ?: return null
        if (owner.isEmpty()) return null
        return File(File(r, hash(owner)), hash(url) + ".json")
    }

    /** Deletes the oldest-used answers until all of them fit in [MAX_BYTES]. */
    private fun trim() {
        val r = root ?: return
        try {
            val files = r.walkTopDown().filter { it.isFile }.toMutableList()
            var total = files.sumOf { it.length() }
            if (total <= MAX_BYTES) return
            files.sortBy { it.lastModified() }
            for (f in files) {
                if (total <= MAX_BYTES) break
                val len = f.length()
                if (f.delete()) total -= len
            }
        } catch (e: Exception) {
            CrashLog.log("api cache: not trimmed: ${e.message}")
        }
    }

    /** SHA-256 of [s] in hex: a file name that is safe whatever the URL or user name holds. */
    private fun hash(s: String): String =
        MessageDigest.getInstance("SHA-256").digest(s.toByteArray()).joinToString("") { "%02x".format(it) }
}
