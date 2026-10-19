package app.encore.android.features.files

import android.content.ContentResolver
import android.database.Cursor
import android.net.Uri
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import app.encore.android.AppGraph
import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.int
import app.encore.android.core.long
import app.encore.android.core.obj
import app.encore.android.core.objects
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.EncoreApi
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonObjectBuilder
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.MultipartBody
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okio.BufferedSink
import okio.source
import java.io.IOException
import java.util.concurrent.TimeUnit

// The admin file manager (0.33): the routes and JSON written down in docs/contracts/0.31-0.33-client-shapes.md, §2.4
// and Part 4.
// Everything is read leniently; a server before 0.33 answers 404 on /api/files/roots and the entries hide.

/** A place the file manager may touch (`GET /api/files/roots`). */
data class FileRoot(val id: String, val name: String, val path: String, val writable: Boolean, val freeBytes: Long) {
    val isTrash: Boolean get() = id == "trash"

    companion object {
        fun from(o: JsonObject): FileRoot? {
            val id = o.str("id")
            if (id.isEmpty()) return null
            return FileRoot(id, o.str("name").ifEmpty { id }, o.str("path"), if (o.containsKey("writable")) o.bool("writable") else true, o.long("free_bytes"))
        }
    }
}

/** What the library knows about an audio file. */
data class FileAudio(val format: String, val durationSec: Double, val title: String, val artist: String, val album: String, val trackId: Long)

/** One entry of a folder. */
data class FileEntry(
    val name: String,
    /** "dir" | "file" | "link" */
    val kind: String,
    val size: Long,
    /** seconds since 1970 */
    val mtime: Double,
    val audio: FileAudio?,
    val image: Boolean,
    /** a link's target as the server shows it */
    val target: String,
    /** a link to a folder can be opened (never written through) */
    val targetKind: String,
) {
    val isDir: Boolean get() = kind == "dir" || (kind == "link" && targetKind == "dir")
    val isAudio: Boolean get() = audio != null || name.substringAfterLast('.', "").lowercase() in AUDIO

    companion object {
        private val AUDIO = setOf("flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "aiff", "aif", "alac", "ape", "wv", "dsf", "dff", "wma", "mpc", "tta")

        fun from(o: JsonObject): FileEntry? {
            val name = o.str("name")
            if (name.isEmpty()) return null
            val a = o.obj("audio")
            return FileEntry(
                name = name,
                kind = o.str("kind").ifEmpty { "file" },
                size = o.long("size"),
                mtime = o.dbl("mtime"),
                audio = a?.let { FileAudio(it.str("format"), it.dbl("duration"), it.str("title"), it.str("artist"), it.str("album"), it.long("track_id")) },
                image = o.bool("image"),
                target = o.str("target"),
                targetKind = o.str("target_kind"),
            )
        }
    }
}

/** A page of a folder (500 entries at a time). */
data class FileListing(val entries: List<FileEntry>, val total: Int, val offset: Int)

/** Something in the trash, with where it came from. */
data class TrashItem(val id: String, val name: String, val root: String, val path: String, val at: Double, val by: String, val size: Long, val kind: String) {
    companion object {
        fun from(o: JsonObject): TrashItem? {
            val id = o.str("id")
            if (id.isEmpty()) return null
            return TrashItem(id, o.str("name").ifEmpty { o.str("path").substringAfterLast('/') }, o.str("root"), o.str("path"), o.dbl("at"), o.str("by"), o.long("size"), o.str("kind"))
        }
    }
}

/** A release an album folder could be tagged as. */
data class ImportRelease(val id: String, val title: String, val artist: String, val date: String, val provider: String, val trackCount: Int, val score: Double) {
    val line: String get() = listOf(artist, title).filter { it.isNotEmpty() }.joinToString(" – ") +
        (if (date.isNotEmpty()) " (${date.take(4)})" else "") + (if (provider.isNotEmpty()) " · $provider" else "")

    companion object {
        fun from(o: JsonObject?): ImportRelease? {
            if (o == null) return null
            val id = o.str("id")
            if (id.isEmpty()) return null
            return ImportRelease(id, o.str("title"), o.str("artist"), o.str("date"), o.str("provider"), o.int("track_count"), if (o.containsKey("match")) o.dbl("match") else o.dbl("score") / 100.0)
        }
    }
}

/** One file of an album folder, before and after. */
data class ImportRow(val path: String, val oldTitle: String, val newTitle: String, val newTrack: Int, val newPath: String)

/** One album folder of an import, with the release it was matched to (or none). */
data class ImportAlbum(
    val folder: String,
    val artist: String,
    val album: String,
    val files: Int,
    val release: ImportRelease?,
    val confidence: Double,
    val rows: List<ImportRow>,
) {
    companion object {
        fun from(o: JsonObject): ImportAlbum? {
            val folder = o.str("folder")
            val rows = o.arr("rows").objectsOf().map { r ->
                ImportRow(r.str("path"), r.str("old_title"), r.str("new_title"), r.int("new_track"), r.str("new_path"))
            }
            return ImportAlbum(
                folder = folder,
                artist = o.str("artist"),
                album = o.str("album").ifEmpty { folder.substringAfterLast('/') },
                files = if (o.containsKey("files")) o.int("files") else rows.size,
                release = ImportRelease.from(o.obj("release")),
                confidence = o.dbl("confidence"),
                rows = rows,
            )
        }
    }
}

/** One file to upload: what the phone picked, and its name on the server ("Album/01 Song.flac" for a folder). */
class UploadItem(val uri: Uri, val name: String, val size: Long, val mime: String)

/**
 * The file manager's calls (admin only). Blocking like [EncoreApi]: callers run them on Dispatchers.IO. Everything
 * goes through the app's OkHttpClient, so it carries the session and the CSRF header.
 */
class FilesApi(private val api: EncoreApi, private val client: OkHttpClient) {
    private val jsonType = "application/json; charset=utf-8".toMediaType()

    /** Uploads take as long as the file needs; the server writes `<name>.part` and renames it when complete. */
    private val slow: OkHttpClient by lazy {
        client.newBuilder().writeTimeout(10, TimeUnit.MINUTES).readTimeout(5, TimeUnit.MINUTES).build()
    }

    fun roots(): List<FileRoot> {
        val e = call("GET", "/api/files/roots", null)
        val list = if (e is JsonObject && e.containsKey("roots")) e["roots"].objects() else e.objects()
        return list.mapNotNull { FileRoot.from(it) }
    }

    /** sort: name | size | mtime; dir: asc | desc. */
    fun list(root: String, path: String, sort: String, dir: String, offset: Int, limit: Int = 500): FileListing {
        val o = call("GET", "/api/files/list?root=${enc(root)}&path=${enc(path)}&sort=${enc(sort)}&dir=${enc(dir)}&offset=$offset&limit=$limit", null)
        val obj = o as? JsonObject
        val entries = (obj?.get("entries") ?: o).objects().mapNotNull { FileEntry.from(it) }
        val total = obj?.int("total")?.takeIf { it > 0 } ?: (offset + entries.size)
        return FileListing(entries, total, offset)
    }

    fun mkdir(root: String, path: String, name: String) {
        call("POST", "/api/files/mkdir", body { put("root", root); put("path", path); put("name", name) })
    }

    fun rename(root: String, path: String, name: String) {
        call("POST", "/api/files/rename", body { put("root", root); put("path", path); put("name", name) })
    }

    /** Returns a short line when some could not be moved ("" when all went). */
    fun move(root: String, from: List<String>, toRoot: String, toPath: String): String =
        failures(call("POST", "/api/files/move", body {
            put("root", root)
            putJsonArray("from") { for (f in from) add(JsonPrimitive(f)) }
            put("to_root", toRoot)
            put("to_path", toPath)
        }))

    fun trash(root: String, paths: List<String>): String =
        failures(call("POST", "/api/files/trash", body { put("root", root); putJsonArray("paths") { for (f in paths) add(JsonPrimitive(f)) } }))

    /** GET /api/files/trash: what is in the trash, newest first. */
    fun trashItems(): List<TrashItem> {
        val e = call("GET", "/api/files/trash", null)
        val list = if (e is JsonObject && e.containsKey("items")) e["items"].objects() else e.objects()
        return list.mapNotNull { TrashItem.from(it) }
    }

    fun restore(ids: List<String>): String =
        failures(call("POST", "/api/files/restore", body { putJsonArray("items") { for (i in ids) add(JsonPrimitive(i)) } }))

    fun deleteForever(id: String) {
        call("DELETE", "/api/files/trash/${enc(id)}", null)
    }

    fun emptyTrash() {
        call("POST", "/api/files/empty-trash", body {})
    }

    /** GET /api/files/tags: the file's tags (title, artist, album, albumartist, tracknumber, discnumber, date, genre). */
    fun tags(root: String, path: String): Map<String, String> {
        val o = call("GET", "/api/files/tags?root=${enc(root)}&path=${enc(path)}", null) as? JsonObject ?: return emptyMap()
        val t = o.obj("tags") ?: o
        return TAGS.associateWith { t.str(it) }
    }

    fun putTags(root: String, path: String, tags: Map<String, String>) {
        call("PUT", "/api/files/tags", body {
            put("root", root)
            put("path", path)
            putJsonObject("tags") { for ((k, v) in tags) put(k, v) }
        })
    }

    /** POST /api/files/import/preview: the album folders under [path] and their matches ([releaseId]: one folder, this release). */
    fun importPreview(root: String, path: String, mode: String, releaseId: String? = null): List<ImportAlbum> {
        val e = call("POST", "/api/files/import/preview", body {
            put("root", root)
            put("path", path)
            put("mode", mode)
            if (releaseId != null) put("release_id", releaseId)
        })
        return ((e as? JsonObject)?.get("albums") ?: e).objects().mapNotNull { ImportAlbum.from(it) }
    }

    /** POST /api/files/import/match: other releases for one album folder. */
    fun importMatch(root: String, folder: String, artist: String, album: String): List<ImportRelease> {
        val e = call("POST", "/api/files/import/match", body { put("root", root); put("path", folder); put("artist", artist); put("album", album) })
        return ((e as? JsonObject)?.get("candidates") ?: e).objects().mapNotNull { ImportRelease.from(it) }
    }

    /** POST /api/files/import/apply → the server's line ("Imported 12 songs"). */
    fun importApply(root: String, path: String, mode: String, albums: List<Pair<String, String?>>): String {
        val o = call("POST", "/api/files/import/apply", body {
            put("root", root)
            put("path", path)
            put("mode", mode)
            putJsonArray("albums") {
                for ((folder, release) in albums) add(buildJsonObject {
                    put("folder", folder)
                    if (release != null) put("release_id", release) else put("release_id", null as String?)
                })
            }
        }) as? JsonObject
        val n = o?.int("imported") ?: 0
        return o?.str("message")?.ifEmpty { null } ?: if (n == 1) "Imported 1 song" else "Imported $n songs"
    }

    /**
     * POST /api/files/upload?root=&path=&conflict= with one file (multipart part `file`, its filename the name on the
     * server, folders with `/`), streamed from the phone's document; [progress] gets the bytes sent so far.
     * Returns what the server did: "uploaded", "renamed" (to [UploadOutcome.name]) or "skipped".
     */
    fun upload(root: String, path: String, conflict: String, item: UploadItem, resolver: ContentResolver, progress: (Long) -> Unit): UploadOutcome =
        try {
            upload(root, path, conflict, item, resolver, progress, item.size)
        } catch (_: SizeChanged) {
            // the picker's size was wrong (a cloud document, a file still being written): once more, its length unsaid
            if (item.size < 0L) throw IOException(CHANGED)
            try { upload(root, path, conflict, item, resolver, progress, -1L) } catch (_: SizeChanged) { throw IOException(CHANGED) }
        }

    private fun upload(root: String, path: String, conflict: String, item: UploadItem, resolver: ContentResolver, progress: (Long) -> Unit, length: Long): UploadOutcome {
        val body = DocumentBody(resolver, item.uri, item.mime.toMediaTypeOrNull() ?: OCTET, length, progress)
        val form = MultipartBody.Builder().setType(MultipartBody.FORM).addFormDataPart("file", item.name, body).build()
        val req = Request.Builder()
            .url(api.url("/api/files/upload?root=${enc(root)}&path=${enc(path)}&conflict=${enc(conflict)}"))
            .post(form)
            .header("Accept", "application/json")
            .build()
        slow.newCall(req).execute().use { res ->
            val text = res.body.string()
            val o = try { api.json.parseToJsonElement(text) as? JsonObject } catch (_: Exception) { null }
            if (!res.isSuccessful) throw ApiException(res.code, o?.str("detail")?.ifEmpty { null } ?: "HTTP ${res.code}", o)
            val renamed = o?.arr("renamed").objectsOf().firstOrNull()
            val skipped = o?.arr("skipped").objectsOf().firstOrNull()
            return when {
                skipped != null -> UploadOutcome("skipped", item.name, skipped.str("reason"))
                renamed != null -> UploadOutcome("renamed", renamed.str("to").ifEmpty { item.name }, "")
                else -> UploadOutcome("uploaded", item.name, "")
            }
        }
    }

    // ---- plumbing ----------------------------------------------------------------------------------------------

    private fun body(build: JsonObjectBuilder.() -> Unit): JsonObject = buildJsonObject(build)

    /** `{failed: [{path, reason}]}` as one line ("" when nothing failed). */
    private fun failures(e: JsonElement?): String {
        val failed = (e as? JsonObject)?.arr("failed").objectsOf()
        if (failed.isEmpty()) return ""
        val first = failed.first()
        val what = first.str("path").substringAfterLast('/').ifEmpty { first.str("id") }
        val why = first.str("reason")
        return (if (failed.size == 1) "$what: $why" else "${failed.size} weren't done. $what: $why").trim()
    }

    private fun call(method: String, path: String, body: JsonElement?): JsonElement? {
        if (api.base.isEmpty()) throw IOException("Not signed in")
        val rb: RequestBody? = when {
            body != null -> api.json.encodeToString(JsonElement.serializer(), body).toRequestBody(jsonType)
            method != "GET" -> "".toRequestBody(jsonType)
            else -> null
        }
        val req = Request.Builder().url(api.url(path)).method(method, rb).header("Accept", "application/json").build()
        client.newCall(req).execute().use { res ->
            val text = res.body.string()
            if (!res.isSuccessful) {
                val o = try { api.json.parseToJsonElement(text) as? JsonObject } catch (_: Exception) { null }
                throw ApiException(res.code, o?.str("detail")?.ifEmpty { null } ?: "HTTP ${res.code}", o)
            }
            if (res.code == 204 || text.isBlank()) return null
            return try { api.json.parseToJsonElement(text) } catch (_: Exception) { throw IOException("Unexpected reply from the server") }
        }
    }

    private fun enc(s: String): String = EncoreApi.enc(s)

    companion object {
        /** The tag writer's fields (the Fix song dialog's), in the order the editor shows them. */
        val TAGS = listOf("title", "artist", "album", "albumartist", "tracknumber", "discnumber", "date", "genre")
        private val OCTET: MediaType = "application/octet-stream".toMediaType()

        /** The picked documents as upload items (names and sizes from the provider). */
        fun documents(resolver: ContentResolver, uris: List<Uri>): List<UploadItem> = uris.mapNotNull { uri ->
            var name = ""
            var size = -1L
            try {
                resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                    if (c.moveToFirst()) {
                        name = c.stringAt(OpenableColumns.DISPLAY_NAME)
                        size = c.longAt(OpenableColumns.SIZE, -1L)
                    }
                }
            } catch (_: Exception) {}
            if (name.isEmpty()) name = uri.lastPathSegment?.substringAfterLast('/') ?: return@mapNotNull null
            UploadItem(uri, safeName(name), size, resolver.getType(uri) ?: "")
        }

        /**
         * Every file under a folder picked with OpenDocumentTree, named "<folder>/<sub>/<file>" (as a browser's folder
         * upload names them). At most [max] files.
         */
        fun tree(resolver: ContentResolver, treeUri: Uri, max: Int = 5000): List<UploadItem> {
            val out = ArrayList<UploadItem>()
            val rootId = DocumentsContract.getTreeDocumentId(treeUri)
            var top = ""
            try {
                resolver.query(DocumentsContract.buildDocumentUriUsingTree(treeUri, rootId), arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null)?.use { c ->
                    if (c.moveToFirst()) top = c.stringAt(DocumentsContract.Document.COLUMN_DISPLAY_NAME)
                }
            } catch (_: Exception) {}
            if (top.isEmpty()) top = rootId.substringAfterLast(':').substringAfterLast('/').ifEmpty { "Folder" }
            walk(resolver, treeUri, rootId, safeName(top) + "/", out, max, 0)
            return out
        }

        private fun walk(resolver: ContentResolver, treeUri: Uri, docId: String, prefix: String, out: MutableList<UploadItem>, max: Int, depth: Int) {
            if (out.size >= max || depth > 24) return
            val children = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, docId)
            val cols = arrayOf(
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                DocumentsContract.Document.COLUMN_MIME_TYPE,
                DocumentsContract.Document.COLUMN_SIZE,
            )
            val dirs = ArrayList<Pair<String, String>>()
            try {
                resolver.query(children, cols, null, null, null)?.use { c ->
                    while (c.moveToNext() && out.size < max) {
                        val id = c.stringAt(DocumentsContract.Document.COLUMN_DOCUMENT_ID)
                        val name = safeName(c.stringAt(DocumentsContract.Document.COLUMN_DISPLAY_NAME))
                        val mime = c.stringAt(DocumentsContract.Document.COLUMN_MIME_TYPE)
                        if (id.isEmpty() || name.isEmpty() || name.startsWith(".")) continue
                        if (mime == DocumentsContract.Document.MIME_TYPE_DIR) dirs.add(id to name)
                        else out.add(UploadItem(DocumentsContract.buildDocumentUriUsingTree(treeUri, id), prefix + name, c.longAt(DocumentsContract.Document.COLUMN_SIZE, -1L), mime))
                    }
                }
            } catch (_: Exception) {}
            for ((id, name) in dirs) walk(resolver, treeUri, id, "$prefix$name/", out, max, depth + 1)
        }

        /** A name the server accepts as one path part: no slashes or control characters. */
        fun safeName(name: String): String = name.replace('/', '_').replace('\\', '_').filter { it >= ' ' }.trim()

        private fun Cursor.stringAt(col: String): String {
            val i = getColumnIndex(col)
            return if (i >= 0 && !isNull(i)) getString(i) ?: "" else ""
        }

        private fun Cursor.longAt(col: String, missing: Long): Long {
            val i = getColumnIndex(col)
            return if (i >= 0 && !isNull(i)) getLong(i) else missing
        }
    }
}

/** What happened to one uploaded file. */
data class UploadOutcome(val result: String, val name: String, val reason: String)

/** The document had another number of bytes than the picker said (the upload is tried once more without a length). */
private class SizeChanged : IOException(CHANGED)

/** The upload error line for a file whose size kept changing ("<name>: …"). */
private const val CHANGED = "The file changed while it was uploading"

/**
 * A picked document streamed into the request (never held in memory), counting what was sent. With a [length]
 * (the picker's size) the request says it up front; when the document turns out longer or shorter, [SizeChanged] is
 * thrown before OkHttp's own "expected N bytes" error. -1: sent chunked, any length.
 */
private class DocumentBody(
    private val resolver: ContentResolver,
    private val uri: Uri,
    private val type: MediaType,
    private val length: Long,
    private val progress: (Long) -> Unit,
) : RequestBody() {
    override fun contentType(): MediaType = type
    override fun contentLength(): Long = length
    override fun writeTo(sink: BufferedSink) {
        val input = resolver.openInputStream(uri) ?: throw IOException("The file can't be read any more")
        input.source().use { src ->
            val buf = okio.Buffer()
            var sent = 0L
            while (true) {
                val n = src.read(buf, 64 * 1024L)
                if (n < 0) break
                if (length >= 0L && sent + n > length) throw SizeChanged()
                sink.write(buf, n)
                sent += n
                progress(sent)
            }
            if (length >= 0L && sent != length) throw SizeChanged()
        }
    }
}

/** The calls for this app's session. */
val AppGraph.files: FilesApi get() = FilesApi(api, http)

/** Whether the server has the file manager (admins only; GET /api/files/roots answered). Quiet on errors. */
object FilesAccess {
    private val _available = MutableStateFlow<Boolean?>(null)
    val available: StateFlow<Boolean?> = _available.asStateFlow()

    /** the server the answer is about (signing in to another one asks again) */
    @Volatile private var server = ""

    suspend fun probe(graph: AppGraph) {
        val base = graph.api.base
        if (base != server) { server = base; _available.value = null }
        if (_available.value == true) return
        try {
            withContext(Dispatchers.IO) { graph.files.roots() }
            _available.value = true
        } catch (e: CancellationException) {
            throw e
        } catch (e: ApiException) {
            // 404: a server before 0.33; 403: not an admin
            _available.value = false
        } catch (_: Exception) {
        }
    }
}
