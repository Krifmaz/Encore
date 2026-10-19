package app.encore.android.features.offline

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.webkit.MimeTypeMap
import app.encore.android.CrashLog
import app.encore.android.R
import app.encore.android.core.EncoreJson
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.int
import app.encore.android.core.long
import app.encore.android.core.obj
import app.encore.android.core.str
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.graph
import app.encore.android.ui.MainActivity
import app.encore.android.ui.components.Toasts
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.IOException
import java.io.InterruptedIOException
import java.util.Locale
import java.util.concurrent.TimeUnit

/**
 * Saving songs on this phone: two at a time, in the app's scope (no foreground service: the app or the playback
 * service keeps the process up), with a notification that shows the progress and then how it went. Per song:
 * `GET /api/library/tracks/{id}/info` (with this phone's decoders) says whether the original plays here; the
 * stream is then the file itself, tags included – or, for a codec the phone can't decode, FLAC the server makes
 * (the copy's quality then ends in "(converted)", offline-manifest.md). The album cover and the lyrics come along.
 * A song that fails is reported and the others go on, except after the phone ran out of room.
 */
class OfflineWorker(private val app: Context, private val store: OfflineStore) {
    /** How far the downloads are: `done` of `total` songs, the title being fetched; null when nothing runs. */
    data class Progress(val done: Int, val total: Int, val bytes: Long, val current: String)

    private val _progress = MutableStateFlow<Progress?>(null)
    val progress: StateFlow<Progress?> = _progress.asStateFlow()

    /** One "Download for offline" (a song, an album, a playlist): its own summary at the end. */
    private class Batch(val again: Boolean, val total: Int, var skipped: Int) {
        var done = 0
        var saved = 0
        var stillConverted = 0
        val failed = ArrayList<String>()
    }

    private class Job(val track: Track, val batch: Batch)

    private val lock = Any()
    private val queue = ArrayDeque<Job>()
    /** songs queued or being fetched */
    private val busy = HashSet<Long>()
    private var running = 0
    private var done = 0
    private var total = 0
    private var bytes = 0L
    private var current = ""
    /** the phone ran out of room: the rest of the queue fails at once */
    private var full = false

    private val nm: NotificationManager? = app.getSystemService(NotificationManager::class.java)

    /** Downloads (no HTTP cache: the stream is read once and kept as a file). */
    private val http: OkHttpClient by lazy {
        app.graph.http.newBuilder().cache(null).readTimeout(STALL_SECONDS, TimeUnit.SECONDS).build()
    }

    /** `/info` gets this long: a server that stopped answering must not hold a download slot. */
    private val infoHttp: OkHttpClient by lazy { http.newBuilder().callTimeout(20, TimeUnit.SECONDS).build() }

    /** A song is queued or being fetched. */
    fun isBusy(id: Long): Boolean = synchronized(lock) { id in busy }

    /**
     * "Download for offline": these library songs, the ones not saved yet. `again` ("Download again in original
     * quality"): saved songs are fetched again and replace their copy, only where the original plays here now.
     */
    fun enqueue(tracks: List<Track>, again: Boolean = false) {
        val wanted = tracks.filter { it.id > 0L }.distinctBy { it.id }
        if (wanted.isEmpty()) { Toasts.show("Nothing to download", "ok"); return }
        if (store.server().isEmpty()) { Toasts.show(OfflineIndex.statusWords(401), "err"); return }
        synchronized(lock) {
            val jobs = ArrayList<Track>()
            var skipped = 0
            for (t in wanted) {
                if (t.id in busy || (!again && store.has(t.id))) skipped++ else jobs.add(t)
            }
            if (jobs.isEmpty()) {
                Toasts.show("Already downloaded", "ok")
                return
            }
            val batch = Batch(again, jobs.size, skipped)
            for (t in jobs) { queue.addLast(Job(t, batch)); busy.add(t.id) }
            total += jobs.size
            while (running < AT_ONCE && queue.isNotEmpty()) {
                running++
                app.graph.scope.launch(Dispatchers.IO) { loop() }
            }
        }
        notifyProgress()
        _progress.value?.let { Toasts.show(OfflineIndex.progressText(it.done, it.total, ""), "info") }
    }

    private fun loop() {
        while (true) {
            var idle = false
            // taking the next job and leaving when there is none happen under one lock, so a song queued meanwhile
            // always finds a loop to run it (enqueue starts one when fewer than AT_ONCE run)
            val job = synchronized(lock) {
                val j = queue.removeFirstOrNull()
                if (j == null) {
                    running--
                    if (running == 0) {
                        done = 0; total = 0; bytes = 0L; current = ""; full = false
                        idle = true
                    }
                } else {
                    current = j.track.title
                }
                j
            }
            if (job == null) {
                if (idle) {
                    _progress.value = null
                    nm?.cancel(NOTIFY_PROGRESS)
                }
                return
            }
            val t = job.track
            notifyProgress()
            val outcome: Outcome = try {
                if (synchronized(lock) { full }) Outcome.Failed(NO_ROOM) else saveOne(t, job.batch.again)
            } catch (e: Exception) {
                Outcome.Failed(reason(e))
            }
            if (outcome is Outcome.Failed) {
                CrashLog.log("offline: download of song ${t.id} failed: ${outcome.why}")
                if (outcome.why == NO_ROOM) synchronized(lock) { full = true }
            }
            var finished: Batch? = null
            synchronized(lock) {
                busy.remove(t.id)
                done++
                val b = job.batch
                b.done++
                when (outcome) {
                    is Outcome.Saved -> b.saved++
                    is Outcome.StillConverted -> b.stillConverted++
                    is Outcome.Failed -> b.failed.add(outcome.why)
                }
                if (b.done == b.total) finished = b
            }
            finished?.let { summary(it) }
            notifyProgress()
        }
    }

    private sealed interface Outcome {
        data object Saved : Outcome
        /** "Download again": the original still can't be played here, the converted copy stays */
        data object StillConverted : Outcome
        data class Failed(val why: String) : Outcome
    }

    private class Fail(message: String) : IOException(message)

    /** One song: what to fetch, the audio, the cover and lyrics, then its record (the audio is complete by then). */
    private fun saveOne(t: Track, again: Boolean): Outcome {
        val server = store.server()
        if (server.isEmpty()) throw Fail(OfflineIndex.statusWords(401))
        if (!store.canWrite()) throw Fail(NO_PERMISSION)
        val info = info(server, t.id)
        val format = t.format.ifEmpty { info.str("format") }.lowercase(Locale.ROOT)
        val original = info.bool("offline_ok") || info.obj("stream")?.str("kind") == "original"
        if (again && !original && store.has(t.id)) return Outcome.StillConverted
        // room for the file (the original's size; a conversion is about as big) and a little more
        val size = info.long("size")
        val free = store.freeBytes()
        if (free in 0L until size + MIN_FREE) throw Fail(NO_ROOM)

        val albumArtist = t.albumArtist.ifBlank { t.artist }
        val albumDir = OfflineIndex.albumDir(albumArtist, t.album)
        val req = Request.Builder()
            .url(server + "/api/library/tracks/${t.id}/stream")
            .header(DECODERS_HEADER, OfflineStore.DECODERS)
            .build()
        val (file, written, kept) = http.newCall(req).execute().use { res ->
            if (!res.isSuccessful) throw Fail(OfflineIndex.statusWords(res.code))
            val label = res.header("x-encore-stream")?.trim() ?: ""
            val ctype = res.header("content-type")?.substringBefore(';')?.trim() ?: ""
            // no label: a server before 0.27, which sent the file itself unless it says it converted it
            val asIs = (label.isEmpty() && res.header("x-transcoded").isNullOrBlank()) || label == "original"
            val kept = keptOf(asIs, format, info, t)
            val name = OfflineIndex.fileName(t.trackNo, t.title, kept.ext)
            val mime = mimeOf(kept.ext, ctype)
            val (f, n) = res.body.byteStream().use { input ->
                store.writeAudio(albumDir, name, mime, input) { add -> addBytes(add) }
            }
            Triple(f, n, kept)
        }
        var ok = false
        try {
            val art = saveCover(server, t.albumId, albumDir)
            val lyrics = saveLyrics(server, t.id)
            val rec = OfflineRecord(
                id = t.id,
                title = t.title,
                artist = t.artist.ifBlank { t.albumArtist },
                album = t.album,
                albumId = t.albumId,
                ext = kept.ext,
                bytes = written,
                quality = kept.quality,
                cover = art.isNotEmpty(),
                lyrics = lyrics,
                savedAt = Math.floor(System.currentTimeMillis() / 1000.0),
                sourceServer = server,
                duration = if (t.durationSec > 0) t.durationSec else info.dbl("duration").coerceAtLeast(0.0),
                track = t.trackNo.coerceAtLeast(0),
                disc = t.disc.coerceAtLeast(0),
                format = kept.format,
                artists = OfflineStore.artistsOf(t),
                file = file,
                art = art,
            )
            store.put(rec)
            ok = true
        } finally {
            // the audio without its record goes again (a saved song keeps its earlier copy and record)
            if (!ok) store.discardAudio(file)
        }
        return Outcome.Saved
    }

    /** What is saved: the file's extension, its codec and the quality label. */
    private data class Kept(val ext: String, val format: String, val quality: String)

    private fun keptOf(asIs: Boolean, format: String, info: JsonObject, t: Track): Kept {
        if (!asIs) {
            // the phone has no decoder for the file: lossless FLAC the server made
            return Kept("flac", "flac", OfflineIndex.CONVERTED_FLAC)
        }
        val ext = format.filter { it in 'a'..'z' || it in '0'..'9' }.ifEmpty { "audio" }
        val codec = OfflineIndex.codecOf(info.str("codec"), ext)
        val kbps = info.dbl("bitrate").takeIf { it > 0 } ?: t.bitrate.toDouble()
        val quality = OfflineIndex.qualityLabel(codec, info.bool("lossless"), info.int("bit_depth"), info.dbl("sample_rate"), kbps)
        return Kept(ext, codec, quality)
    }

    private fun mimeOf(ext: String, served: String): String {
        val byExt = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)
        return when {
            !byExt.isNullOrEmpty() -> byExt
            served.startsWith("audio/") -> served
            ext == "flac" -> "audio/flac"
            else -> "audio/mpeg"
        }
    }

    private fun info(server: String, id: Long): JsonObject {
        val req = Request.Builder()
            .url("$server/api/library/tracks/$id/info")
            .header(DECODERS_HEADER, OfflineStore.DECODERS)
            .header("Accept", "application/json")
            .build()
        infoHttp.newCall(req).execute().use { res ->
            if (!res.isSuccessful) throw Fail(OfflineIndex.statusWords(res.code))
            return try { EncoreJson.parseToJsonElement(res.body.string()) as? JsonObject } catch (_: Exception) { null }
                ?: throw Fail("Unexpected reply from the server")
        }
    }

    /** The album's cover at 600 px, once per album and server: its `art` ("" when there is none). */
    private fun saveCover(server: String, albumId: String, albumDir: String): String {
        val (art, f) = store.coverTarget(server, albumId, albumDir) ?: return ""
        if (f.isFile && f.length() > 0) return art
        return try {
            val req = Request.Builder()
                .url("$server/api/library/albums/${EncoreApi.enc(albumId)}/cover?s=600")
                .header("Accept", "image/jpeg")
                .build()
            infoHttp.newCall(req).execute().use { res ->
                val type = res.header("content-type") ?: ""
                if (!res.isSuccessful || !type.startsWith("image/")) return ""
                f.parentFile?.mkdirs()
                val tmp = File(f.parentFile, f.name + ".part")
                tmp.outputStream().use { out -> res.body.byteStream().use { it.copyTo(out) } }
                if (tmp.length() <= 0L || !tmp.renameTo(f)) { tmp.delete(); return "" }
            }
            art
        } catch (_: Exception) { "" }
    }

    /** The lyrics, when the server has words for the song (it looks them up when the song has none). */
    private fun saveLyrics(server: String, id: Long): Boolean = try {
        val req = Request.Builder().url("$server/api/library/tracks/$id/lyrics").header("Accept", "application/json").build()
        infoHttp.newCall(req).execute().use { res ->
            if (!res.isSuccessful) return false
            val text = res.body.string()
            val o = EncoreJson.parseToJsonElement(text) as? JsonObject ?: return false
            if (o.str("lyrics").isBlank()) return false
            val f = store.lyricsFile(server, id)
            f.parentFile?.mkdirs()
            f.writeText(text)
            true
        }
    } catch (_: Exception) { false }

    private fun reason(e: Exception): String {
        val m = e.message ?: ""
        return when {
            e is Fail -> m
            m.contains("ENOSPC") || m.contains("No space left", ignoreCase = true) -> NO_ROOM
            e is InterruptedIOException -> "The download stalled"
            e is IOException && (m.contains("unexpected end of stream", ignoreCase = true) || m.contains("reset", ignoreCase = true)) -> "The download broke off"
            e is IOException -> OfflineIndex.statusWords(0)
            e is SecurityException -> NO_PERMISSION
            else -> m.ifEmpty { "Something went wrong" }
        }
    }

    // ---- progress and summaries ------------------------------------------------------------------------------

    private var lastNotify = 0L

    private fun addBytes(n: Long) {
        synchronized(lock) { bytes += n }
        val now = System.currentTimeMillis()
        if (now - lastNotify >= 500L) notifyProgress()
    }

    private fun notifyProgress() {
        val p = synchronized(lock) {
            if (total == 0) null else Progress(done, total, bytes, current)
        }
        _progress.value = p
        lastNotify = System.currentTimeMillis()
        if (p == null) return
        val text = OfflineIndex.progressText(p.done, p.total, p.current)
        post(NOTIFY_PROGRESS, builder()
            .setContentTitle(text)
            .setProgress(p.total, p.done, false)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build())
    }

    private fun summary(b: Batch) {
        val (text, kind) = if (b.again) {
            if (b.failed.isNotEmpty()) b.failed.first() to "err" else OfflineIndex.againText(b.total, b.stillConverted)
        } else {
            OfflineIndex.summaryText(b.saved, b.skipped, b.failed.size, b.failed.firstOrNull())
        }
        Toasts.show(text, kind)
        post(NOTIFY_DONE, builder().setContentTitle(text).setAutoCancel(true).build())
    }

    private fun builder(): Notification.Builder {
        val nm = nm
        if (nm != null && nm.getNotificationChannel(CHANNEL) == null) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, "Downloads", NotificationManager.IMPORTANCE_LOW))
        }
        val open = Intent(app, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pi = PendingIntent.getActivity(app, 3, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return Notification.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_download)
            .setContentIntent(pi)
            .setShowWhen(false)
    }

    /** Without the notification permission (Android 13+) nothing shows; the toasts still say how it went. */
    private fun post(id: Int, n: Notification) {
        try { nm?.notify(id, n) } catch (_: SecurityException) {}
    }

    companion object {
        /** Songs fetched at the same time. */
        private const val AT_ONCE = 2
        /** No data for this long: the download stops. */
        private const val STALL_SECONDS = 60L
        /** Room kept free beside a download. */
        private const val MIN_FREE = 50L * 1024 * 1024
        private const val DECODERS_HEADER = "X-Encore-Decoders"
        private const val CHANNEL = "downloads"
        private const val NOTIFY_PROGRESS = 7301
        private const val NOTIFY_DONE = 7302

        /** the web's words (offline/store.rs QUOTA_FULL) */
        const val NO_ROOM = "There is no room left on this device for downloads"
        const val NO_PERMISSION = "Encore needs access to the phone's storage to keep downloads in Music/Encore"
    }
}
