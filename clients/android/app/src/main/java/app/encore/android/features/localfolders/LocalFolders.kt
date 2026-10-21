package app.encore.android.features.localfolders

import android.content.Context
import android.content.Intent
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.os.Build
import android.provider.DocumentsContract
import app.encore.android.core.EncoreJson
import app.encore.android.core.obj
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File
import java.util.Locale

/** One song in a folder the listener picked on this phone. */
@Serializable
data class LocalSong(
    /** the file's content:// URI (a document in a picked tree) */
    val uri: String,
    val name: String = "",
    val size: Long = 0,
    val modified: Long = 0,
    val title: String = "",
    val artist: String = "",
    val album: String = "",
    val albumArtist: String = "",
    val trackNo: Int = 0,
    val disc: Int = 0,
    val year: String = "",
    val durationSec: Double = 0.0,
    /** codec, lower case: flac, mp3, aac … */
    val format: String = "",
    val bitrate: Int = 0,
    val sampleRate: Int = 0,
    val bitDepth: Int = 0,
) {
    /** A stable id below zero, so the song never meets a library id (the player's lists key by id). */
    val localId: Long get() = -((uri.hashCode().toLong() and 0x7fffffffL) + 1L)

    /** The song as a Track for the shared rows and the player (no library id: [localId]). */
    fun asTrack(): Track = Track(
        id = localId,
        title = title.ifEmpty { name.substringBeforeLast('.') },
        artist = artist,
        album = album,
        albumArtist = albumArtist.ifEmpty { artist },
        albumId = "",
        trackNo = trackNo,
        disc = disc,
        year = year,
        durationSec = durationSec,
        format = format,
        bitrate = bitrate,
        sampleRate = sampleRate,
        bitDepth = bitDepth,
    )
}

@Serializable
private data class Index(
    val folders: List<String> = emptyList(),
    val songs: List<LocalSong> = emptyList(),
    /** per server: file URI → the library song's row (as `/api/library/tracks` sends it) */
    val matches: Map<String, Map<String, JsonObject>> = emptyMap(),
    /** per server: the scan its answers belong to */
    val matchedAt: Map<String, Long> = emptyMap(),
    val scan: Long = 0,
)

/**
 * Music in the listener's own folders on this phone (the desktop app's "Your folders", desktop/local.rs): folders
 * are picked with the system folder picker (a persisted tree permission, so no storage permission is needed),
 * scanned here, their tags read with MediaMetadataRetriever, and only the tags sent to the server
 * (`POST /api/library/match-local`) to learn which songs the library already has. Those play from the phone's
 * file wherever they appear ([uriFor], used by the StreamResolver); the rest are listed under Library → On this
 * device. Nothing is uploaded.
 *
 * Kept in `files/local/index.json`. File work is blocking: call [scan] and [match] off the main thread.
 */
class LocalFolders private constructor(private val app: Context) {
    private val file = File(File(app.filesDir, "local"), "index.json")
    private val lock = Any()
    private var index: Index = read()
    private val _state = MutableStateFlow(index)
    private val _scanning = MutableStateFlow(false)
    val scanning: StateFlow<Boolean> = _scanning.asStateFlow()
    /** file URI → library id, for the signed-in server (built when first asked) */
    @Volatile private var byId: Pair<String, Map<Long, String>>? = null

    val folders: List<String> get() = _state.value.folders
    val songs: List<LocalSong> get() = _state.value.songs

    /** Changes with every scan and match (screens collect it). */
    val changes: StateFlow<*> get() = _state.asStateFlow()

    fun matches(server: String): Map<String, JsonObject> = _state.value.matches[server] ?: emptyMap()

    /** The library song's file in a local folder, for this server; null when no folder has it. */
    fun uriFor(server: String, id: Long): String? {
        if (server.isEmpty()) return null
        val cached = byId?.takeIf { it.first == server }
        val map = cached?.second ?: matches(server).mapNotNull { (uri, row) -> Track.from(row)?.let { it.id to uri } }.toMap()
            .also { byId = server to it }
        return map[id]
    }

    /** Add a folder the picker returned (its permission is kept across restarts). */
    fun addFolder(tree: Uri): Boolean {
        try {
            app.contentResolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (_: SecurityException) {
            return false
        }
        update { it.copy(folders = (it.folders + tree.toString()).distinct()) }
        return true
    }

    fun removeFolder(tree: String) {
        try {
            app.contentResolver.releasePersistableUriPermission(Uri.parse(tree), Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (_: SecurityException) {
        }
        update { it.copy(folders = it.folders - tree) }
    }

    /** A folder's name for the list ("Music/Albums" for primary:Music/Albums). */
    fun label(tree: String): String = try {
        DocumentsContract.getTreeDocumentId(Uri.parse(tree)).substringAfter(':').ifEmpty { tree }
    } catch (_: Exception) {
        tree
    }

    /** Scan every folder again; a file's tags are read again only when its size or time changed. Blocking. */
    fun scan() {
        if (_scanning.value) return
        _scanning.value = true
        try {
            val before = songs.associateBy { it.uri }
            val found = ArrayList<LocalSong>()
            for (f in folders) {
                val tree = Uri.parse(f)
                val root = try { DocumentsContract.getTreeDocumentId(tree) } catch (_: Exception) { continue }
                walk(tree, root, before, found, 0)
            }
            val sorted = found.distinctBy { it.uri }.sortedWith(compareBy({ it.albumArtist.ifEmpty { it.artist }.lowercase(Locale.ROOT) }, { it.album.lowercase(Locale.ROOT) }, { it.disc }, { it.trackNo }, { it.title.lowercase(Locale.ROOT) }))
            update { cur ->
                if (cur.songs == sorted) cur else {
                    val keep = sorted.map { it.uri }.toSet()
                    cur.copy(songs = sorted, scan = cur.scan + 1, matches = cur.matches.mapValues { (_, m) -> m.filterKeys { it in keep } })
                }
            }
        } finally {
            _scanning.value = false
        }
    }

    private fun walk(tree: Uri, docId: String, before: Map<String, LocalSong>, out: MutableList<LocalSong>, depth: Int) {
        if (depth > 32) return
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, docId)
        val cols = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
        )
        val c = try { app.contentResolver.query(children, cols, null, null, null) } catch (_: Exception) { null } ?: return
        c.use {
            while (it.moveToNext()) {
                val id = it.getString(0) ?: continue
                val name = it.getString(1) ?: ""
                val mime = it.getString(2) ?: ""
                if (name.startsWith(".")) continue
                if (mime == DocumentsContract.Document.MIME_TYPE_DIR) {
                    walk(tree, id, before, out, depth + 1)
                    continue
                }
                if (!isAudio(name, mime)) continue
                val size = if (it.isNull(3)) 0L else it.getLong(3)
                val modified = if (it.isNull(4)) 0L else it.getLong(4)
                val uri = DocumentsContract.buildDocumentUriUsingTree(tree, id).toString()
                val old = before[uri]
                out.add(if (old != null && old.size == size && old.modified == modified) old else readTags(uri, name, size, modified))
            }
        }
    }

    private fun readTags(uri: String, name: String, size: Long, modified: Long): LocalSong {
        val ext = name.substringAfterLast('.', "").lowercase(Locale.ROOT)
        val base = LocalSong(uri = uri, name = name, size = size, modified = modified, title = name.substringBeforeLast('.'), format = codecOf(ext))
        val r = MediaMetadataRetriever()
        return try {
            r.setDataSource(app, Uri.parse(uri))
            fun m(k: Int) = r.extractMetadata(k)?.trim().orEmpty()
            val artist = m(MediaMetadataRetriever.METADATA_KEY_ARTIST)
            val albumArtist = m(MediaMetadataRetriever.METADATA_KEY_ALBUMARTIST)
            base.copy(
                title = m(MediaMetadataRetriever.METADATA_KEY_TITLE).ifEmpty { base.title },
                artist = artist.ifEmpty { albumArtist },
                album = m(MediaMetadataRetriever.METADATA_KEY_ALBUM),
                albumArtist = albumArtist,
                trackNo = leadingNumber(m(MediaMetadataRetriever.METADATA_KEY_CD_TRACK_NUMBER)),
                disc = leadingNumber(m(MediaMetadataRetriever.METADATA_KEY_DISC_NUMBER)),
                year = m(MediaMetadataRetriever.METADATA_KEY_YEAR).take(4),
                durationSec = (m(MediaMetadataRetriever.METADATA_KEY_DURATION).toLongOrNull() ?: 0L) / 1000.0,
                bitrate = ((m(MediaMetadataRetriever.METADATA_KEY_BITRATE).toLongOrNull() ?: 0L) / 1000L).toInt(),
                sampleRate = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) m(MediaMetadataRetriever.METADATA_KEY_SAMPLERATE).toIntOrNull() ?: 0 else 0,
                bitDepth = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) m(MediaMetadataRetriever.METADATA_KEY_BITS_PER_SAMPLE).toIntOrNull() ?: 0 else 0,
            )
        } catch (_: Exception) {
            base
        } finally {
            try { r.release() } catch (_: Exception) {}
        }
    }

    /** Whether the server's answers are older than the songs. */
    fun needsMatch(server: String): Boolean {
        val s = _state.value
        return server.isNotEmpty() && s.songs.isNotEmpty() && s.matchedAt[server] != s.scan
    }

    /** Ask the server which songs its library has (only the tags go), and keep the answers. Blocking. */
    fun match(api: EncoreApi): Int {
        val server = api.base
        if (server.isEmpty()) return 0
        val (list, scan) = _state.value.let { it.songs to it.scan }
        val found = HashMap<String, JsonObject>()
        for (part in list.chunked(MATCH_CHUNK)) {
            val body = buildJsonObject {
                put("songs", buildJsonArray {
                    for (s in part) add(buildJsonObject {
                        put("key", s.uri)
                        put("title", s.title)
                        put("artist", s.artist)
                        if (s.durationSec > 0) put("duration", s.durationSec)
                    })
                })
            }
            val answer = (api.post("/api/library/match-local", body) as? JsonObject)?.obj("matches") ?: continue
            for ((k, v) in answer) (v as? JsonObject)?.let { found[k] = it }
        }
        update { it.copy(matches = it.matches + (server to found), matchedAt = it.matchedAt + (server to scan)) }
        return found.size
    }

    private fun update(f: (Index) -> Index) {
        synchronized(lock) {
            index = f(index)
            byId = null
            write(index)
            _state.value = index
        }
    }

    private fun read(): Index = try {
        if (file.isFile) EncoreJson.decodeFromString(Index.serializer(), file.readText()) else Index()
    } catch (_: Exception) {
        Index()
    }

    private fun write(i: Index) {
        try {
            file.parentFile?.mkdirs()
            val tmp = File(file.parentFile, "index.json.part")
            tmp.writeText(EncoreJson.encodeToString(Index.serializer(), i))
            tmp.renameTo(file)
        } catch (_: Exception) {
        }
    }

    companion object {
        private const val MATCH_CHUNK = 500
        private val AUDIO = setOf("flac", "mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff", "wv", "ape", "mka", "wma")

        @Volatile private var instance: LocalFolders? = null

        fun of(context: Context): LocalFolders =
            instance ?: synchronized(this) { instance ?: LocalFolders(context.applicationContext).also { instance = it } }

        fun isAudio(name: String, mime: String): Boolean =
            name.substringAfterLast('.', "").lowercase(Locale.ROOT) in AUDIO || (mime.startsWith("audio/") && !mime.endsWith("mpegurl"))

        /** "3/12" → 3. */
        fun leadingNumber(s: String): Int = s.trim().takeWhile { it.isDigit() }.toIntOrNull() ?: 0

        fun codecOf(ext: String): String = when (ext) {
            "m4a", "aac" -> "aac"
            "ogg", "oga" -> "vorbis"
            "aif", "aiff" -> "aiff"
            else -> ext
        }
    }
}
