package app.encore.android.features.offline

import android.Manifest
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.os.StatFs
import android.provider.MediaStore
import androidx.annotation.RequiresApi
import app.encore.android.CrashLog
import app.encore.android.core.arr
import app.encore.android.core.parseJsonObject
import app.encore.android.core.str
import app.encore.android.data.api.ArtistCredit
import app.encore.android.data.api.Track
import app.encore.android.graph
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.util.Locale
import java.util.concurrent.ConcurrentHashMap

/**
 * Songs saved on this phone (0.30, offline-manifest.md "Phone"): the audio in `Music/Encore/<Artist>/<Album>/`
 * on shared storage, `NN - Title.ext`, the file exactly as the server sends it (tags included), so other music
 * players see it and it stays when the app is removed.
 *
 * - Android 10 and later: the audio goes through MediaStore (`IS_PENDING` while it is written). Android lets an
 *   app put only audio in `Music/`, so the index (`.encore-index.json`), the album covers and the lyrics are in
 *   the app's own storage (`files/offline/`); the index's `art` is relative to that folder.
 * - Android 8 and 9: plain files with the storage permission (asked at the first download): the index is
 *   `Music/Encore/.encore-index.json` (hidden from players) and each album's cover is `cover.jpg` in its folder,
 *   `art` relative to `Music/Encore/`. A copy of the index is kept in the app's storage too, so the list still
 *   reads when the permission is taken away; after a reinstall the index in `Music/Encore` is picked up again.
 *
 * The index holds every server's songs; lists and playback use the signed-in server's ([songs], [record]).
 * Thread-safe; file work is blocking (call it off the main thread, except [all]).
 */
class OfflineStore private constructor(private val app: Context) {
    private val dir = File(app.filesDir, "offline")
    private val privateIndex = File(dir, INDEX_NAME)
    private val lock = Any()
    @Volatile private var loaded = false

    private val _all = MutableStateFlow<List<OfflineRecord>>(emptyList())

    /** Every saved song of every server, oldest first. */
    val all: StateFlow<List<OfflineRecord>> = _all.asStateFlow()

    /** Android 10+: the content URI of each saved file, by its `file`. */
    private val uris = ConcurrentHashMap<String, Uri>()

    /** Plays and likes waiting for the server. */
    val outbox = OutboxFile(File(dir, "outbox.json"))

    private val likesFile = File(dir, "likes.json")

    /** This phone's own download settings (not the account's). */
    private val settings by lazy { app.getSharedPreferences(SETTINGS_NAME, Context.MODE_PRIVATE) }

    private val _preferLocal by lazy { MutableStateFlow(settings.getBoolean(PREFER_LOCAL, true)) }

    /**
     * "Play downloaded songs from this device" (on unless turned off, the web's `encore.offline.preferLocal`): off, a
     * downloaded song streams from the server while it can be reached (quality steps, server processing); without
     * a server to ask its copy plays anyway.
     */
    val preferLocal: StateFlow<Boolean> get() = _preferLocal

    fun setPreferLocal(on: Boolean) {
        _preferLocal.value = on
        settings.edit().putBoolean(PREFER_LOCAL, on).apply()
    }

    /** The downloads. */
    val worker: OfflineWorker by lazy { OfflineWorker(app, this) }

    init {
        app.graph.scope.launch(Dispatchers.IO) {
            ensureLoaded()
            try { prune() } catch (e: Exception) { CrashLog.log("offline: check failed: ${e.message}") }
            try { sweepLeftovers() } catch (_: Exception) {}
        }
    }

    // ---- where things are ---------------------------------------------------------------------------------

    /** Android 8–9: plain files in Music/Encore (Android 10 and later: MediaStore, and the rest in the app's storage). */
    val legacy: Boolean get() = Build.VERSION.SDK_INT < Build.VERSION_CODES.Q

    @Suppress("DEPRECATION")
    private fun musicDir(): File = File(Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_MUSIC), FOLDER)

    private fun sharedIndex(): File = File(musicDir(), INDEX_NAME)

    /** Android 8–9: the storage permission is granted (always true from Android 10: MediaStore needs none for our own files). */
    fun canWrite(): Boolean = !legacy ||
        app.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE) == PackageManager.PERMISSION_GRANTED

    private fun mounted(): Boolean = Environment.getExternalStorageState() == Environment.MEDIA_MOUNTED

    // ---- the index ----------------------------------------------------------------------------------------

    /** Reads the index once (blocking). */
    fun ensureLoaded() {
        if (loaded) return
        synchronized(lock) {
            if (loaded) return
            var list = readIndex(privateIndex)
            if (list == null && legacy && canWrite() && mounted()) {
                // a reinstall on Android 8–9: the index in Music/Encore, for the files still there
                list = readIndex(sharedIndex())?.filter { r -> OfflineIndex.safeRelative(r.file)?.let { File(musicDir(), it).isFile } == true }
                if (list != null) CrashLog.log("offline: picked up ${list.size} songs from Music/Encore")
            }
            _all.value = list ?: emptyList()
            loaded = true
            if (list != null && !privateIndex.isFile) save(list)
        }
    }

    private fun readIndex(f: File): List<OfflineRecord>? =
        try { if (f.isFile) OfflineIndex.parse(f.readText()) else null } catch (_: Exception) { null }

    /** Writes the index (call holding [lock]). */
    private fun save(list: List<OfflineRecord>) {
        _all.value = list
        val text = OfflineIndex.write(list)
        try { writeAtomic(privateIndex, text) } catch (e: Exception) { CrashLog.log("offline: index not written: ${e.message}") }
        if (legacy && canWrite() && mounted()) {
            try { writeAtomic(sharedIndex(), text) } catch (_: Exception) { /* the app's copy is enough */ }
        }
    }

    private fun writeAtomic(f: File, text: String) {
        f.parentFile?.mkdirs()
        val tmp = File(f.parentFile, f.name + ".tmp")
        tmp.writeText(text)
        if (!tmp.renameTo(f)) {
            f.delete()
            if (!tmp.renameTo(f)) throw IOException("rename failed")
        }
    }

    // ---- the signed-in server's songs -----------------------------------------------------------------------

    /** The signed-in server, as records name it ("" when signed out). */
    fun server(): String = app.graph.store.current?.server?.trim()?.trimEnd('/') ?: ""

    /** The account outbox entries belong to: `<server>|<username>` ("" when signed out). */
    fun owner(): String {
        val s = app.graph.store.current ?: return ""
        return norm(s.server) + "|" + s.username
    }

    private fun sameServer(a: String, b: String): Boolean = b.isNotEmpty() && norm(a) == norm(b)

    /** The signed-in server's saved songs, oldest first. */
    fun songs(list: List<OfflineRecord> = all.value): List<OfflineRecord> {
        val s = server()
        return list.filter { sameServer(it.sourceServer, s) }
    }

    /** [songs] once the index is read (blocking: for the playback service and Android Auto, not the UI). */
    fun songsNow(): List<OfflineRecord> {
        ensureLoaded()
        return songs(_all.value)
    }

    /** The saved copy of a song of the signed-in server. */
    fun record(id: Long): OfflineRecord? {
        ensureLoaded()
        val s = server()
        return _all.value.lastOrNull { it.id == id && sameServer(it.sourceServer, s) }
    }

    fun has(id: Long): Boolean = record(id) != null

    /** Where the song plays from on this phone: a content:// (Android 10+) or file:// URI; null when it has no copy here. */
    fun localUri(id: Long): String? {
        val r = record(id) ?: return null
        return try { audioUri(r)?.toString() } catch (_: Exception) { null }
    }

    private fun audioUri(r: OfflineRecord): Uri? {
        val rel = OfflineIndex.safeRelative(r.file) ?: return null
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            val f = File(musicDir(), rel)
            return if (f.isFile) Uri.fromFile(f) else null
        }
        uris[rel]?.let { return it }
        return findAudio(rel)?.also { uris[rel] = it }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private fun audioCollection(): Uri = MediaStore.Audio.Media.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)

    /** `Music/Encore/<dir>/` as MediaStore's RELATIVE_PATH writes it. */
    private fun relativeDir(rel: String): String {
        val d = rel.substringBeforeLast('/', "")
        return if (d.isEmpty()) ROOT_REL else "$ROOT_REL$d/"
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private fun findAudio(rel: String): Uri? {
        val sel = "${MediaStore.MediaColumns.RELATIVE_PATH}=? AND ${MediaStore.MediaColumns.DISPLAY_NAME}=?"
        app.contentResolver.query(audioCollection(), arrayOf(MediaStore.MediaColumns._ID), sel, arrayOf(relativeDir(rel), rel.substringAfterLast('/')), null)?.use { c ->
            if (c.moveToFirst()) return ContentUris.withAppendedId(audioCollection(), c.getLong(0))
        }
        return null
    }

    /** The cover saved with an album of the signed-in server (the car's covers and the lists when offline). */
    fun coverFile(albumId: String): File? {
        if (albumId.isEmpty()) return null
        val s = server()
        val r = _all.value.lastOrNull { it.albumId == albumId && it.cover && it.art.isNotEmpty() && sameServer(it.sourceServer, s) } ?: return null
        return artFile(r.art)?.takeIf { it.isFile }
    }

    private fun artFile(art: String): File? {
        val rel = OfflineIndex.safeRelative(art) ?: return null
        return if (legacy) File(musicDir(), rel) else File(dir, rel)
    }

    /** The lyrics saved with a song of the signed-in server (the server's `/lyrics` answer). */
    fun savedLyrics(id: Long): JsonObject? {
        val r = record(id) ?: return null
        if (!r.lyrics) return null
        return try { parseJsonObject(lyricsFile(r.sourceServer, r.id).readText()) } catch (_: Exception) { null }
    }

    // ---- writing a download (OfflineWorker) --------------------------------------------------------------------

    /**
     * Writes a song's audio as `Music/Encore/<albumDir>/<name>` (a name that is taken gets " (2)", or MediaStore's
     * " (1)"): returns its `file` and its size. Whatever was written goes again when it fails.
     */
    fun writeAudio(albumDir: String, name: String, mime: String, input: InputStream, onBytes: (Long) -> Unit): Pair<String, Long> {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) return writeAudioQ(albumDir, name, mime, input, onBytes)
        if (!mounted()) throw IOException(NO_STORAGE)
        val d = File(musicDir(), albumDir)
        if (!d.isDirectory && !d.mkdirs()) throw IOException("Couldn't make the folder Music/Encore/$albumDir")
        var actual = name
        var n = 2
        while (File(d, actual).exists() || File(d, "$actual.part").exists()) actual = OfflineIndex.numbered(name, n++)
        val part = File(d, "$actual.part")
        var ok = false
        try {
            val bytes = part.outputStream().use { copy(input, it, onBytes) }
            if (bytes <= 0L) throw IOException("The server sent an empty file")
            val f = File(d, actual)
            if (!part.renameTo(f)) throw IOException("Couldn't save the file")
            ok = true
            // other players see it at once
            MediaScannerConnection.scanFile(app, arrayOf(f.path), arrayOf(mime), null)
            return "$albumDir/$actual" to bytes
        } finally {
            if (!ok) part.delete()
        }
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private fun writeAudioQ(albumDir: String, name: String, mime: String, input: InputStream, onBytes: (Long) -> Unit): Pair<String, Long> {
        val resolver = app.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, mime)
            put(MediaStore.MediaColumns.RELATIVE_PATH, "$ROOT_REL$albumDir/")
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(audioCollection(), values) ?: throw IOException(NO_STORAGE)
        var ok = false
        try {
            val bytes = resolver.openOutputStream(uri, "w")?.use { copy(input, it, onBytes) } ?: throw IOException(NO_STORAGE)
            if (bytes <= 0L) throw IOException("The server sent an empty file")
            resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
            // the name it got: MediaStore numbers a name that is taken ("01 - Song (1).flac")
            var rel = "$albumDir/$name"
            resolver.query(uri, arrayOf(MediaStore.MediaColumns.RELATIVE_PATH, MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    val rp = c.getString(0) ?: ""
                    val dn = c.getString(1) ?: ""
                    if (rp.startsWith(ROOT_REL) && dn.isNotEmpty()) {
                        val d = rp.removePrefix(ROOT_REL).trimEnd('/')
                        rel = if (d.isEmpty()) dn else "$d/$dn"
                    }
                }
            }
            uris[rel] = uri
            ok = true
            return rel to bytes
        } finally {
            if (!ok) try { resolver.delete(uri, null, null) } catch (_: Exception) {}
        }
    }

    private fun copy(input: InputStream, out: OutputStream, onBytes: (Long) -> Unit): Long {
        val buf = ByteArray(64 * 1024)
        var total = 0L
        while (true) {
            val n = input.read(buf)
            if (n < 0) break
            if (n == 0) continue
            out.write(buf, 0, n)
            total += n
            onBytes(n.toLong())
        }
        out.flush()
        return total
    }

    /**
     * Where an album's cover goes: (`art`, file). Android 8–9: `cover.jpg` in the album's folder; Android 10+: the
     * app's `covers/` folder (Android keeps only audio in Music/). Null for a song without an album id.
     */
    fun coverTarget(server: String, albumId: String, albumDir: String): Pair<String, File>? {
        val id = OfflineIndex.cleanId(albumId)
        if (id.isEmpty()) return null
        if (legacy) {
            val art = "$albumDir/cover.jpg"
            return art to File(musicDir(), art)
        }
        val art = "covers/${tag(server)}-$id.jpg"
        return art to File(dir, art)
    }

    /** Where a song's lyrics go (the app's storage on every version). */
    fun lyricsFile(server: String, id: Long): File = File(dir, "lyrics/${tag(server)}-$id.json")

    /** Adds a saved song (replacing its earlier copy, whose file goes when it was another one). */
    fun put(r: OfflineRecord) {
        synchronized(lock) {
            ensureLoaded()
            val key = norm(r.sourceServer) to r.id
            val old = _all.value.lastOrNull { (norm(it.sourceServer) to it.id) == key }
            save(_all.value.filter { (norm(it.sourceServer) to it.id) != key } + r)
            if (old != null && old.file != r.file) deleteAudio(old)
        }
    }

    /** A file written for a song whose record could not be saved: it goes, unless a saved song uses it. */
    fun discardAudio(file: String) {
        synchronized(lock) {
            if (_all.value.any { it.file == file }) return
            try { deleteAudio(OfflineRecord(id = 0L, title = "", artist = "", album = "", ext = "", file = file)) } catch (_: Exception) {}
        }
    }

    // ---- removing ------------------------------------------------------------------------------------------

    /**
     * Deletes these downloads (their audio, lyrics, and the covers no other saved song uses); how many went. The
     * index changes first (the lists update at once), then the files go.
     */
    fun remove(records: Collection<OfflineRecord>): Int {
        val gone: List<OfflineRecord>
        synchronized(lock) {
            ensureLoaded()
            val keys = records.map { norm(it.sourceServer) to it.id }.toHashSet()
            val parts = _all.value.partition { (norm(it.sourceServer) to it.id) in keys }
            gone = parts.first
            if (gone.isEmpty()) return 0
            save(parts.second)
        }
        for (r in gone) {
            try { deleteAudio(r) } catch (e: Exception) { CrashLog.log("offline: delete failed: ${e.message}") }
            lyricsFile(r.sourceServer, r.id).delete()
        }
        val now = _all.value
        for (art in gone.map { it.art }.filter { it.isNotEmpty() }.distinct()) {
            if (now.none { it.art == art }) artFile(art)?.delete()
        }
        if (legacy) for (r in gone) OfflineIndex.safeRelative(r.file)?.let { pruneEmptyDirs(File(musicDir(), it).parentFile) }
        return gone.size
    }

    /** Every download of every server. */
    fun removeAll(): Int = remove(all.value)

    private fun deleteAudio(r: OfflineRecord) {
        val rel = OfflineIndex.safeRelative(r.file) ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            val uri = uris.remove(rel) ?: findAudio(rel) ?: return
            app.contentResolver.delete(uri, null, null)
            return
        }
        val f = File(musicDir(), rel)
        if (f.isFile && f.delete()) MediaScannerConnection.scanFile(app, arrayOf(f.path), null, null)
    }

    /** Android 8–9: an album or artist folder left empty goes (never Music/Encore itself). */
    private fun pruneEmptyDirs(start: File?) {
        val root = musicDir().canonicalFile
        var d = start?.canonicalFile ?: return
        while (d.path.startsWith(root.path + File.separator) && d != root) {
            val names = d.list() ?: return
            if (names.any { it != "cover.jpg" }) return
            File(d, "cover.jpg").delete()
            if (!d.delete()) return
            d = d.parentFile ?: return
        }
    }

    // ---- housekeeping --------------------------------------------------------------------------------------

    /**
     * Songs whose file is gone (deleted in a file manager, another app) leave the index; sizes the index lacks
     * (older shapes) are read from the files. Only when the storage is there and could be read.
     */
    private fun prune() {
        if (!mounted()) return
        synchronized(lock) {
            val list = _all.value
            if (list.isEmpty()) return
            val keep: List<OfflineRecord> = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                // nothing of ours visible at all (e.g. files saved before an update to Android 10, which this app
                // no longer owns): leave the index alone rather than forget every song
                val found = ourAudio()?.takeIf { it.isNotEmpty() } ?: return
                list.filter { r -> OfflineIndex.safeRelative(r.file)?.let { it in found } == true }
            } else {
                if (!canWrite()) return
                list.mapNotNull { r ->
                    val f = OfflineIndex.safeRelative(r.file)?.let { File(musicDir(), it) }
                    when {
                        f == null || !f.isFile -> null
                        r.bytes <= 0L -> r.copy(bytes = f.length())
                        else -> r
                    }
                }
            }
            if (keep == list) return
            val dropped = list.filter { r -> keep.none { it.id == r.id && it.sourceServer == r.sourceServer } }
            for (r in dropped) lyricsFile(r.sourceServer, r.id).delete()
            for (art in dropped.map { it.art }.filter { it.isNotEmpty() }.distinct()) if (keep.none { it.art == art }) artFile(art)?.delete()
            if (dropped.isNotEmpty()) CrashLog.log("offline: ${dropped.size} saved songs were deleted outside the app")
            save(keep)
        }
    }

    /** Android 10+: our audio under Music/Encore as `file` paths (and their URIs remembered); null when the query fails. */
    @RequiresApi(Build.VERSION_CODES.Q)
    private fun ourAudio(): Set<String>? {
        val out = HashSet<String>()
        val proj = arrayOf(MediaStore.MediaColumns._ID, MediaStore.MediaColumns.RELATIVE_PATH, MediaStore.MediaColumns.DISPLAY_NAME)
        val c = try {
            app.contentResolver.query(audioCollection(), proj, "${MediaStore.MediaColumns.RELATIVE_PATH} LIKE ?", arrayOf("$ROOT_REL%"), null)
        } catch (_: Exception) { null } ?: return null
        c.use {
            while (it.moveToNext()) {
                val rp = it.getString(1) ?: continue
                val dn = it.getString(2) ?: continue
                if (!rp.startsWith(ROOT_REL)) continue
                val d = rp.removePrefix(ROOT_REL).trimEnd('/')
                val rel = if (d.isEmpty()) dn else "$d/$dn"
                out.add(rel)
                uris[rel] = ContentUris.withAppendedId(audioCollection(), it.getLong(0))
            }
        }
        return out
    }

    /**
     * What downloads stopped in the middle left behind (the app was closed): `.part` files (Android 8–9), and
     * our pending MediaStore rows (Android 11+; Android 10 drops them itself after a week). Only those older
     * than an hour, so a download that just started is never touched.
     */
    private fun sweepLeftovers() {
        val hourAgo = System.currentTimeMillis() - 3_600_000L
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val args = Bundle().apply {
                putInt(MediaStore.QUERY_ARG_MATCH_PENDING, MediaStore.MATCH_ONLY)
                putString(android.content.ContentResolver.QUERY_ARG_SQL_SELECTION,
                    "${MediaStore.MediaColumns.RELATIVE_PATH} LIKE ? AND ${MediaStore.MediaColumns.DATE_ADDED} < ?")
                putStringArray(android.content.ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS, arrayOf("$ROOT_REL%", (hourAgo / 1000L).toString()))
            }
            val ids = ArrayList<Long>()
            app.contentResolver.query(audioCollection(), arrayOf(MediaStore.MediaColumns._ID), args, null)?.use { c ->
                while (c.moveToNext()) ids.add(c.getLong(0))
            }
            for (id in ids) try { app.contentResolver.delete(ContentUris.withAppendedId(audioCollection(), id), null, null) } catch (_: Exception) {}
            if (ids.isNotEmpty()) CrashLog.log("offline: removed ${ids.size} unfinished downloads")
        } else if (legacy && canWrite() && mounted()) {
            musicDir().walkTopDown().maxDepth(3)
                .filter { it.isFile && it.name.endsWith(".part") && it.lastModified() < hourAgo }
                .forEach { it.delete() }
        }
    }

    // ---- space -----------------------------------------------------------------------------------------------

    /** The room the saved songs take (audio only, as the index counts it). */
    fun usedBytes(list: List<OfflineRecord> = all.value): Long = list.sumOf { it.bytes }

    /** Free room on the phone's shared storage; -1 when unknown. */
    fun freeBytes(): Long = try {
        @Suppress("DEPRECATION")
        StatFs(Environment.getExternalStorageDirectory().path).availableBytes
    } catch (_: Exception) { -1L }

    // ---- the hearts while offline ------------------------------------------------------------------------------

    /** The liked songs as the server listed them last, kept for the hearts while it can't be reached. */
    fun keepLikes(ids: Set<Long>) {
        val owner = owner()
        if (owner.isEmpty()) return
        try {
            dir.mkdirs()
            writeAtomic(likesFile, buildJsonObject {
                put("owner", owner)
                put("ids", buildJsonArray { for (id in ids.sorted()) add(JsonPrimitive(id)) })
            }.toString())
        } catch (_: Exception) {}
    }

    /** The liked songs this phone last knew for the signed-in account (empty when it never saw the list). */
    fun keptLikes(): Set<Long> = try {
        val o = parseJsonObject(likesFile.readText())
        if (o == null || o.str("owner") != owner()) emptySet()
        else o.arr("ids")?.mapNotNull { (it as? JsonPrimitive)?.content?.toDoubleOrNull()?.toLong() }?.toSet() ?: emptySet()
    } catch (_: Exception) { emptySet() }

    companion object {
        /** The folder under Music. */
        const val FOLDER = "Encore"
        /** `Music/Encore/` as MediaStore's RELATIVE_PATH (Environment.DIRECTORY_MUSIC is "Music"). */
        const val ROOT_REL = "Music/Encore/"
        /** The index's file name (hidden from music players). */
        const val INDEX_NAME = ".encore-index.json"
        /**
         * The codecs this phone plays as they are (ExoPlayer's own decoders), sent as `X-Encore-Decoders` with
         * `/info` and the download: the server sends the original for these and FLAC it makes for the rest.
         */
        const val DECODERS = "flac,mp3,aac,opus,vorbis,wav"

        const val NO_STORAGE = "This phone's storage can't be written to right now"

        private const val SETTINGS_NAME = "encore.offline"
        private const val PREFER_LOCAL = "preferLocal"

        @Volatile var instance: OfflineStore? = null
            private set

        fun of(context: Context): OfflineStore = instance ?: synchronized(this) {
            instance ?: OfflineStore(context.applicationContext).also { instance = it }
        }

        fun norm(s: String): String = s.trim().trimEnd('/').lowercase(Locale.ROOT)

        /** 8 hex digits for a server (a file name part). */
        fun tag(server: String): String = String.format(Locale.ROOT, "%08x", norm(server).hashCode())

        /** A saved song as the player's queue wants it. A converted copy leaves its format out (the server's file is not that codec). */
        fun trackOf(r: OfflineRecord): Track = Track(
            id = r.id,
            title = r.title.ifEmpty { "Unknown song" },
            artist = r.artist,
            album = r.album,
            albumArtist = r.artist,
            albumId = r.albumId,
            trackNo = r.track,
            disc = r.disc,
            year = "",
            durationSec = r.duration,
            format = if (r.converted || r.ext == "audio") "" else r.ext,
            bitrate = 0,
            artists = credits(r.artists),
        )

        private fun credits(list: List<JsonElement>): List<ArtistCredit> = list.mapNotNull { e ->
            when (e) {
                is JsonPrimitive -> e.content.takeIf { e.isString && it.isNotBlank() }?.let { ArtistCredit(it) }
                is JsonObject -> e.str("name").takeIf { it.isNotBlank() }?.let { ArtistCredit(it, e.str("mbid")) }
                else -> null
            }
        }

        /** The song's `artists` as the record keeps them (the server's list as it came). */
        fun artistsOf(t: Track): List<JsonElement> = t.artists.map { a ->
            buildJsonObject { put("name", a.name); if (a.mbid.isNotEmpty()) put("mbid", a.mbid) }
        }
    }
}
