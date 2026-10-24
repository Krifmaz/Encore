package app.encore.android.features.library

import android.content.ContentResolver
import android.net.Uri
import android.provider.OpenableColumns
import app.encore.android.AppGraph
import app.encore.android.core.str
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.EncoreApi
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MultipartBody
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okio.BufferedSink
import okio.source
import java.io.IOException
import java.util.concurrent.TimeUnit

/**
 * Bring your own music's calls (server routes::own_file). Blocking like [EncoreApi]: callers run them on
 * Dispatchers.IO. They go through the app's OkHttpClient, so they carry the session and the CSRF header.
 */
class OwnFileApi(private val api: EncoreApi, private val client: OkHttpClient) {
    /** An upload takes as long as the file needs (a hi-res FLAC is big; the server checks and tags it before it answers). */
    private val slow: OkHttpClient by lazy {
        client.newBuilder().writeTimeout(10, TimeUnit.MINUTES).readTimeout(5, TimeUnit.MINUTES).build()
    }
    private val jsonType = "application/json; charset=utf-8".toMediaType()

    /** POST /api/library/tracks/{id}/own-file: the document replaces song [id]'s file → `{track, before, after, warnings, quality?}`. */
    fun replace(id: Long, resolver: ContentResolver, uri: Uri): JsonObject = upload("/api/library/tracks/$id/own-file", resolver, uri)

    /** POST /api/library/own-files with one document: staged and matched → `{files: [{token, name, tags, match, candidates}], failed}`. */
    fun stage(resolver: ContentResolver, uri: Uri): JsonObject = upload("/api/library/own-files", resolver, uri)

    /** POST /api/library/own-files/apply: each (token, song) replaces that song's file → `{replaced, failed}`. */
    fun apply(items: List<Pair<String, Long>>): JsonObject {
        val body = buildJsonObject {
            putJsonArray("items") { for ((token, id) in items) add(buildJsonObject { put("token", token); put("track_id", id) }) }
        }
        val req = Request.Builder().url(api.url("/api/library/own-files/apply"))
            .post(api.json.encodeToString(JsonObject.serializer(), body).toRequestBody(jsonType)).header("Accept", "application/json").build()
        return execute(slow, req)
    }

    /** One document as the multipart `file` part, streamed (never held in memory). */
    private fun upload(path: String, resolver: ContentResolver, uri: Uri): JsonObject {
        if (api.base.isEmpty()) throw IOException("Not signed in")
        val name = displayName(resolver, uri)
        val type = (resolver.getType(uri) ?: "application/octet-stream").let { runCatching { it.toMediaType() }.getOrDefault(OCTET) }
        val form = MultipartBody.Builder().setType(MultipartBody.FORM).addFormDataPart("file", name, StreamBody(resolver, uri, type)).build()
        val req = Request.Builder().url(api.url(path)).post(form).header("Accept", "application/json").build()
        return execute(slow, req)
    }

    /** Runs [req] → its JSON answer; an error answer throws [ApiException] with the server's `detail`. */
    private fun execute(c: OkHttpClient, req: Request): JsonObject {
        c.newCall(req).execute().use { res ->
            val text = res.body.string()
            val o = try { api.json.parseToJsonElement(text) as? JsonObject } catch (_: Exception) { null }
            if (!res.isSuccessful) throw ApiException(res.code, o?.str("detail")?.ifEmpty { null } ?: "HTTP ${res.code}", o)
            return o ?: throw IOException("Unexpected reply from the server")
        }
    }

    companion object {
        private val OCTET: MediaType = "application/octet-stream".toMediaType()

        /** The document's name as its provider says it ("My Rip.flac"), else the last part of its address. */
        fun displayName(resolver: ContentResolver, uri: Uri): String {
            try {
                resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                    if (c.moveToFirst()) c.getString(0)?.takeIf { it.isNotBlank() }?.let { return it.replace('/', '_') }
                }
            } catch (_: Exception) {}
            return uri.lastPathSegment?.substringAfterLast('/')?.ifEmpty { null } ?: "file"
        }
    }
}

/** A picked document streamed into the request in chunks (any length). */
private class StreamBody(private val resolver: ContentResolver, private val uri: Uri, private val type: MediaType) : RequestBody() {
    override fun contentType(): MediaType = type
    override fun writeTo(sink: BufferedSink) {
        val input = resolver.openInputStream(uri) ?: throw IOException("The file can't be read any more")
        input.source().use { sink.writeAll(it) }
    }
}

/** The calls for this app's session. */
val AppGraph.ownFiles: OwnFileApi get() = OwnFileApi(api, http)
