package app.encore.android.features.settings

import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.decodeModel
import app.encore.android.core.obj
import app.encore.android.core.objectsOf
import app.encore.android.core.parseJson
import app.encore.android.core.str
import app.encore.android.core.strings
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * What Settings → Server → Plugins (0.32) needs without Android: the words of a plugin's card, its badges, status
 * line and buttons, the checks before an upload or a URL install, the settings card of a plugin (its fields in runs
 * under their `part`, the values it shows) and the download client notes. The web's page is
 * web-rs/src/settings/plugins.rs (the same rules and words); the server's answers are /api/plugins
 * (server/crates/api/src/routes/plugins.rs, the cards made by encore_plugins::Registry).
 *
 * A card (`c`) is one entry of GET /api/plugins `plugins`, or the answer of GET /api/plugins/{id} and of the
 * actions: id, name, description, kind, version, installed, installed_version, enabled, origin, trust, builtin,
 * warning, auto_update, can_update, latest, update_available, last_error, status, busy, sandbox, network, hosts,
 * reads_library, kinds, lossless, settings_on, has_settings, homepage, license.
 */
object PluginRules {
    /** The cards of the Downloads page that became plugin cards in 0.32: (card, plugin). YouTube sign-in is a part of yt-dlp's. */
    val MOVED: List<Pair<String, String>> = listOf(
        "streamrip" to "streamrip", "torznab" to "torznab", "slskd" to "slskd", "ytdlp" to "ytdlp", "youtube" to "ytdlp",
    )

    /** Other `?s=` names that open the Plugins page: the page, the old Download tools card, and the catalogue plugins with no other page. */
    val LINKS: List<String> = listOf(
        "plugins", "tools", "qbittorrent", "transmission", "deluge", "rtorrent", "torrent-blackhole", "sabnzbd", "nzbget",
        "usenet-blackhole", "translator", "jellyfin", "plex", "navidrome",
    )

    /** A download client plugin, by the choice of the Torrents & Usenet card that sends downloads to it. */
    data class Client(val select: String, val value: String, val plugin: String)

    val CLIENTS: List<Client> = listOf(
        Client("torrent_client", "qbittorrent", "qbittorrent"), Client("torrent_client", "transmission", "transmission"),
        Client("torrent_client", "deluge", "deluge"), Client("torrent_client", "rtorrent", "rtorrent"),
        Client("torrent_client", "blackhole", "torrent-blackhole"),
        Client("nzb_client", "sabnzbd", "sabnzbd"), Client("nzb_client", "nzbget", "nzbget"), Client("nzb_client", "blackhole", "usenet-blackhole"),
    )

    /** Whether a card of the settings schema is drawn on the Plugins page instead of its own page. */
    fun moved(card: String): Boolean = MOVED.any { it.first == card }

    /** Whether `#/settings?s=<s>` opens the Plugins page. */
    fun opensPlugins(s: String): Boolean = moved(s) || s in LINKS

    /** The plugin card a `?s=` name asks for: a moved card's plugin, or the name itself. */
    fun cardFor(s: String): String = MOVED.firstOrNull { it.first == s }?.second ?: s

    /**
     * Whether GET /api/plugins says this server has plugins: true for a JSON answer with its list, false for a 404 or
     * an answer that is not one (a server before 0.32 answers the address with its web page), null when it could
     * not tell (signed out, not an admin, the server not reached).
     */
    fun hasPlugins(code: Int, text: String?): Boolean? = when {
        code == 404 -> false
        code !in 200..299 -> null
        else -> (parseJson(text) as? JsonObject)?.get("plugins") is kotlinx.serialization.json.JsonArray
    }

    // ---- words ---------------------------------------------------------------------------------------------

    /** A plugin's kind in a badge. */
    fun kindLabel(kind: String): String = when (kind) {
        "source" -> "Source"
        "client" -> "Download client"
        "tool" -> "Tool"
        "playback" -> "Playback"
        "connection" -> "Connection"
        "" -> "Plugin"
        else -> kind.replaceFirstChar { it.uppercase() }
    }

    /** "songs, albums and links": what a source finds (`kinds`). */
    fun kindsWords(kinds: List<String>): String {
        val words = kinds.mapNotNull { when (it) { "track" -> "songs"; "album" -> "albums"; "url" -> "links"; else -> null } }
        return when (words.size) {
            0 -> ""
            1 -> words[0]
            else -> words.dropLast(1).joinToString(", ") + " and " + words.last()
        }
    }

    /** The version a card shows: the installed one (the program's), else the plugin's. */
    fun versionText(c: JsonObject): String =
        if (c.bool("installed") && c.str("installed_version").isNotEmpty()) c.str("installed_version") else c.str("version")

    /** The name of a plugin, its id when it has none. */
    fun nameOf(c: JsonObject): String = c.str("name").ifEmpty { c.str("id") }

    /** What the page (or the server, `busy`) is doing to a plugin, in a few words. */
    fun busyWords(what: String): String = when (what) {
        "install", "installing" -> "Installing…"
        "update", "updating" -> "Updating…"
        "remove", "removing" -> "Removing…"
        "enable" -> "Enabling…"
        "disable" -> "Disabling…"
        "test" -> "Testing…"
        else -> "Working…"
    }

    /** The warning before a plugin from a zip, an address or GitHub is installed, for the sandbox level here. */
    fun trustText(full: Boolean): String = if (full) {
        "This plugin is not made by Encore. It runs in full isolation: it sees only its own folders, and the network only when its plugin.toml asks for it. Install only plugins you trust."
    } else {
        "This plugin is not made by Encore. This server can only set limits on it (see the compose option in the README): it runs as Encore's user and can read what Encore can. Install only plugins you trust."
    }

    /** The toast after an action on a plugin ([before] and [after]: its card). */
    fun doneWords(act: String, before: JsonObject, after: JsonObject): String {
        val name = after.str("name").ifEmpty { before.str("name") }
        val v = versionText(after)
        return when (act) {
            "install" -> if (after.str("installed_version").isNotEmpty()) "$name $v installed" else "$name installed"
            "update" -> if (before.str("installed_version") == after.str("installed_version")) "$name is up to date" else "$name updated to $v"
            "remove" -> "$name removed"
            "enable" -> "$name enabled"
            "disable" -> "$name disabled"
            else -> "Done"
        }
    }

    /** The toast after an upload or a URL install: "Name 1.2 installed". */
    fun addedWords(c: JsonObject): String = listOf(c.str("name"), versionText(c)).filter { it.isNotEmpty() }.joinToString(" ") + " installed"

    /** The toast of Check for updates. */
    fun checkWords(updates: Int): String = when (updates) {
        0 -> "No updates found"
        1 -> "1 update available"
        else -> "$updates updates available"
    }

    /** Remove's question: a catalogue plugin keeps its settings, one added from a zip or an address loses everything. */
    fun removeText(c: JsonObject): String {
        val name = nameOf(c)
        return if (c.str("origin") == "catalog") {
            "Remove $name? Encore stops using it until you install it again. What the plugin downloaded is deleted; your settings for it are kept."
        } else {
            "Remove $name? Its program, data, log and settings are deleted."
        }
    }

    /** The auto-update switch's words. */
    fun autoUpdateWords(id: String): String =
        if (id == "ytdlp") "Update automatically (recommended – sites change often)" else "Update automatically when a new release is out"

    /** A failed call as one text: the server's sentence and, for a refused manifest, each problem on its own line. */
    fun errorText(message: String?, body: JsonObject?): String {
        val problems = body?.arr("problems").strings().filter { it.isNotBlank() }
        val head = if (message.isNullOrBlank()) "Failed" else message
        return if (problems.isEmpty()) head else head + "\n" + problems.joinToString("\n") { "- $it" }
    }

    /** "1 m 30 s" (the web's apps.rs `span`). */
    fun span(secs: Double): String {
        val s = Math.round(maxOf(0.0, secs))
        val h = s / 3600
        val m = (s % 3600) / 60
        val r = s % 60
        return when {
            h == 0L && m == 0L -> "$r s"
            h == 0L && r == 0L -> "$m m"
            h == 0L -> "$m m $r s"
            m == 0L -> "$h h"
            else -> "$h h $m m"
        }
    }

    /** What a Test answered (`{ok, message}` or `{ok: false, error, code, retry_after}`), as its line. */
    fun testAnswer(r: JsonObject): ResultLine {
        val ok = r.bool("ok")
        var msg = r.str("message").ifEmpty { r.str("error") }
        if (!ok && r.str("code") == "rate_limited" && r.dbl("retry_after") > 0) msg = "$msg – try again in ${span(r.dbl("retry_after"))}"
        if (msg.isBlank()) msg = if (ok) "Test passed" else "Test failed"
        return ResultLine(ok, msg)
    }

    /** Why an address for Install from URL cannot be used, before it is sent. */
    fun urlProblem(input: String): String? {
        val s = input.trim()
        if (s.isEmpty()) return "Enter an https address, or owner/repo@ref for GitHub"
        if (s.lowercase().startsWith("http://")) return "Plugins install only from https addresses"
        return null
    }

    /** Why a picked file cannot be uploaded, before it is sent ([bytes] < 0: its size is not known). */
    fun fileProblem(name: String, bytes: Long, maxMb: Long): String? {
        if (!name.lowercase().endsWith(".zip")) return "A plugin is uploaded as a .zip file"
        if (bytes > maxOf(1L, maxMb) * 1024L * 1024L) return "The plugin zip is larger than $maxMb MB"
        return null
    }

    /** The upload limit GET /api/plugins gives (200 MB when it does not say). */
    fun uploadMaxMb(list: JsonObject?): Long = list?.dbl("upload_max_mb")?.toLong()?.takeIf { it > 0 } ?: 200L

    /** An address the page may open: http(s) only (a plugin's homepage is the plugin's own text). */
    fun safeLink(u: String): Boolean = u.trim().lowercase().let { it.startsWith("https://") || it.startsWith("http://") }

    // ---- the sandbox line --------------------------------------------------------------------------------------

    /** The sandbox line at the top of the page (`sandbox` of GET /api/plugins). */
    data class Sandbox(val full: Boolean, val text: String, val what: String, val reason: String)

    fun sandbox(sb: JsonObject?): Sandbox {
        val full = sb?.str("level") == "full"
        val text = sb?.str("text")?.ifEmpty { null } ?: if (full) "Full isolation" else "Limits only, see the compose option"
        val what = if (full) {
            "Plugins you add see only their own folders, and the network only when they ask for it."
        } else {
            "Plugins you add run as Encore's user with limits on memory, processor time and processes; the compose option in the README gives them full isolation."
        }
        return Sandbox(full, text, what, sb?.str("reason")?.trim().orEmpty())
    }

    // ---- a card ------------------------------------------------------------------------------------------------

    /** A badge: its words and tone ("" plain, "warn", "info", "ok"). */
    data class BadgeText(val text: String, val kind: String)

    /** The badges of a card or row: kind, third-party, off, an update. */
    fun badges(c: JsonObject): List<BadgeText> {
        val out = mutableListOf(BadgeText(kindLabel(c.str("kind")), ""))
        if (isThirdParty(c)) out.add(BadgeText("Third-party", "warn"))
        val installed = c.bool("installed")
        if (installed && !c.bool("enabled")) out.add(BadgeText("Off", ""))
        if (installed && c.bool("update_available") && c.str("latest").isNotEmpty()) {
            out.add(BadgeText("${c.str("latest")} available", "info"))
        } else if (installed && c.str("latest").isNotEmpty()) {
            out.add(BadgeText("Up to date", "ok"))
        }
        return out
    }

    /** Not made by Encore: uploaded, installed from an address or from GitHub. */
    fun isThirdParty(c: JsonObject): Boolean = c.str("trust") == "third-party" || (c.str("origin").isNotEmpty() && c.str("origin") != "catalog")

    /** The dot beside an installed plugin's name: "ok", "err" or "" (off, or not known yet); null when not installed. */
    fun dotTone(c: JsonObject): String? {
        if (!c.bool("installed")) return null
        val st = c.obj("status")
        return when {
            !c.bool("enabled") -> ""
            c.str("last_error").isNotEmpty() || (st != null && !st.bool("ok")) -> "err"
            st != null && st.bool("ok") -> "ok"
            else -> ""
        }
    }

    /** The status line of a plugin: what it is doing, whether it works ([error]: red with Copy), and the last error. */
    data class Status(val text: String, val busy: Boolean = false, val muted: Boolean = false, val error: Boolean = false, val lastError: String = "")

    /** null for a plugin that is not installed and not busy. [acting]: what this page is doing to it. */
    fun status(c: JsonObject, acting: String?): Status? {
        val busy = acting ?: c.str("busy").ifEmpty { null }
        if (busy != null) return Status(busyWords(busy), busy = true, muted = true)
        if (!c.bool("installed")) return null
        val last = c.str("last_error")
        if (!c.bool("enabled")) return Status("Disabled – Encore does not use it until you enable it again", muted = true, lastError = last)
        val st = c.obj("status") ?: return Status("Checking…", busy = true, muted = true, lastError = last)
        return if (st.bool("ok")) {
            Status(st.str("message").ifEmpty { "Ready" }, lastError = last)
        } else {
            Status(st.str("message").ifEmpty { "Not working" }, error = true, lastError = last)
        }
    }

    /** The buttons of a card, in their order (an installed plugin), or Install (one that is not). */
    data class Action(val act: String, val label: String, val primary: Boolean = false, val danger: Boolean = false)

    fun actions(c: JsonObject, testLabel: String): List<Action> {
        if (!c.bool("installed")) return listOf(Action("install", "Install", primary = true))
        val out = mutableListOf<Action>()
        if (c.bool("can_update")) {
            out.add(
                if (c.bool("update_available") && c.str("latest").isNotEmpty()) Action("update", "Update to ${c.str("latest")}", primary = true)
                else Action("update", "Update"),
            )
        }
        out.add(Action("test", testLabel))
        out.add(Action("logs", "Logs"))
        out.add(if (c.bool("enabled")) Action("disable", "Disable") else Action("enable", "Enable", primary = true))
        out.add(Action("remove", "Remove", danger = true))
        return out
    }

    /** The line about a plugin: what it finds, its sandbox and permissions (a third-party plugin) and its licence (the homepage is a link of its own). */
    fun aboutParts(c: JsonObject): List<String> {
        val parts = mutableListOf<String>()
        if (c.str("kind") == "source") {
            val finds = kindsWords(c.arr("kinds").strings())
            if (finds.isNotEmpty()) parts.add("Finds $finds" + if (c.bool("lossless")) ", lossless" else "")
        }
        if (c.str("trust") == "third-party") {
            if (c.str("sandbox").isNotEmpty()) parts.add("Sandbox: ${c.str("sandbox")}")
            val hosts = c.arr("hosts").strings().filter { it.isNotEmpty() }
            parts.add(
                when {
                    !c.bool("network") -> "No network"
                    hosts.isEmpty() -> "Network: any address"
                    else -> "Network: ${hosts.joinToString(", ")}"
                },
            )
            if (c.bool("reads_library")) parts.add("Reads your library (read only)")
        }
        if (c.str("license").isNotEmpty()) parts.add("Licence ${c.str("license")}")
        return parts
    }

    /** The homepage a card links to, or "" (only http and https addresses). */
    fun homepage(c: JsonObject): String = c.str("homepage").trim().takeIf { safeLink(it) }.orEmpty()

    /** An installed plugin with a settings card of its own here (the others say where their settings are, or have none). */
    fun hasCard(c: JsonObject): Boolean = c.bool("installed") && c.bool("has_settings") && c.str("settings_on").isEmpty()

    /** The Test button's words: those of the card the plugin's settings were before 0.32, or "Test". */
    fun testLabel(schema: SettingsSchema?, id: String): String =
        schema?.group(id)?.takeIf { it.test.isNotEmpty() && it.testLabel.isNotEmpty() }?.testLabel ?: "Test"

    /** "Settings → Server processing": the page a plugin's settings are on ([on]: its `settings_on`). */
    fun settingsOnTitle(schema: SettingsSchema?, on: String): String = schema?.section(on)?.title?.ifEmpty { null } ?: on

    /** The number of installed plugins with an update in an answer of POST /api/plugins/check. */
    fun updates(list: JsonObject): Int = list.arr("plugins").objectsOf().count { it.bool("installed") && it.bool("update_available") }

    /** Whether the installed plugins of [list] differ from [shown] (an install or remove elsewhere: the page is drawn again). */
    fun changedShape(list: JsonObject, shown: Set<String>): Boolean =
        list.arr("plugins").objectsOf().any { it.bool("installed") != (it.str("id") in shown) }

    // ---- the settings of a card --------------------------------------------------------------------------------

    /** A run of a card's fields under the heading of their `part` ("" for none): a part holds its field and the ones after it. */
    data class Run(val part: String, val fields: List<SchemaField>)

    /**
     * The fields of a settings answer (GET /api/plugins/{id}/settings) in runs under their part. A `when` rule naming a
     * key that is not on the card (a client's fields name the Torrents & Usenet card's choice) is left out: the
     * client's card is shown whole.
     */
    fun runs(answer: JsonObject): List<Run> {
        val raw = answer.arr("fields").objectsOf()
        val keys = raw.map { it.str("key") }.toSet()
        val out = mutableListOf<Run>()
        var part = ""
        for (o in raw) {
            val f = fieldOf(o, keys) ?: continue
            if (o.str("part").isNotEmpty()) part = o.str("part")
            val last = out.lastOrNull()
            if (last != null && last.part == part) out[out.lastIndex] = last.copy(fields = last.fields + f) else out.add(Run(part, listOf(f)))
        }
        return out
    }

    /** One field of a plugin card, its `when` left out when it names a key the card does not have. */
    fun fieldOf(o: JsonObject, keys: Set<String>): SchemaField? {
        val f = o.decodeModel(SchemaField.serializer()) ?: return null
        val w = SchemaRules.parseWhen(f.rule)
        return if (w != null && w.key !in keys) f.copy(rule = "") else f
    }

    /** Every field of a settings answer (for Save and Show advanced). */
    fun fields(answer: JsonObject): List<SchemaField> = runs(answer).flatMap { it.fields }

    /** A plugin card's settings as a [SchemaGroup] (for Save and the VPN block's save): its id is the plugin's. */
    fun groupOf(answer: JsonObject, title: String): SchemaGroup = SchemaGroup(id = answer.str("id"), title = title, fields = fields(answer))

    /**
     * The values a card's fields show, with `_meta` (where each comes from). A built-in plugin's keys are Encore's
     * settings: [settings] (GET /api/admin/settings) has them; a key it lacks is taken from the answer. A plugin's
     * own settings are only in its answer (saved secrets show as saved, none locked by the environment).
     */
    fun cardValues(answer: JsonObject, settings: JsonObject?): JsonObject {
        val builtin = answer.bool("builtin")
        val values = LinkedHashMap<String, JsonElement>()
        if (builtin && settings != null) values.putAll(settings)
        val meta = LinkedHashMap<String, JsonElement>((values["_meta"] as? JsonObject).orEmpty())
        val given = answer.obj("values")
        for (f in answer.arr("fields").objectsOf()) {
            val key = f.str("key")
            if (key.isEmpty()) continue
            if (!values.containsKey(key)) values[key] = given?.get(key) ?: JsonNull
            if (!meta.containsKey(key)) {
                val source = if (f.bool("env_locked")) "env" else if (builtin) "default" else "ui"
                meta[key] = JsonObject(mapOf("source" to JsonPrimitive(source), "env" to (f["env"] ?: JsonNull)))
            }
        }
        values["_meta"] = JsonObject(meta)
        return JsonObject(values)
    }

    /** A part's heading from the settings schema: a part that was a card of its own before 0.32 (YouTube sign-in) keeps its description and Test. */
    data class PartInfo(val desc: String, val test: String, val testLabel: String, val card: String)

    fun partInfo(schema: SettingsSchema?, part: String): PartInfo {
        val g = schema?.sections?.asSequence()?.flatMap { it.groups.asSequence() }?.firstOrNull { it.title == part }
        return PartInfo(g?.desc.orEmpty(), g?.test.orEmpty(), g?.testLabel?.ifEmpty { null } ?: "Test", g?.id.orEmpty())
    }

    // ---- download clients --------------------------------------------------------------------------------------

    /** A client card's note: in use (muted) or not (warning). */
    data class Note(val inUse: Boolean, val text: String)

    fun clientNoteText(name: String, selectLabel: String, optionLabel: String, chosen: Boolean, chosenLabel: String): Note =
        if (chosen) {
            Note(true, "In use: $selectLabel is set to $optionLabel on the Torrents & Usenet card.")
        } else {
            Note(false, "Not in use: $selectLabel is set to ${chosenLabel.ifEmpty { "something else" }} on the Torrents & Usenet card. Choose $optionLabel there to send downloads to $name.")
        }

    private fun schemaField(schema: SettingsSchema?, key: String): SchemaField? =
        schema?.sections?.asSequence()?.flatMap { it.groups.asSequence() }?.flatMap { it.fields.asSequence() }?.firstOrNull { it.key == key }

    /** A client card's note from its fields' `when` rule (null for a card without one). */
    fun clientNote(c: JsonObject, answer: JsonObject, settings: JsonObject, schema: SettingsSchema?): Note? {
        val rule = answer.arr("fields").objectsOf().mapNotNull { SchemaRules.parseWhen(it.str("when")) }.firstOrNull { it.equal } ?: return null
        val select = schemaField(schema, rule.key)
        val words = { v: String -> select?.options?.firstOrNull { it.value == v }?.label ?: v }
        val chosen = SchemaRules.textOf(settings[rule.key])
        return clientNoteText(nameOf(c), select?.label ?: rule.key, words(rule.value), chosen == rule.value, words(chosen))
    }

    /** A client the Torrents & Usenet card chose whose plugin is not installed or is off: the line and its button. */
    data class ChosenClient(val text: String, val act: String, val plugin: String)

    /** The Torrents & Usenet card: each chosen client has a plugin of its own, which has to be installed and on. */
    fun chosenClients(settings: JsonObject, plugins: List<JsonObject>, schema: SettingsSchema?): List<ChosenClient> =
        listOf("torrent_client", "nzb_client").mapNotNull { key ->
            val v = SchemaRules.textOf(settings[key])
            val client = CLIENTS.firstOrNull { it.select == key && it.value == v } ?: return@mapNotNull null
            val c = plugins.firstOrNull { it.str("id") == client.plugin } ?: return@mapNotNull null
            val f = schemaField(schema, key)
            val label = f?.label ?: key
            val chosen = f?.options?.firstOrNull { it.value == v }?.label ?: v
            when {
                !c.bool("installed") -> ChosenClient("$label: $chosen has a plugin of its own, not installed yet.", "install", client.plugin)
                !c.bool("enabled") -> ChosenClient("$label: the $chosen plugin is disabled.", "enable", client.plugin)
                else -> null
            }
        }
}
