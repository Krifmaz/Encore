package app.encore.android.features.group

import app.encore.android.core.long
import app.encore.android.core.objects
import app.encore.android.core.str
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.EncoreApi
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonObjectBuilder
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException

/**
 * The group listen routes (0.31; the shapes in docs/contracts/client-shapes-0.31-0.33.md, §3.6–§3.18). Blocking like
 * [EncoreApi]: callers run them on Dispatchers.IO. Every call carries this phone's device id, so the server knows which
 * membership it is.
 */
class GroupApi(private val api: EncoreApi, private val client: OkHttpClient, private val deviceId: () -> String, private val deviceName: () -> String) {
    private val jsonType = "application/json; charset=utf-8".toMediaType()

    /** GET /api/group: the live group listens this account may see. 403 (turned off) and 404 (older server) throw. */
    fun list(): List<GroupSummary> = parseList(call("GET", "/api/group", null))

    /** GET /api/group/code/{code}: who hosts the group behind a code, before joining. */
    fun lookup(code: String): GroupSummary? = (call("GET", "/api/group/code/" + EncoreApi.enc(code), null) as? JsonObject)?.let { GroupSummary.from(it) }

    /** POST /api/group: start one; what plays here becomes its queue. */
    fun start(visibility: String, everyoneControls: Boolean, trackIds: List<Long>, index: Int, positionSec: Double, playing: Boolean): GroupState =
        state(call("POST", "/api/group", body {
            put("visibility", visibility)
            put("everyone_controls", everyoneControls)
            putJsonArray("tracks") { for (id in trackIds) add(kotlinx.serialization.json.JsonPrimitive(id)) }
            put("index", index)
            put("position", positionSec)
            put("playing", playing)
        }))

    /** POST /api/group/join {code}. */
    fun joinCode(code: String): GroupState = state(call("POST", "/api/group/join", body { put("code", code) }))

    /** POST /api/group/{id}/join: an "anyone" group from the list, without its code. */
    fun joinId(id: String): GroupState = state(call("POST", "/api/group/${EncoreApi.enc(id)}/join", body {}))

    /** GET /api/group/current?device_id=: this device's group, or null (204). */
    fun current(): GroupState? = GroupState.from(call("GET", "/api/group/current?device_id=" + EncoreApi.enc(deviceId()), null))

    fun leave(id: String) { call("POST", "/api/group/${EncoreApi.enc(id)}/leave", body {}) }

    fun end(id: String) { call("POST", "/api/group/${EncoreApi.enc(id)}/end", body {}) }

    /** PATCH /api/group/{id}: the host's settings (any of everyone_controls, visibility, guests_allowed, max_members). */
    fun patch(id: String, build: JsonObjectBuilder.() -> Unit): GroupState = state(call("PATCH", "/api/group/${EncoreApi.enc(id)}", body(build)))

    /** POST /api/group/{id}/command {op, …}: answers the new state. */
    fun command(id: String, op: String, build: JsonObjectBuilder.() -> Unit): GroupState? =
        GroupState.from(call("POST", "/api/group/${EncoreApi.enc(id)}/command", body { put("op", op); build() }))

    /**
     * DELETE /api/group/{id}/members/{member}?device_id=: "Remove from the group" (guests; the host and admins). The
     * device id (a DELETE has no body) lets the answer's `you` be this device's membership, as in every other answer.
     */
    fun removeMember(id: String, member: String): GroupState? =
        GroupState.from(call("DELETE", "/api/group/${EncoreApi.enc(id)}/members/${EncoreApi.enc(member)}?device_id=" + EncoreApi.enc(deviceId()), null))

    /** GET /api/time: the server's clock and its process id. */
    fun time(): Pair<Long, String> {
        val o = call("GET", "/api/time", null) as? JsonObject ?: throw IOException("Unexpected reply from the server")
        return o.long("now_ms") to o.str("boot")
    }

    /** The event stream's URL. */
    fun eventsUrl(id: String): String = api.url("/api/group/${EncoreApi.enc(id)}/events?device_id=" + EncoreApi.enc(deviceId()))

    // ---- plumbing --------------------------------------------------------------------------------------------

    /** Every body carries the device: {device_id, device_name, …}. */
    private fun body(build: JsonObjectBuilder.() -> Unit): JsonObject = buildJsonObject {
        put("device_id", deviceId())
        put("device_name", deviceName())
        build()
    }

    private fun state(e: JsonElement?): GroupState = GroupState.from(e) ?: throw IOException("Unexpected reply from the server")

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

    companion object {
        /** A bare array or `{groups: [...]}`. */
        fun parseList(e: JsonElement?): List<GroupSummary> = when (e) {
            is JsonArray -> e.objects().mapNotNull { GroupSummary.from(it) }
            is JsonObject -> (e["groups"] ?: e["items"]).objects().mapNotNull { GroupSummary.from(it) }
            else -> emptyList()
        }
    }
}
