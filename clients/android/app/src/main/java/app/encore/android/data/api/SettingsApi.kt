package app.encore.android.data.api

import app.encore.android.AppGraph
import app.encore.android.core.parseJsonObject
import app.encore.android.core.str
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.CacheControl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MultipartBody
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException
import java.util.concurrent.TimeUnit

/**
 * The calls behind Settings (everything but the schema-drawn cards, which SchemaApi makes): the web's paths and
 * bodies (the web-rs/src/settings files and the pages they embed). Blocking like [EncoreApi]: callers run them on
 * Dispatchers.IO. Answers are read leniently by the screens (core/Json.kt readers); a failed call throws an
 * [ApiException] whose message is the server's `detail`.
 */
class SettingsApi(private val api: EncoreApi, private val client: OkHttpClient) {
    private val jsonType = "application/json; charset=utf-8".toMediaType()

    /** Tests, a backup, Sync now and Update now can take a while. */
    private val slow: OkHttpClient by lazy { client.newBuilder().readTimeout(300, TimeUnit.SECONDS).build() }

    /**
     * A plugin's install, update, removal or test, and an upload: the server answers only when the work is done (a
     * program or a model of gigabytes on a slow line), so no read limit, at most 3 hours in all.
     */
    private val long: OkHttpClient by lazy {
        client.newBuilder().readTimeout(0, TimeUnit.SECONDS).writeTimeout(0, TimeUnit.SECONDS).callTimeout(3, TimeUnit.HOURS).build()
    }

    /** The base URL of the signed-in server, for links the server does not send ("" when signed out). */
    val base: String get() = api.base

    // ---- account (Appearance, Language, the About card) ------------------------------------------------------

    /** GET /api/auth/info: app name, version, build and the server's features. */
    fun info(): JsonObject = obj("GET", "/api/auth/info")

    /** GET /api/auth/me with `prefs`. */
    fun me(): JsonObject = obj("GET", "/api/auth/me")

    /** PUT /api/auth/prefs: the keys given replace the account's (a null removes one); the answer is every pref. */
    fun putPrefs(patch: JsonObject): JsonObject = obj("PUT", "/api/auth/prefs", patch)

    fun translateLanguages(): JsonObject = obj("GET", "/api/translate/languages")

    fun translateStatus(): JsonObject = obj("GET", "/api/translate/status")

    fun translateRun() { call("POST", "/api/translate/run", null) }

    // ---- Connections -----------------------------------------------------------------------------------------

    fun connections(): JsonObject = obj("GET", "/api/connections")

    /**
     * POST /api/connections/{service}: a sign-in form's fields (username/password, token, profile, headers) or
     * `{}` to start a sign-in (`{url}` for Spotify, Tidal and Last.fm; `{url, pin_id}` for Plex; a device code for
     * YouTube Music).
     */
    fun connect(service: String, body: JsonObject): JsonObject = obj("POST", "/api/connections/${enc(service)}", body, slow = true)

    fun plexCheck(pinId: JsonElement): JsonObject = obj("POST", "/api/connections/plex/check", JsonObject(mapOf("pin_id" to pinId)))

    fun ytmusicCheck(deviceCode: JsonElement): JsonObject = obj("POST", "/api/connections/ytmusic/check", JsonObject(mapOf("device_code" to deviceCode)))

    /** PATCH /api/connections/{service} {scrobble | sync_likes | sync_playlists: bool}. */
    fun setConnectionFlag(service: String, flag: String, on: Boolean) {
        call("PATCH", "/api/connections/${enc(service)}", JsonObject(mapOf(flag to JsonPrimitive(on))))
    }

    fun disconnect(service: String) { call("DELETE", "/api/connections/${enc(service)}", null) }

    fun remotePlaylists(service: String): List<JsonObject> = rows(call("GET", "/api/connections/${enc(service)}/playlists", null, slow = true))

    /** POST /api/connections/{service}/playlists/{rid}/import {name, url, follow}: `{added, duplicates_skipped}`. */
    fun importRemote(service: String, rid: String, name: String, url: String, follow: Boolean): JsonObject {
        val body = buildJsonObject {
            put("name", name)
            put("url", if (url.isEmpty()) JsonNull else JsonPrimitive(url))
            put("follow", follow)
        }
        return obj("POST", "/api/connections/${enc(service)}/playlists/${enc(rid)}/import", body, slow = true)
    }

    /** POST /api/sync: `{liked_added, liked_removed, pushed, playlists, errors}`. */
    fun syncNow(): JsonObject = obj("POST", "/api/sync", null, slow = true)

    // ---- People ----------------------------------------------------------------------------------------------

    fun users(): List<JsonObject> = rows(call("GET", "/api/admin/users", null))

    fun permissions(): JsonObject = obj("GET", "/api/admin/permissions")

    /** PATCH /api/admin/users/{id} with one change: role, request_limit, auto_approve, disabled, password or perms. */
    fun patchUser(id: String, body: JsonObject) { call("PATCH", "/api/admin/users/${enc(id)}", body) }

    fun deleteUser(id: String) { call("DELETE", "/api/admin/users/${enc(id)}", null) }

    /** POST /api/admin/users {username, email, password, role}. */
    fun createUser(body: JsonObject) { call("POST", "/api/admin/users", body) }

    /** POST /api/admin/users/bulk {perm, value}: one permission for every non-admin. */
    fun bulkPermission(perm: String, value: Boolean) {
        call("POST", "/api/admin/users/bulk", buildJsonObject { put("perm", perm); put("value", value) })
    }

    fun invites(): List<JsonObject> = rows(call("GET", "/api/admin/invites", null))

    /** POST /api/admin/invites {max_uses, expires_days, role, note}: the invite with its `code`. */
    fun createInvite(body: JsonObject): JsonObject = obj("POST", "/api/admin/invites", body)

    fun deleteInvite(id: String) { call("DELETE", "/api/admin/invites/${enc(id)}", null) }

    // ---- settings saved one key at a time (the switches outside the schema cards) ----------------------------

    /** GET /api/admin/settings: every value, with `_meta` saying where each one comes from. */
    fun adminSettings(): JsonObject = obj("GET", "/api/admin/settings")

    /**
     * Told about every answer of [putSettings] (Settings sets it while open, so the Server pages show what a switch
     * on another page saved: the sign-up mode under Invites is also General's).
     */
    @Volatile var onSettingsSaved: ((JsonObject) -> Unit)? = null

    /** PUT /api/admin/settings with the keys to change; the answer is every value again. */
    fun putSettings(body: JsonObject): JsonObject = obj("PUT", "/api/admin/settings", body).also { onSettingsSaved?.invoke(it) }

    /** GET /api/system/api-key (the owner's): never kept in the HTTP cache, whatever a proxy does to Cache-Control. */
    fun apiKey(): JsonObject =
        send(client, Request.Builder().url(api.url("/api/system/api-key")).cacheControl(CacheControl.Builder().noStore().build()).get()) as? JsonObject
            ?: JsonObject(emptyMap())

    fun regenerateApiKey(): JsonObject = obj("POST", "/api/admin/settings/api-key", null)

    // ---- Server pages with their own code ---------------------------------------------------------------------

    fun autodownload(): JsonObject = obj("GET", "/api/admin/autodownload")

    /** PUT /api/admin/autodownload {settings, users}. */
    fun putAutodownload(body: JsonObject): JsonObject = obj("PUT", "/api/admin/autodownload", body)

    fun autodownloadRunNow() { call("POST", "/api/admin/autodownload/run-now", null) }

    /** POST /api/admin/autodownload/upgrade-now: `{queued}`. */
    fun autodownloadUpgradeNow(): JsonObject = obj("POST", "/api/admin/autodownload/upgrade-now", null)

    fun contribute(): JsonObject = obj("GET", "/api/contribute")

    /** POST /api/contribute/{acoustid|isrc}/run: `{message}`. */
    fun contributeRun(what: String): JsonObject = obj("POST", "/api/contribute/${enc(what)}/run", null, slow = true)

    /** POST /api/contribute/mb/connect: `{url}` of MusicBrainz's sign-in page. */
    fun mbConnect(): JsonObject = obj("POST", "/api/contribute/mb/connect", null)

    fun mbDisconnect() { call("POST", "/api/contribute/mb/disconnect", null) }

    fun reports(): JsonObject = obj("GET", "/api/reports")

    fun blocks(): JsonObject = obj("GET", "/api/blocks")

    fun suspect(): JsonObject = obj("GET", "/api/quality/suspect")

    fun retryReport(id: Long) { call("POST", "/api/reports/$id/retry", null) }

    fun dismissReport(id: Long) { call("POST", "/api/reports/$id/dismiss", null) }

    fun unblock(id: Long) { call("DELETE", "/api/blocks/$id", null) }

    /** POST /api/quality/tracks/{id}/check: `{status, summary}`. */
    fun checkTrack(id: Long): JsonObject = obj("POST", "/api/quality/tracks/$id/check", null, slow = true)

    fun tasks(): JsonObject = obj("GET", "/api/admin/tasks")

    /** PATCH /api/admin/tasks/{key} {enabled} or {minutes}. */
    fun patchTask(key: String, body: JsonObject) { call("PATCH", "/api/admin/tasks/${enc(key)}", body) }

    fun runTask(key: String) { call("POST", "/api/admin/tasks/${enc(key)}/run", null) }

    // ---- Plugins (0.32; web-rs/src/settings/plugins.rs, server/crates/api/src/routes/plugins.rs) ----------------

    /**
     * GET /api/plugins as a probe: true when this server has plugins, false when it is older than 0.32 (a 404, or
     * its web page instead of the list), null when it could not tell (not reached, signed out).
     */
    fun pluginsProbe(): Boolean? = try {
        if (api.base.isEmpty()) {
            null
        } else {
            client.newCall(Request.Builder().url(api.url("/api/plugins")).header("Accept", "application/json").get().build()).execute().use { res ->
                app.encore.android.features.settings.PluginRules.hasPlugins(res.code, res.body.string())
            }
        }
    } catch (_: Exception) {
        null
    }

    /** GET /api/plugins: `{sandbox: {level, text, reason}, encore_version, protocol, upload_max_mb, plugins: [card…]}`. */
    fun plugins(): JsonObject = obj("GET", "/api/plugins")

    /** GET /api/plugins/{id}: one card, with its status asked now when it is installed. */
    fun plugin(id: String): JsonObject = obj("GET", "/api/plugins/${enc(id)}")

    /**
     * POST /api/plugins/{id}/{install|update|check|enable|disable|remove}: the card. The server answers when the work
     * is done (an install downloads the program), so these wait without a read limit.
     */
    fun pluginAction(id: String, act: String): JsonObject = obj("POST", "/api/plugins/${enc(id)}/${enc(act)}", null, long = true)

    /** POST /api/plugins/check: the list, after every installed plugin was checked for an update. */
    fun pluginsCheck(): JsonObject = obj("POST", "/api/plugins/check", null, long = true)

    /** POST /api/plugins/{id}/test: `{ok: true, message}` or `{ok: false, error, code, retry_after}`. */
    fun pluginTest(id: String): JsonObject = obj("POST", "/api/plugins/${enc(id)}/test", null, long = true)

    /** POST /api/plugins/{id}/auto-update {on}: the card. */
    fun pluginAutoUpdate(id: String, on: Boolean): JsonObject = obj("POST", "/api/plugins/${enc(id)}/auto-update", buildJsonObject { put("on", on) })

    /** GET /api/plugins/{id}/logs?lines=200: `{id, lines, file}`. */
    fun pluginLogs(id: String, lines: Int = 200): JsonObject = obj("GET", "/api/plugins/${enc(id)}/logs?lines=$lines")

    /** GET /api/plugins/{id}/settings: `{id, builtin, settings_on, fields, values}` (fields in the schema's shape plus `part`). */
    fun pluginSettings(id: String): JsonObject = obj("GET", "/api/plugins/${enc(id)}/settings")

    /** PUT /api/plugins/{id}/settings {values}: a plugin's own settings (a masked secret is left as it is); the settings answer. */
    fun pluginSaveSettings(id: String, values: JsonObject): JsonObject =
        obj("PUT", "/api/plugins/${enc(id)}/settings", buildJsonObject { put("values", values) })

    /** POST /api/plugins/upload, the zip as the multipart field `file`: the new plugin's card. */
    fun pluginUpload(fileName: String, body: RequestBody): JsonObject {
        val form = MultipartBody.Builder().setType(MultipartBody.FORM).addFormDataPart("file", fileName, body).build()
        return send(long, Request.Builder().url(api.url("/api/plugins/upload")).post(form)) as? JsonObject ?: JsonObject(emptyMap())
    }

    /** POST /api/plugins/install-url {url}: an https address of a zip or a plugin.toml, or `owner/repo@ref`; the card. */
    fun pluginInstallUrl(url: String): JsonObject = obj("POST", "/api/plugins/install-url", buildJsonObject { put("url", url) }, long = true)

    // ---- Downloads and Server processing: the cards' extras ----------------------------------------------------

    fun soulseekStatus(): JsonObject = obj("GET", "/api/soulseek/status")

    fun soulseekStart() { call("POST", "/api/soulseek/start", null) }

    fun vpn(): JsonObject = obj("GET", "/api/system/vpn")

    fun vpnTest(): JsonObject = obj("POST", "/api/system/vpn/test", null, slow = true)

    /** GET /api/playback/dsp: the pre-render cache and the engine (with its own settings) for the Cadenza card. */
    fun dsp(): JsonObject = obj("GET", "/api/playback/dsp")

    fun dspCacheClear() { call("POST", "/api/playback/dsp/cache/clear", null) }

    fun tidalLoginStatus(): JsonObject = obj("GET", "/api/system/streamrip/tidal-login")

    fun tidalLoginStart(): JsonObject = obj("POST", "/api/system/streamrip/tidal-login", null, slow = true)

    /** POST /api/system/streamrip/tidal-tokens {toml}: the pasted [tidal] section. */
    fun tidalTokensSave(toml: String): JsonObject = obj("POST", "/api/system/streamrip/tidal-tokens", buildJsonObject { put("toml", toml) })

    fun tidalTokensClear(): JsonObject = obj("DELETE", "/api/system/streamrip/tidal-tokens", null)

    /** POST /api/system/streamrip/reset-config: `{ok, message}`. */
    fun streamripResetConfig(): JsonObject = obj("POST", "/api/system/streamrip/reset-config", null, slow = true)

    fun youtubeAuth(): JsonObject = obj("GET", "/api/youtube/auth")

    fun youtubeRefresh(): JsonObject = obj("POST", "/api/youtube/refresh", null, slow = true)

    /** POST /api/youtube/signout {source: "all"}. */
    fun youtubeSignOut(): JsonObject = obj("POST", "/api/youtube/signout", buildJsonObject { put("source", "all") })

    fun youtubeExtensionToken(): JsonObject = obj("POST", "/api/youtube/extension-token", null)

    // ---- Diagnostics ---------------------------------------------------------------------------------------------

    fun diagnostics(): JsonObject = obj("GET", "/api/system/diagnostics", null, slow = true)

    fun backups(): JsonObject = obj("GET", "/api/system/backups")

    /** POST /api/system/backups: `{created: {name}, backups}`. */
    fun backupNow(): JsonObject = obj("POST", "/api/system/backups", null, slow = true)

    fun restoreBackup(name: String) { call("POST", "/api/system/backups/${enc(name)}/restore", null, slow = true) }

    /** The address of one backup (GET /api/system/backups/{name}), for the system's download manager. */
    fun backupUrl(name: String): String = api.url("/api/system/backups/${enc(name)}")

    /** POST /api/system/backups/upload, the file as the multipart field `file`: `{created: {name}, backups}`. */
    fun uploadBackup(fileName: String, body: RequestBody): JsonObject {
        val form = MultipartBody.Builder().setType(MultipartBody.FORM).addFormDataPart("file", fileName, body).build()
        return send(slow, Request.Builder().url(api.url("/api/system/backups/upload")).post(form)) as? JsonObject ?: JsonObject(emptyMap())
    }

    fun crashes(): JsonObject = obj("GET", "/api/diagnostics/client-crash")

    fun clearCrashes() { call("DELETE", "/api/diagnostics/client-crash", null) }

    /** GET /api/system/update (`?force=1` for Check now): this server's version and the newest release. */
    fun serverUpdate(force: Boolean): JsonObject = obj("GET", if (force) "/api/system/update?force=1" else "/api/system/update", null, slow = force)

    /** POST /api/system/update {tag}: `{message}`; the server restarts on the new version a moment later. */
    fun serverUpdateNow(tag: String): JsonObject = obj("POST", "/api/system/update", buildJsonObject { put("tag", tag) }, slow = true)

    fun restartServer() { call("POST", "/api/system/restart", null) }

    /**
     * GET /api/health with a short timeout and no cache (the poll after a restart): the version when the server
     * answers with ok, null when it does not answer.
     */
    fun health(): String? = try {
        val quick = client.newBuilder().callTimeout(3, TimeUnit.SECONDS).build()
        val req = Request.Builder().url(api.url("/api/health")).header("Cache-Control", "no-store").get().build()
        quick.newCall(req).execute().use { res ->
            if (!res.isSuccessful) null else parseJsonObject(res.body.string())?.takeIf { it.str("ok") == "true" }?.str("version")
        }
    } catch (_: Exception) {
        null
    }

    // ---- plumbing ------------------------------------------------------------------------------------------------

    private fun obj(method: String, path: String, body: JsonElement? = null, slow: Boolean = false, long: Boolean = false): JsonObject =
        call(method, path, body, slow, long) as? JsonObject ?: JsonObject(emptyMap())

    private fun rows(el: JsonElement?): List<JsonObject> = when (el) {
        is kotlinx.serialization.json.JsonArray -> el.filterIsInstance<JsonObject>()
        is JsonObject -> (el["items"] as? kotlinx.serialization.json.JsonArray)?.filterIsInstance<JsonObject>() ?: emptyList()
        else -> emptyList()
    }

    private fun call(method: String, path: String, body: JsonElement?, slow: Boolean = false, long: Boolean = false): JsonElement? {
        val rb: RequestBody? = when {
            body != null -> api.json.encodeToString(JsonElement.serializer(), body).toRequestBody(jsonType)
            method != "GET" -> "".toRequestBody(jsonType)
            else -> null
        }
        val http = when {
            long -> this.long
            slow -> this.slow
            else -> client
        }
        return send(http, Request.Builder().url(api.url(path)).method(method, rb))
    }

    private fun send(http: OkHttpClient, req: Request.Builder): JsonElement? {
        if (api.base.isEmpty()) throw IOException("Not signed in")
        http.newCall(req.header("Accept", "application/json").build()).execute().use { res ->
            val text = res.body.string()
            if (!res.isSuccessful) {
                val o = parseJsonObject(text)
                throw ApiException(res.code, o?.str("detail")?.ifEmpty { null } ?: "HTTP ${res.code}", o)
            }
            if (text.isBlank()) return null
            return try { api.json.parseToJsonElement(text) } catch (_: Exception) { throw IOException("Unexpected reply from the server") }
        }
    }

    private fun enc(s: String): String = EncoreApi.enc(s)
}

private val settingsApiCache = java.util.WeakHashMap<AppGraph, SettingsApi>()

/** The one [SettingsApi] of the app's graph. */
val AppGraph.settingsApi: SettingsApi
    get() = synchronized(settingsApiCache) { settingsApiCache.getOrPut(this) { SettingsApi(api, http) } }
