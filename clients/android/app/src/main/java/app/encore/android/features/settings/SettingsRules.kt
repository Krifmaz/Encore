package app.encore.android.features.settings

import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.obj
import app.encore.android.core.str
import app.encore.android.core.strings
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlin.math.floor

// The words and decisions of the hand-written Settings pages, apart from Compose so they can be tested: the same
// rules and sentences as the web (the web-rs/src/settings files and the pages they embed).

object SettingsRules {
    /** JavaScript's `Math.round` (halves go up). */
    fun jsRound(x: Double): Long = floor(x + 0.5).toLong()

    /** A number as JavaScript prints it: 15 → "15", 2.5 → "2.5". */
    fun num(d: Double): String = if (d == floor(d) && !d.isInfinite() && kotlin.math.abs(d) < 1e15) d.toLong().toString() else d.toString()

    /** "just now", "12 min ago", "3 h ago", "4 days ago"; "never" for 0 (tasks.rs and youtube.rs `ago`). */
    fun ago(t: Double, now: Double): String {
        if (t == 0.0) return "never"
        val s = now - t
        return when {
            s < 90 -> "just now"
            s < 5400 -> "${jsRound(s / 60)} min ago"
            s < 129600 -> "${jsRound(s / 3600)} h ago"
            else -> "${jsRound(s / 86400)} days ago"
        }
    }

    /** "any moment", "in 12 min", "in 3 h", "in 4 days"; "" for 0. */
    fun until(t: Double, now: Double): String {
        if (t == 0.0) return ""
        val s = t - now
        return when {
            s <= 60 -> "any moment"
            s < 5400 -> "in ${jsRound(s / 60)} min"
            s < 129600 -> "in ${jsRound(s / 3600)} h"
            else -> "in ${jsRound(s / 86400)} days"
        }
    }

    // ---- the VPN for Soulseek (diagnostics.rs vpn_text) ------------------------------------------------------

    /** "Connected · port 51234 · public IP 1.2.3.4 · last handshake 12 s ago" / "Connecting…" / the error / "Off". */
    fun vpnText(v: JsonObject?): String {
        if (v == null) return "Off"
        return when (v.str("state")) {
            "up" -> {
                val parts = arrayListOf("Connected")
                if (!nullish(v, "forwarded_port")) parts.add("port ${v.str("forwarded_port")}")
                else if (v.bool("port_forwarding")) {
                    parts.add(if (v.str("natpmp_error").isNotEmpty()) "no forwarded port (${v.str("natpmp_error")})" else "no forwarded port yet")
                }
                if (v.str("public_ip").isNotEmpty()) parts.add("public IP ${v.str("public_ip")}")
                if (!nullish(v, "last_handshake_secs_ago")) parts.add("last handshake ${jsRound(v.dbl("last_handshake_secs_ago"))} s ago")
                parts.joinToString(" · ")
            }
            "connecting" -> "Connecting…"
            "error" -> v.str("error").ifEmpty { "The VPN could not connect" }
            else -> "Off"
        }
    }

    fun vpnIsError(v: JsonObject?): Boolean = v?.str("state") == "error"

    /** Test VPN's extra: " · Port forwarding failed: …" when NAT-PMP said no ("" otherwise). */
    fun vpnTestNote(r: JsonObject): String {
        val n = r.obj("natpmp") ?: return ""
        val ok = n["ok"] as? JsonPrimitive
        return if (ok != null && !ok.isString && ok.content == "false" && n.str("error").isNotEmpty()) "Port forwarding failed: ${n.str("error")}" else ""
    }

    // ---- Soulseek status (apps.rs soulseek_state_html) -------------------------------------------------------

    /** The status line of Settings → Soulseek: its text, whether it is an error, the warning and the log to show. */
    data class SlskLine(val text: String, val error: Boolean, val warning: String, val log: List<String>)

    fun soulseekLine(r: JsonObject?): SlskLine {
        if (r == null) return SlskLine("Status unavailable", false, "", emptyList())
        val ver = r.str("version").let { if (it.isEmpty()) "" else " v" + it.removePrefix("v") }
        val error = r.str("error")
        var red = false
        val state = r.str("state")
        var line = when (state) {
            "not_installed" -> "Not installed"
            "starting" -> { if (error.isNotEmpty()) red = true; "Starting…" + if (error.isNotEmpty()) " · $error" else "" }
            "running" -> "Running$ver" + when {
                r.bool("logged_in") -> " · signed in as ${r.str("username").ifEmpty { "?" }}"
                error.isNotEmpty() -> { red = true; " · $error" }
                else -> " · signing in…"
            }
            "crashed" -> { red = true; "Crashed" + if (error.isNotEmpty()) " – $error" else "" }
            "blocked" -> { red = true; "Can't start" + if (error.isNotEmpty()) " – $error" else "" }
            "waiting_vpn" -> "Waiting for the VPN"
            else -> {
                val en = r["enabled"] as? JsonPrimitive
                if (en != null && !en.isString && en.content == "false") "Off – switch on Enable Soulseek and save" else "Off"
            }
        }
        if (r.str("hint").isNotEmpty()) line = "$line · ${r.str("hint")}"
        val unhealthy = state == "crashed" || state == "blocked" || (state == "running" && !r.bool("logged_in"))
        val log = r.arr("log").strings()
        return SlskLine(line, red, r.str("warning"), if (unhealthy) log else emptyList())
    }

    /** Start now belongs to a crash or a blocked start: the states a person can act on. */
    fun canStartNow(state: String): Boolean = state == "crashed" || state == "blocked"

    // ---- Cadenza (apps.rs) -----------------------------------------------------------------------------------

    /** "Cache: 812 MB of 4096 MB · 23 songs · renders at 6.2× real time". */
    fun cadenzaCacheText(c: JsonObject?): String {
        if (c == null) return "Cache: not available on this server"
        val n = c.dbl("files")
        val f = c.dbl("last_factor")
        return "Cache: ${jsRound(c.dbl("used_mb"))} MB of ${jsRound(c.dbl("max_mb"))} MB · ${num(n)} ${if (n == 1.0) "song" else "songs"}" +
            if (f > 0) " · renders at ${String.format(java.util.Locale.US, "%.1f", f)}× real time" else ""
    }

    /** "Engine: Cadenza 2.1.0 (updated)" / "Engine: Cadenza (built in)". */
    fun cadenzaEngineText(e: JsonObject?): String =
        "Engine: " + if (e != null && e.str("source") == "updated" && e.str("version").isNotEmpty()) "Cadenza ${e.str("version")} (updated)" else "Cadenza (built in)"

    /** One setting of the engine in use (`engine.settings` of GET /api/playback/dsp). */
    data class EngineSetting(
        val key: String,
        val label: String,
        val help: String,
        val type: String,
        val default: JsonElement,
        val options: List<Pair<JsonElement, String>>,
        val min: Double?,
        val max: Double?,
        val step: Double?,
    )

    fun engineSettings(engine: JsonObject?): List<EngineSetting> {
        val list = engine?.arr("settings") ?: return emptyList()
        return list.mapNotNull { e ->
            val s = e as? JsonObject ?: return@mapNotNull null
            val type = s.str("type")
            if (s.str("key").isEmpty() || type !in listOf("select", "bool", "number")) return@mapNotNull null
            EngineSetting(
                key = s.str("key"),
                label = s.str("label").ifEmpty { s.str("key") },
                help = s.str("help"),
                type = type,
                default = s["default"] ?: JsonNull,
                options = s.arr("options")?.mapNotNull { o -> (o as? JsonArray)?.let { (it.getOrNull(0) ?: JsonNull) to text(it.getOrNull(1)) } } ?: emptyList(),
                min = (s["min"] as? JsonPrimitive)?.content?.toDoubleOrNull()?.takeIf { it.isFinite() },
                max = (s["max"] as? JsonPrimitive)?.content?.toDoubleOrNull()?.takeIf { it.isFinite() },
                step = (s["step"] as? JsonPrimitive)?.content?.toDoubleOrNull()?.takeIf { it.isFinite() },
            )
        }
    }

    /** "Default: on" / the option's label / the number. */
    fun engineDefaultText(s: EngineSetting): String = when (s.type) {
        "bool" -> if (truthy(s.default)) "on" else "off"
        "select" -> s.options.firstOrNull { text(it.first) == text(s.default) }?.second ?: text(s.default)
        else -> text(s.default)
    }

    /** What a field starts with: the saved value, the default when a select no longer offers the saved one. */
    fun engineInitial(s: EngineSetting, values: JsonObject?): String {
        val v = values?.get(s.key) ?: s.default
        return when (s.type) {
            "bool" -> if (truthy(v)) "true" else "false"
            "select" -> if (s.options.any { text(it.first) == text(v) }) text(v) else text(s.default)
            else -> text(v)
        }
    }

    /**
     * The engine settings as the JSON text for `cadenza_engine_settings` (apps.rs engine_settings_value): what was
     * saved, with each field of this engine set to what it holds, and left out where it is the default; keys this
     * engine doesn't have stay as saved.
     */
    fun engineValue(settings: List<EngineSetting>, saved: JsonObject?, edits: Map<String, String>): String {
        val out = LinkedHashMap<String, JsonElement>(saved ?: emptyMap())
        for (s in settings) {
            val raw = edits[s.key] ?: continue
            val v: JsonElement = when (s.type) {
                "bool" -> JsonPrimitive(raw == "true")
                "number" -> {
                    val n = raw.trim().replace(',', '.').toDoubleOrNull()
                    if (n == null || !n.isFinite()) { out.remove(s.key); continue }
                    val d = (s.default as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0
                    if (n == d) { out.remove(s.key); continue }
                    if (n == floor(n) && kotlin.math.abs(n) < 1e15) JsonPrimitive(n.toLong()) else JsonPrimitive(n)
                }
                else -> s.options.firstOrNull { text(it.first) == raw }?.first ?: JsonPrimitive(raw)
            }
            if (s.type != "number" && v == s.default) out.remove(s.key) else out[s.key] = v
        }
        return JsonObject(out).toString()
    }

    /**
     * What the Cadenza card's Save (and the save before its Test) sends for the engine block: [engineValue], or null
     * when nothing in it changed (no edits, or edits that come back to what is saved), so only changed keys are saved.
     */
    fun engineChange(settings: List<EngineSetting>, saved: JsonObject?, edits: Map<String, String>): String? {
        if (edits.isEmpty()) return null
        val v = engineValue(settings, saved, edits)
        return if (v == JsonObject(saved ?: emptyMap()).toString()) null else v
    }

    // ---- Download tools (apps.rs) ----------------------------------------------------------------------------

    /** The tools of GET /api/system/tools (a list, or `{tools}`). */
    fun toolList(r: JsonElement?): List<JsonObject> = when (r) {
        is JsonArray -> r.filterIsInstance<JsonObject>()
        is JsonObject -> r.arr("tools")?.filterIsInstance<JsonObject>() ?: emptyList()
        else -> emptyList()
    }

    /** slskd and the Translator are installed by Encore, not bundled: before that their button says Install. */
    fun toolInstall(t: JsonObject): Boolean = (t.str("id") == "slskd" || t.str("id") == "translator") && !truthy(t["installed"])

    /** What a tool row offers on the right. */
    sealed interface ToolAction {
        data class Install(val latest: String) : ToolAction
        data class Update(val latest: String) : ToolAction
        data class Available(val latest: String) : ToolAction
        data object UpToDate : ToolAction
        data object NotChecked : ToolAction
        data object None : ToolAction
    }

    fun toolAction(t: JsonObject): ToolAction {
        val latest = t.str("latest")
        val updatable = t.bool("updatable")
        return when {
            toolInstall(t) && updatable -> ToolAction.Install(latest)
            t.bool("update_available") && updatable -> ToolAction.Update(latest)
            t.bool("update_available") -> ToolAction.Available(latest)
            latest.isNotEmpty() -> ToolAction.UpToDate
            updatable -> ToolAction.NotChecked
            else -> ToolAction.None
        }
    }

    /** The reset link of a row: (link, confirm question, toast). */
    fun toolReset(t: JsonObject): Triple<String, String, String> = when (t.str("id")) {
        "cadenza" -> Triple("Use the built-in engine", "Go back to the engine built into Encore? The downloaded Cadenza is removed.", "Back to the built-in engine")
        "slskd" -> Triple("Remove slskd", "Remove slskd? Soulseek stays off until you install it again.", "slskd removed")
        "translator" -> Triple(
            "Remove the Translator",
            "Remove the Translator and its model? Translations made so far are kept; new titles only get official translations until you install it again.",
            "Translator removed",
        )
        else -> {
            val name = t.str("name").ifEmpty { t.str("id") }
            Triple("Use the bundled version", "Go back to the $name version that came with Encore? The updated copy is removed.", "Back to the bundled $name")
        }
    }

    /** The automatic-update switches under the tool list (shown when the server has the key). */
    val TOOL_TOGGLES: List<Pair<String, String>> = listOf(
        "tools_auto_ytdlp" to "Update yt-dlp automatically (recommended – sites change often)",
        "tools_auto_streamrip" to "Update streamrip automatically when a new release is out",
        "tools_auto_cadenza" to "Update Cadenza automatically when a new release is out",
        "tools_auto_slskd" to "Update slskd automatically when a new release is out",
        "tools_auto_translator" to "Update the Translator (llama.cpp) automatically",
        "translate_background" to "Translate the whole library in the background (titles on screen are always translated first)",
    )

    val TRANSLATOR_MODELS: List<Pair<String, String>> = listOf(
        "small" to "Small – Qwen3 4B Instruct (about 2.5 GB)",
        "best" to "Best – Qwen3 8B (about 5 GB)",
    )

    // ---- Tidal sign-in (apps.rs bind_tidal) ------------------------------------------------------------------

    /** The state line and whether the account is signed in (partial counts as signed in). */
    fun tidalState(r: JsonObject): Pair<String, Boolean> {
        val partial = r.bool("partial")
        val signed = r.bool("signed_in") || partial
        val text = when {
            partial -> "Signed in for 16/44.1 FLAC only – sign in again for hi-res (step 2 was not finished)."
            r.bool("signed_in") -> "Signed in – Tidal downloads use this account."
            else -> "Not signed in – Tidal needs a one-time sign-in with your own Tidal account (no API key)."
        }
        return text to signed
    }

    /** "Step 1 of 2 · " when the sign-in has more than one step. */
    fun tidalStepPrefix(r: JsonObject): String {
        val steps = r.dbl("steps").let { if (it >= 1) it.toLong() else 1L }
        val step = r.dbl("step").let { if (it >= 1) it.toLong() else 1L }
        return if (steps > 1) "Step $step of $steps · " else ""
    }

    // ---- YouTube sign-in (youtube.rs) ------------------------------------------------------------------------

    fun youtubeSourceWords(key: String): String = when (key) {
        "handoff" -> "Handed-over sign-in"
        "extension" -> "Everyday browser (extension)"
        else -> "Pasted cookies"
    }

    /** "Handed-over sign-in: signed in as …" for one source. */
    fun youtubeSourceLine(key: String, s: JsonObject?): String {
        val state = when {
            s == null || !s.bool("has_cookies") -> if (key == "pasted") "none" else "not connected"
            s.bool("signed_in") -> "signed in" + if (s.str("account").isNotEmpty()) " as ${s.str("account")}" else ""
            else -> "signed out"
        }
        return "${youtubeSourceWords(key)}: $state"
    }

    /** The badge of the card: "No cookies" / "Signed in" / "Signed out" with its kind. */
    fun youtubeBadge(r: JsonObject): Pair<String, String> {
        val source = r.str("source")
        if (source.isEmpty()) return "No cookies" to ""
        val ok = r.obj("sources")?.obj(source)?.bool("signed_in") == true
        return if (ok) "Signed in" to "ok" else "Signed out" to "warn"
    }

    /** "Last refresh 2 h ago · next refresh in 2 h" ("" without a source). */
    fun youtubeWhen(r: JsonObject, now: Double): String {
        if (r.str("source").isEmpty()) return ""
        val next = r.dbl("next_refresh")
        return "Last refresh ${ago(r.dbl("last_refresh"), now)}" + if (next != 0.0) " · next refresh ${until(next, now)}" else ""
    }

    /** What Refresh now says: (toast, kind). */
    fun youtubeRefreshToast(r: JsonObject): Pair<String, String> {
        val ok = r.bool("refreshed")
        val rotated = r["rotated"] as? JsonPrimitive
        val nothingRenewed = rotated != null && !rotated.isString && rotated.content.toDoubleOrNull() == 0.0
        val trouble = r.obj("sources")?.obj(r.str("source"))?.str("message").orEmpty()
        return when {
            !ok -> "YouTube signed out" to "err"
            trouble.isNotEmpty() -> trouble to "warn"
            nothingRenewed -> "Still signed in – Google sent no renewed cookies, so nothing was refreshed" to "warn"
            else -> "Refreshed – still signed in" to "ok"
        }
    }

    // ---- Connections (connections.rs) ------------------------------------------------------------------------

    /** What a service does and which connect form it uses ("" for a service this app does not know). */
    fun connectionInfo(k: String): Pair<String, String> = when (k) {
        "jellyfin" -> "Two-way favourites (Finamp ♥) and playlist sync" to "userpass"
        "plex" -> "Two-way loves (Plexamp ♥) and playlist sync" to "plex"
        "navidrome" -> "Two-way stars and playlist sync" to "userpass"
        "ytmusic" -> "Two-way likes (👍) and playlist sync; import your library & Liked Music" to "ytmusic"
        "spotify" -> "Import private playlists & Liked Songs; likes flow into Encore" to "oauth"
        "tidal" -> "Import playlists & collection; likes flow into Encore" to "oauth"
        "deezer" -> "Import your public favourites & playlists" to "profile"
        "listenbrainz" -> "Scrobbles what you play here; two-way loves; playlists pushed (needs MusicBrainz-tagged tracks); For you recommendations" to "token"
        "lastfm" -> "Scrobbles what you play here; imports loved tracks; For you recommendations" to "lastfm"
        else -> "" to ""
    }

    /** Where a service keeps its liked songs (the Import Liked Songs button); null for none. */
    fun likedUrl(svc: String): String? = when (svc) {
        "spotify" -> "https://open.spotify.com/collection/tracks"
        "ytmusic" -> "https://music.youtube.com/playlist?list=LM"
        else -> null
    }

    /** "Synced: +2 / −0 likes, 3 changes pushed, 1 playlist(s) · 1 error(s)" and whether it had errors. */
    fun syncToast(r: JsonObject): Pair<String, Boolean> {
        val errors = truthy(r["errors"])
        return "Synced: +${r.str("liked_added")} / −${r.str("liked_removed")} likes, ${r.str("pushed")} changes pushed, ${r.str("playlists")} playlist(s)" +
            (if (errors) " · ${r.str("errors")} error(s)" else "") to errors
    }

    /**
     * Whether a sign-in in a browser tab has finished, from the service's connection (null: none). Last.fm can be
     * connected by username only before the sign-in starts; it is done only once it can scrobble (the server saves
     * the session key then, sync/connections.rs `public`).
     */
    fun signInDone(svc: String, conn: JsonElement?): Boolean {
        val c = conn as? JsonObject ?: return false
        return svc != "lastfm" || truthy(c["can_scrobble"])
    }

    /** How long the phone waits for a sign-in in a browser tab: Plex 4 minutes (120 checks 2 s apart, as the web), the others 2. */
    fun signInWaitMs(plex: Boolean): Long = if (plex) 240_000L else 120_000L

    /** A sign-in the admin has not set up yet: shown in a dialog, not a passing toast. */
    fun isSetupError(message: String): Boolean = message.contains("Public URL") || message.contains("Client ID")

    // ---- Diagnostics -----------------------------------------------------------------------------------------

    val CHECK_KINDS: List<Pair<String, String>> = listOf("service" to "Connections", "path" to "Folders & permissions", "tool" to "Tools in the container")

    /** App crash report kinds, the web's words. */
    fun crashKind(k: String): String = when (k) {
        "crash" -> "Crash"
        "native" -> "Native crash"
        "anr" -> "Not responding"
        "low_memory" -> "Closed for memory"
        "signaled" -> "Killed by the system"
        "web" -> "Web page"
        "" -> "Crash"
        else -> k
    }

    /** The Server block (`machine` of the diagnostics answer) as (label, value) rows; [size] formats bytes. */
    fun serverRows(s: JsonObject, size: (Double) -> String): List<Pair<String, String>> {
        fun folder(v: JsonObject?): String {
            if (v == null) return "not set"
            var t = v.str("fs").ifEmpty { "unknown file system" }
            if (v.containsKey("network")) t += if (v.bool("network")) " (network)" else " (local)"
            return if (v.str("path").isNotEmpty()) "${v.str("path")} · $t" else t
        }
        val memory = if (nullish(s, "memory")) "unknown" else size(s.dbl("memory")) + if (s.str("memory_source") == "cgroup") " (container limit)" else " (whole machine)"
        val n = s.dbl("db_readers").toLong()
        val readers = when (s.str("db_readers_source")) {
            "env" -> "$n (ENCORE_DB_READERS)"
            "network" -> "$n – off because the config folder is on a network file system (set ENCORE_DB_READERS to turn them on)"
            else -> "$n (default)"
        }
        return listOf(
            "CPUs" to if (nullish(s, "cpus")) "unknown" else s.str("cpus"),
            "Memory" to memory,
            "Config folder" to folder(s.obj("config")),
            "Music folder" to folder(s.obj("music")),
            "Database" to if (nullish(s, "database_bytes")) "unknown" else size(s.dbl("database_bytes")),
            "Translations" to if (s.bool("translations_on")) "on for at least one account" else "off for every account",
            "Database read connections" to readers,
        )
    }

    /** The VPN as one more connection check (diagnostics.rs run). */
    fun vpnCheck(v: JsonObject): JsonObject = JsonObject(
        mapOf(
            "kind" to JsonPrimitive("service"),
            "name" to JsonPrimitive("VPN for Soulseek"),
            "ok" to JsonPrimitive(!v.bool("enabled") || v.str("state") == "up"),
            "request" to JsonPrimitive(if (v.str("endpoint").isNotEmpty()) "WireGuard ${v.str("endpoint")}" else ""),
            "detail" to JsonPrimitive(vpnText(v)),
        ),
    )

    /** The plain-text report of Copy report. */
    fun diagnosticsReport(last: JsonObject, size: (Double) -> String): String {
        val lines = arrayListOf("Encore ${jsStr(last, "version")} diagnostics (uid ${jsStr(last, "uid")})")
        for (e in last.arr("checks") ?: JsonArray(emptyList())) {
            val c = e as? JsonObject ?: continue
            val ok = c.bool("ok")
            val status = if (truthy(c["status"])) " → HTTP ${jsStr(c, "status")}" else ""
            val ms = if (!nullish(c, "ms")) " (${jsStr(c, "ms")} ms)" else ""
            val body = if (truthy(c["body"]) && !ok) "\n     body: ${c.str("body").take(300)}" else ""
            lines.add("${if (ok) "OK  " else "FAIL"} ${jsStr(c, "name")} – ${c.str("request")}$status$ms\n     ${c.str("detail")}$body")
        }
        last.obj("machine")?.let { m ->
            lines.add("")
            lines.add("Server:")
            for ((k, v) in serverRows(m, size)) lines.add("  $k: $v")
        }
        lines.add("")
        lines.add("Settings:")
        for (e in last.arr("settings") ?: JsonArray(emptyList())) {
            val s = e as? JsonObject ?: continue
            lines.add("  ${jsStr(s, "key")} = ${jsStr(s, "value")}  [${if (s.str("source") == "env") "env ${jsStr(s, "env")}" else "saved"}]")
        }
        return lines.joinToString("\n")
    }

    // ---- the server's own update (diagnostics.rs, 0.28) ------------------------------------------------------

    /** After Restart server / Update now: /api/health every 2 s; late after 90 s; looking for 10 minutes in all. */
    const val POLL_MS = 2000L
    const val BACK_BUDGET_MS = 90_000L
    const val GIVE_UP_MS = 600_000L
    /** A restart so quick that no poll saw the server down: this long of answers in a row counts as back. */
    const val QUIET_BACK_MS = 30_000L

    enum class Wait { RESTART, UPDATE }

    sealed interface Back {
        data object Waiting : Back
        data class Running(val version: String) : Back
        data object SameVersion : Back
        data object Late : Back
    }

    /** What the polls so far say (diagnostics.rs judge). */
    fun judge(w: Wait, from: String, version: String?, down: Boolean, answeringForMs: Long, elapsedMs: Long): Back = when {
        w == Wait.RESTART && version != null && (down || answeringForMs >= QUIET_BACK_MS) -> Back.Running(version)
        w == Wait.UPDATE && version != null && version != from -> Back.Running(version)
        w == Wait.UPDATE && version != null && down -> Back.SameVersion
        elapsedMs >= BACK_BUDGET_MS -> Back.Late
        else -> Back.Waiting
    }

    /** Release notes (GitHub markdown) as plain lines for "What changed" (diagnostics.rs notes_text). */
    fun notesText(md: String): String {
        var text = md.replace("\r\n", "\n")
        while (true) {
            val a = text.indexOf("<!--")
            if (a < 0) break
            val e = text.indexOf("-->", a)
            text = text.substring(0, a) + if (e < 0) "" else text.substring(e + 3)
        }
        val out = ArrayList<String>()
        for (raw in text.split('\n')) {
            val body = raw.trimStart()
            val indent = (raw.length - body.length).coerceAtMost(8)
            var t = body.trimEnd()
            if (t.startsWith("```") || t.startsWith("~~~")) continue
            if (t.length >= 3 && t.all { it == '-' || it == '*' || it == '_' || it == ' ' } && t.count { it != ' ' } >= 3) t = ""
            if (t.startsWith("#")) t = t.trimStart('#').trimStart()
            while (t.startsWith(">")) t = t.substring(1).trimStart()
            var bullet = false
            for (m in listOf("* ", "- ", "+ ")) {
                if (t.startsWith(m)) { t = t.substring(m.length).trimStart(); bullet = true; break }
            }
            val line = inlineText(t)
            if (line.isBlank()) {
                if (out.isNotEmpty() && out.last().isNotEmpty()) out.add("")
                continue
            }
            out.add(" ".repeat(indent) + (if (bullet) "• " else "") + line.trimEnd())
        }
        while (out.isNotEmpty() && out.last().isEmpty()) out.removeAt(out.lastIndex)
        return out.joinToString("\n")
    }

    private fun inlineText(s: String): String {
        val c = s.toCharArray()
        val out = StringBuilder()
        var i = 0
        while (i < c.size) {
            val at = if (c[i] == '!' && i + 1 < c.size && c[i + 1] == '[') i + 1 else i
            if (c[at] == '[') {
                val close = (at + 1 until c.size).firstOrNull { c[it] == ']' }
                if (close != null && close + 1 < c.size && c[close + 1] == '(') {
                    val end = (close + 2 until c.size).firstOrNull { c[it] == ')' }
                    if (end != null) {
                        out.append(inlineText(String(c, at + 1, close - at - 1)))
                        i = end + 1
                        continue
                    }
                }
            }
            if (c[i] == '<') {
                val end = (i + 1 until c.size).firstOrNull { c[it] == '>' }
                if (end != null) {
                    val inner = String(c, i + 1, end - i - 1)
                    if (inner.startsWith("http://") || inner.startsWith("https://")) {
                        out.append(inner)
                        i = end + 1
                        continue
                    }
                }
            }
            if (c[i] != '*' && c[i] != '`') out.append(c[i])
            i++
        }
        return out.toString()
    }

    // ---- Scheduled tasks (tasks.rs) --------------------------------------------------------------------------

    private val EVERY: List<Pair<Long, String>> = listOf(
        15L to "15 minutes", 30L to "30 minutes", 60L to "hour", 180L to "3 hours", 360L to "6 hours", 720L to "12 hours", 1440L to "day", 10080L to "week",
    )

    /** The "every" choices of a task: the usual ones and its own interval, in order. */
    fun everyOptions(minutes: Double): List<Pair<String, String>> {
        val ms = EVERY.map { it.first.toDouble() }.toMutableList()
        if (minutes !in ms) ms.add(minutes)
        ms.sort()
        return ms.map { m -> num(m) to (EVERY.firstOrNull { it.first.toDouble() == m }?.second ?: "${num(m)} minutes") }
    }

    /** "Last run 3 h ago · took 12 s · running… · result · next in 2 h" (the result or the error separately). */
    fun taskLine(t: JsonObject, now: Double): String {
        val running = t.bool("running")
        val took = if (truthy(t["duration"])) {
            val d = t.dbl("duration")
            " · took " + if (d < 60) "${jsRound(d)} s" else "${jsRound(d / 60)} min"
        } else ""
        val next = if (t.bool("enabled") && truthy(t["next_run"]) && !running) " · next ${until(t.dbl("next_run"), now)}" else ""
        val result = if (t.str("error").isEmpty() && t.str("result").isNotEmpty() && !running) " · ${t.str("result")}" else ""
        return "Last run ${ago(t.dbl("last_run"), now)}$took${if (running) " · running…" else ""}$result$next"
    }

    // ---- Users, invites ----------------------------------------------------------------------------------------

    /** What new members start with (users.rs NEW). */
    val NEW_MEMBER: List<Pair<String, String>> = listOf(
        "new_user_auto_approve" to "Requests skip the approval queue",
        "new_user_auto_approve_playlists" to "Playlist downloads skip the approval queue",
        "new_user_pick" to "Pick the exact version to download",
        "new_user_fix" to "Fix their own downloads",
        "new_user_storage" to "See server storage",
        "new_user_auto_download" to "Auto-download (likes and followed playlists)",
    )

    /** `parseInt(s, 10) || 0`. */
    fun parseIntOrZero(s: String): Long {
        val t = s.trimStart()
        val neg = t.startsWith("-")
        val rest = t.removePrefix("-").removePrefix("+")
        val n = rest.takeWhile { it.isDigit() }.toLongOrNull() ?: 0L
        return if (neg) -n else n
    }

    /** `+s` as JSON.stringify sends it: a number, 0 for "", null for anything that is not a number. */
    fun jsPlus(s: String): JsonElement {
        val t = s.trim()
        if (t.isEmpty()) return JsonPrimitive(0)
        val d = t.toDoubleOrNull()?.takeIf { it.isFinite() } ?: return JsonNull
        return if (d == floor(d) && kotlin.math.abs(d) < 1e15) JsonPrimitive(d.toLong()) else JsonPrimitive(d)
    }

    /** An invite that can no longer be used (all uses taken or expired). */
    fun inviteDead(i: JsonObject, now: Double): Boolean =
        i.dbl("uses") >= i.dbl("max_uses") || (truthy(i["expires_at"]) && i.dbl("expires_at") < now)

    /** `<base>/#/signup?invite=<code>`. */
    fun inviteLink(base: String, code: String): String =
        base.trimEnd('/') + "/#/signup?invite=" + java.net.URLEncoder.encode(code, "UTF-8").replace("+", "%20")

    // ---- Blocked (report.rs) -----------------------------------------------------------------------------------

    /** A report's status words and badge kind. */
    fun reportStatus(s: String): Pair<String, String> = when (s) {
        "replacing" -> "Looking for a replacement" to "warn"
        "replaced" -> "Replaced" to "ok"
        "no_replacement" -> "No replacement yet" to "err"
        "dismissed" -> "Dismissed" to ""
        else -> s to ""
    }

    /** The reason a quality-check flag suggests. */
    fun reasonFor(codes: List<String>): String = when {
        "fake_lossless" in codes || "upsampled" in codes || "low_bitrate" in codes -> "fake_lossless"
        "quiet" in codes -> "loudness"
        "wrong_length" in codes -> "wrong_version"
        else -> "bad_audio"
    }

    // ---- reading answers -----------------------------------------------------------------------------------------

    /** JavaScript truthiness of a JSON value. */
    fun truthy(v: JsonElement?): Boolean = SchemaRules.truthy(v)

    /** A value as text ("" for null). */
    fun text(v: JsonElement?): String = SchemaRules.textOf(v)

    /** A missing key or null (`== null` in JavaScript). */
    fun nullish(o: JsonObject, key: String): Boolean = o[key] == null || o[key] is JsonNull

    /** `String(obj[key])` as JavaScript prints it (null → "null", missing → "undefined"). */
    fun jsStr(o: JsonObject, key: String): String = when (val v = o[key]) {
        null -> "undefined"
        is JsonNull -> "null"
        else -> text(v)
    }
}
