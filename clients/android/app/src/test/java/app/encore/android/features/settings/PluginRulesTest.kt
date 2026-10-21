package app.encore.android.features.settings

import app.encore.android.core.EncoreJson
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.File

/** Settings → Plugins (PluginRules.kt): the same rules and words as web-rs/src/settings/plugins.rs. */
class PluginRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json).jsonObject

    /** The schema the web's tests use, recorded from GET /api/admin/settings/schema (null when the repo is not there). */
    private fun recordedSchema(): SettingsSchema? =
        listOf("../../../web-rs", "../../web-rs", "web-rs", "../web-rs")
            .map { File(it, "src/settings/fixtures/settings-schema.json") }
            .firstOrNull { it.isFile }
            ?.let { EncoreJson.decodeFromString(SettingsSchema.serializer(), it.readText()) }

    @Test
    fun oldLinksOpenThePluginsPage() {
        for (card in listOf("streamrip", "torznab", "slskd", "ytdlp", "youtube")) assertTrue(card, PluginRules.moved(card))
        assertFalse(PluginRules.moved("pipeline"))
        assertFalse(PluginRules.moved("releases"))
        for (s in listOf("plugins", "tools", "qbittorrent", "translator", "navidrome", "usenet-blackhole", "slskd")) assertTrue(s, PluginRules.opensPlugins(s))
        assertFalse(PluginRules.opensPlugins("downloads"))
        assertFalse(PluginRules.opensPlugins("cadenza")) // its settings stay on Server processing
        assertEquals("ytdlp", PluginRules.cardFor("youtube"))
        assertEquals("slskd", PluginRules.cardFor("slskd"))
        assertEquals("qbittorrent", PluginRules.cardFor("qbittorrent"))
        // every choice of the Torrents & Usenet card has its client plugin
        assertEquals(8, PluginRules.CLIENTS.size)
        assertEquals("torrent-blackhole", PluginRules.CLIENTS.first { it.select == "torrent_client" && it.value == "blackhole" }.plugin)
    }

    @Test
    fun theProbeTellsAnOlderServer() {
        assertEquals(true, PluginRules.hasPlugins(200, """{"sandbox":{"level":"full"},"plugins":[]}"""))
        assertEquals(false, PluginRules.hasPlugins(404, """{"detail":"Not Found"}"""))
        assertEquals(false, PluginRules.hasPlugins(200, "<!doctype html><html></html>"))
        assertEquals(false, PluginRules.hasPlugins(200, """{"items":[]}"""))
        assertNull(PluginRules.hasPlugins(403, """{"detail":"Admins only"}"""))
        assertNull(PluginRules.hasPlugins(502, ""))
    }

    @Test
    fun wordsOfACard() {
        assertEquals("Download client", PluginRules.kindLabel("client"))
        assertEquals("Plugin", PluginRules.kindLabel(""))
        assertEquals("Lyrics", PluginRules.kindLabel("lyrics"))
        assertEquals("songs, albums and links", PluginRules.kindsWords(listOf("track", "album", "url")))
        assertEquals("albums", PluginRules.kindsWords(listOf("album", "other")))
        assertEquals("", PluginRules.kindsWords(emptyList()))
        val c = obj("""{"id":"ytdlp","name":"yt-dlp","version":"0.32.0","installed":true,"installed_version":"2026.10.14","enabled":true}""")
        assertEquals("2026.10.14", PluginRules.versionText(c))
        assertEquals("0.32.0", PluginRules.versionText(obj("""{"version":"0.32.0","installed":false,"installed_version":null}""")))
        assertEquals("Update automatically (recommended – sites change often)", PluginRules.autoUpdateWords("ytdlp"))
        assertEquals("Update automatically when a new release is out", PluginRules.autoUpdateWords("slskd"))
        assertEquals("Installing…", PluginRules.busyWords("installing"))
        assertEquals("Testing…", PluginRules.busyWords("test"))
    }

    @Test
    fun toastsAndQuestions() {
        val before = obj("""{"name":"slskd","installed_version":"0.24.0","origin":"catalog"}""")
        assertEquals("slskd 0.24.1 installed", PluginRules.doneWords("install", before, obj("""{"name":"slskd","installed":true,"installed_version":"0.24.1"}""")))
        assertEquals("Torznab installed", PluginRules.doneWords("install", before, obj("""{"name":"Torznab","installed":true}""")))
        assertEquals("slskd updated to 0.24.1", PluginRules.doneWords("update", before, obj("""{"name":"slskd","installed":true,"installed_version":"0.24.1"}""")))
        assertEquals("slskd is up to date", PluginRules.doneWords("update", before, obj("""{"name":"slskd","installed":true,"installed_version":"0.24.0"}""")))
        assertEquals("slskd disabled", PluginRules.doneWords("disable", before, obj("{}")))
        assertEquals("Internet Archive 0.1.0 installed", PluginRules.addedWords(obj("""{"name":"Internet Archive","installed":true,"installed_version":"0.1.0"}""")))
        assertEquals("No updates found", PluginRules.checkWords(0))
        assertEquals("1 update available", PluginRules.checkWords(1))
        assertEquals("3 updates available", PluginRules.checkWords(3))
        assertTrue(PluginRules.removeText(before).endsWith("your settings for it are kept."))
        assertEquals("Remove IA? Its program, data, log and settings are deleted.", PluginRules.removeText(obj("""{"name":"IA","origin":"upload"}""")))
        assertTrue(PluginRules.trustText(true).contains("full isolation"))
        assertTrue(PluginRules.trustText(false).contains("can read what Encore can"))
    }

    @Test
    fun failuresAndTests() {
        val refused = obj("""{"detail":"The plugin.toml has problems","problems":["[plugin] id is missing","[run] shell is not allowed"]}""")
        assertEquals("The plugin.toml has problems\n- [plugin] id is missing\n- [run] shell is not allowed", PluginRules.errorText("The plugin.toml has problems", refused))
        assertEquals("Failed", PluginRules.errorText("", null))
        assertEquals("1 m 30 s", PluginRules.span(90.0))
        assertEquals("45 s", PluginRules.span(45.0))
        assertEquals("2 m", PluginRules.span(120.0))
        assertEquals("1 h 1 m", PluginRules.span(3661.0))
        assertEquals(ResultLine(true, "yt-dlp 2026.10.14 runs"), PluginRules.testAnswer(obj("""{"ok":true,"message":"yt-dlp 2026.10.14 runs"}""")))
        assertEquals(ResultLine(true, "Test passed"), PluginRules.testAnswer(obj("""{"ok":true}""")))
        assertEquals(
            ResultLine(false, "Too many searches – try again in 1 m 30 s"),
            PluginRules.testAnswer(obj("""{"ok":false,"error":"Too many searches","code":"rate_limited","retry_after":90}""")),
        )
        assertEquals(ResultLine(false, "Test failed"), PluginRules.testAnswer(obj("""{"ok":false}""")))
    }

    @Test
    fun checksBeforeSending() {
        assertEquals("Enter an https address, or owner/repo@ref for GitHub", PluginRules.urlProblem("  "))
        assertEquals("Plugins install only from https addresses", PluginRules.urlProblem("HTTP://example.org/p.zip"))
        assertNull(PluginRules.urlProblem("owner/repo@v1"))
        assertNull(PluginRules.urlProblem("https://example.org/p.zip"))
        assertEquals("A plugin is uploaded as a .zip file", PluginRules.fileProblem("plugin.tar.gz", 10, 200))
        assertEquals("The plugin zip is larger than 200 MB", PluginRules.fileProblem("p.ZIP", 200L * 1024 * 1024 + 1, 200))
        assertNull(PluginRules.fileProblem("owner-internet-archive-0.1.0.zip", 30_000, 200))
        assertNull(PluginRules.fileProblem("p.zip", -1, 200)) // the provider did not say: the server checks
        assertEquals(200L, PluginRules.uploadMaxMb(null))
        assertEquals(50L, PluginRules.uploadMaxMb(obj("""{"upload_max_mb":50}""")))
        assertTrue(PluginRules.safeLink("https://archive.org"))
        assertFalse(PluginRules.safeLink("javascript:alert(1)"))
        assertEquals("", PluginRules.homepage(obj("""{"homepage":"javascript:alert(1)"}""")))
    }

    @Test
    fun theSandboxLine() {
        val full = PluginRules.sandbox(obj("""{"level":"full","text":"Full isolation","reason":""}"""))
        assertTrue(full.full)
        assertEquals("Full isolation", full.text)
        val limits = PluginRules.sandbox(obj("""{"level":"limits","reason":"bwrap: No permissions to create a new namespace"}"""))
        assertFalse(limits.full)
        assertEquals("Limits only, see the compose option", limits.text)
        assertEquals("bwrap: No permissions to create a new namespace", limits.reason)
        assertTrue(limits.what.contains("compose option"))
    }

    @Test
    fun badgesDotAndStatus() {
        val third = obj("""{"id":"ia","kind":"source","installed":true,"enabled":true,"origin":"upload","trust":"third-party","latest":"0.2.0","update_available":true}""")
        assertEquals(
            listOf(PluginRules.BadgeText("Source", ""), PluginRules.BadgeText("Third-party", "warn"), PluginRules.BadgeText("0.2.0 available", "info")),
            PluginRules.badges(third),
        )
        val off = obj("""{"id":"slskd","kind":"source","installed":true,"enabled":false,"origin":"catalog","trust":"first-party","latest":"0.24.1"}""")
        assertEquals(listOf("Source", "Off", "Up to date"), PluginRules.badges(off).map { it.text })
        assertEquals(listOf("Tool"), PluginRules.badges(obj("""{"kind":"tool","installed":false,"origin":"catalog"}""")).map { it.text })

        assertNull(PluginRules.dotTone(obj("""{"installed":false}""")))
        assertEquals("", PluginRules.dotTone(off))
        assertEquals("ok", PluginRules.dotTone(obj("""{"installed":true,"enabled":true,"status":{"ok":true}}""")))
        assertEquals("err", PluginRules.dotTone(obj("""{"installed":true,"enabled":true,"status":{"ok":false}}""")))
        assertEquals("err", PluginRules.dotTone(obj("""{"installed":true,"enabled":true,"status":null,"last_error":"sha256 mismatch"}""")))
        assertEquals("", PluginRules.dotTone(obj("""{"installed":true,"enabled":true,"status":null}""")))

        assertEquals("Installing…", PluginRules.status(obj("""{"installed":false,"busy":"installing"}"""), null)!!.text)
        assertEquals("Testing…", PluginRules.status(third, "test")!!.text)
        assertNull(PluginRules.status(obj("""{"installed":false,"busy":null}"""), null))
        assertEquals("Disabled – Encore does not use it until you enable it again", PluginRules.status(off, null)!!.text)
        assertEquals("Checking…", PluginRules.status(obj("""{"installed":true,"enabled":true,"status":null}"""), null)!!.text)
        assertEquals("Ready", PluginRules.status(obj("""{"installed":true,"enabled":true,"status":{"ok":true}}"""), null)!!.text)
        val failed = PluginRules.status(obj("""{"installed":true,"enabled":true,"status":{"ok":false,"message":"could not reach archive.org"},"last_error":"update: sums missing"}"""), null)!!
        assertTrue(failed.error)
        assertEquals("could not reach archive.org", failed.text)
        assertEquals("update: sums missing", failed.lastError)
        assertEquals("Not working", PluginRules.status(obj("""{"installed":true,"enabled":true,"status":{"ok":false}}"""), null)!!.text)
    }

    @Test
    fun buttonsInTheirOrder() {
        assertEquals(listOf("install"), PluginRules.actions(obj("""{"installed":false}"""), "Test").map { it.act })
        val c = obj("""{"installed":true,"enabled":true,"can_update":true,"update_available":true,"latest":"2.0"}""")
        val a = PluginRules.actions(c, "Test & write config")
        assertEquals(listOf("update", "test", "logs", "disable", "remove"), a.map { it.act })
        assertEquals("Update to 2.0", a[0].label)
        assertTrue(a[0].primary)
        assertEquals("Test & write config", a[1].label)
        assertTrue(a.last().danger)
        val plain = PluginRules.actions(obj("""{"installed":true,"enabled":false,"can_update":false}"""), "Test")
        assertEquals(listOf("test", "logs", "enable", "remove"), plain.map { it.act })
        assertEquals("Update", PluginRules.actions(obj("""{"installed":true,"enabled":true,"can_update":true}"""), "Test")[0].label)
    }

    @Test
    fun theAboutLine() {
        val ia = obj("""{"kind":"source","kinds":["track","album"],"lossless":true,"trust":"third-party","sandbox":"Full isolation","network":true,"hosts":["archive.org","*.archive.org"],"reads_library":false,"license":"MIT","homepage":"https://github.com/Krifmaz/Encore"}""")
        assertEquals(listOf("Finds songs and albums, lossless", "Sandbox: Full isolation", "Network: archive.org, *.archive.org", "Licence MIT"), PluginRules.aboutParts(ia))
        assertEquals("https://github.com/Krifmaz/Encore", PluginRules.homepage(ia))
        val local = obj("""{"kind":"tool","trust":"third-party","network":false,"reads_library":true}""")
        assertEquals(listOf("No network", "Reads your library (read only)"), PluginRules.aboutParts(local))
        assertEquals(listOf("Network: any address"), PluginRules.aboutParts(obj("""{"kind":"tool","trust":"third-party","network":true,"hosts":[]}""")))
        // a catalogue plugin says nothing about a sandbox
        assertEquals(listOf("Finds links"), PluginRules.aboutParts(obj("""{"kind":"source","kinds":["url"],"trust":"first-party","network":true}""")))
    }

    /** GET /api/plugins/{id}/settings of yt-dlp: its own fields, then YouTube sign-in's as a part. */
    private val ytdlp = """{"id":"ytdlp","builtin":true,"settings_on":null,"fields":[
        {"key":"ytdlp_enabled","label":"Enable yt-dlp","type":"bool","part":null,"when":null},
        {"key":"ytdlp_format","label":"Format","type":"select","options":[{"value":"best","label":"Best"}],"part":null},
        {"key":"ytdlp_cookies","label":"Cookies","type":"textarea","secret":true,"part":"YouTube sign-in"},
        {"key":"ytdlp_cookie_browser","label":"Browser","type":"text","part":null}],
        "values":{"ytdlp_enabled":true,"ytdlp_format":"best","ytdlp_cookies":"•••• (saved)","ytdlp_cookie_browser":""}}"""

    @Test
    fun aCardsFieldsRunUnderTheirParts() {
        val runs = PluginRules.runs(obj(ytdlp))
        assertEquals(listOf("", "YouTube sign-in"), runs.map { it.part })
        assertEquals(listOf("ytdlp_enabled", "ytdlp_format"), runs[0].fields.map { it.key })
        // a part holds its field and the ones after it
        assertEquals(listOf("ytdlp_cookies", "ytdlp_cookie_browser"), runs[1].fields.map { it.key })
        val g = PluginRules.groupOf(obj(ytdlp), "yt-dlp")
        assertEquals("ytdlp", g.id)
        assertEquals(4, g.fields.size)
        assertEquals("Best", g.field("ytdlp_format")!!.options.single().label)
        // a client's `when` names the Torrents & Usenet card's select: the client's card is shown whole
        val qbit = obj("""{"id":"qbittorrent","builtin":true,"fields":[
            {"key":"qbit_url","label":"qBittorrent URL","type":"url","when":"torrent_client=qbittorrent"},
            {"key":"qbit_mode","label":"Mode","type":"select","options":[{"value":"a","label":"A"},{"value":"b","label":"B"}]},
            {"key":"qbit_b","label":"Only for B","type":"text","when":"qbit_mode=b"}],"values":{}}""")
        val fields = PluginRules.fields(qbit)
        assertEquals("", fields[0].rule)
        assertEquals("qbit_mode=b", fields[2].rule) // a rule on the card's own select stays
    }

    @Test
    fun valuesOfACard() {
        // a plugin's own settings: only its answer, none locked, saved secrets masked
        val own = obj("""{"id":"ia","builtin":false,"fields":[
            {"key":"format","label":"Format","type":"select","env":null,"env_locked":false},
            {"key":"token","label":"Token","type":"password","secret":true}],
            "values":{"format":"flac","token":"•••• (saved)"}}""")
        val v = PluginRules.cardValues(own, obj("""{"format":"mp3"}"""))
        assertEquals(JsonPrimitive("flac"), v["format"])
        assertEquals("ui", SchemaRules.source(v, "format"))
        val token = PluginRules.groupOf(own, "IA").field("token")!!
        assertEquals(SchemaRules.Note.SAVED_SECRET, SchemaRules.note(token, v))
        assertTrue(SchemaRules.changes(listOf(token), v, mapOf("token" to "")).isEmpty()) // empty keeps the saved one
        // a built-in plugin's keys are Encore's: GET /api/admin/settings wins, a key it lacks comes from the answer
        val builtin = obj("""{"id":"translator","builtin":true,"fields":[
            {"key":"translator_model","label":"Translator model","type":"select","env":"ENCORE_TRANSLATOR_MODEL","env_locked":true},
            {"key":"translate_background","label":"Translate","type":"bool","env_locked":false}],
            "values":{"translator_model":"small","translate_background":false}}""")
        val settings = obj("""{"translate_background":true,"_meta":{"translate_background":{"source":"ui","env":null}}}""")
        val b = PluginRules.cardValues(builtin, settings)
        assertEquals(JsonPrimitive(true), b["translate_background"])
        assertEquals(JsonPrimitive("small"), b["translator_model"])
        assertEquals("env", SchemaRules.source(b, "translator_model"))
        assertEquals("ENCORE_TRANSLATOR_MODEL", SchemaRules.envVar(b, "translator_model"))
        assertEquals("ui", SchemaRules.source(b, "translate_background"))
    }

    @Test
    fun clientNotesAndChosenClients() {
        val schema = SettingsSchema(
            listOf(
                SchemaSection(
                    id = "downloads",
                    title = "Downloads",
                    groups = listOf(
                        SchemaGroup(
                            id = "torznab",
                            title = "Torrents & Usenet (Prowlarr)",
                            test = "torznab",
                            testLabel = "Test connection",
                            fields = listOf(
                                SchemaField(key = "torrent_client", label = "Torrent client", kind = "select", options = listOf(SchemaOption("qbittorrent", "qBittorrent"), SchemaOption("transmission", "Transmission"))),
                                SchemaField(key = "nzb_client", label = "Usenet client", kind = "select", options = listOf(SchemaOption("sabnzbd", "SABnzbd"), SchemaOption("blackhole", "Blackhole folder"))),
                            ),
                        ),
                        SchemaGroup(id = "youtube", title = "YouTube sign-in", desc = "Which cookies yt-dlp uses.", test = "youtube", testLabel = "Test cookies"),
                        SchemaGroup(id = "ytdlp", title = "YouTube Music & links (yt-dlp)"),
                    ),
                ),
                SchemaSection(id = "processing", title = "Server processing"),
            ),
        )
        val qbit = obj("""{"id":"qbittorrent","name":"qBittorrent","kind":"client"}""")
        val answer = obj("""{"fields":[{"key":"qbit_url","when":"torrent_client=qbittorrent"}]}""")
        assertEquals(
            PluginRules.Note(true, "In use: Torrent client is set to qBittorrent on the Torrents & Usenet card."),
            PluginRules.clientNote(qbit, answer, obj("""{"torrent_client":"qbittorrent"}"""), schema),
        )
        assertEquals(
            PluginRules.Note(false, "Not in use: Torrent client is set to Transmission on the Torrents & Usenet card. Choose qBittorrent there to send downloads to qBittorrent."),
            PluginRules.clientNote(qbit, answer, obj("""{"torrent_client":"transmission"}"""), schema),
        )
        assertNull(PluginRules.clientNote(qbit, obj("""{"fields":[{"key":"x"}]}"""), obj("{}"), schema))

        val plugins = listOf(
            obj("""{"id":"qbittorrent","installed":false}"""),
            obj("""{"id":"usenet-blackhole","installed":true,"enabled":false}"""),
            obj("""{"id":"sabnzbd","installed":true,"enabled":true}"""),
        )
        val chosen = PluginRules.chosenClients(obj("""{"torrent_client":"qbittorrent","nzb_client":"blackhole"}"""), plugins, schema)
        assertEquals(
            listOf(
                PluginRules.ChosenClient("Torrent client: qBittorrent has a plugin of its own, not installed yet.", "install", "qbittorrent"),
                PluginRules.ChosenClient("Usenet client: the Blackhole folder plugin is disabled.", "enable", "usenet-blackhole"),
            ),
            chosen,
        )
        assertTrue(PluginRules.chosenClients(obj("""{"torrent_client":"other","nzb_client":"sabnzbd"}"""), plugins, schema).isEmpty())

        // the words that come from the settings schema
        assertEquals("Test connection", PluginRules.testLabel(schema, "torznab"))
        assertEquals("Test", PluginRules.testLabel(schema, "ytdlp"))
        assertEquals("Test", PluginRules.testLabel(schema, "ia"))
        assertEquals(PluginRules.PartInfo("Which cookies yt-dlp uses.", "youtube", "Test cookies", "youtube"), PluginRules.partInfo(schema, "YouTube sign-in"))
        assertEquals(PluginRules.PartInfo("", "", "Test", ""), PluginRules.partInfo(schema, "Accounts"))
        assertEquals("Server processing", PluginRules.settingsOnTitle(schema, "processing"))
        assertEquals("integrations", PluginRules.settingsOnTitle(schema, "integrations"))
    }

    @Test
    fun theRecordedSchemaNamesTheMovedCards() {
        val s = recordedSchema()
        assumeTrue("web-rs fixture not found from ${File(".").absolutePath}", s != null)
        // the cards that moved to the Plugins page are still in the schema (their keys are Encore's), on Downloads
        for ((card, _) in PluginRules.MOVED) assertEquals(card, "downloads", s!!.pageOf(card))
        assertEquals(PluginRules.PartInfo(s!!.group("youtube")!!.desc, "youtube", "Test cookies", "youtube"), PluginRules.partInfo(s, "YouTube sign-in"))
        assertEquals("Test connection", PluginRules.testLabel(s, "torznab"))
        // every client choice of the Torrents & Usenet card maps to a client plugin, and back
        for (key in listOf("torrent_client", "nzb_client")) {
            val f = s.group("torznab")!!.field(key)!!
            assertEquals(key, f.options.map { it.value }.toSet(), PluginRules.CLIENTS.filter { it.select == key }.map { it.value }.toSet())
        }
    }

    @Test
    fun theListAfterAnInstallElsewhere() {
        val list = obj("""{"plugins":[{"id":"ytdlp","installed":true},{"id":"slskd","installed":false}]}""")
        assertFalse(PluginRules.changedShape(list, setOf("ytdlp")))
        assertTrue(PluginRules.changedShape(list, setOf("ytdlp", "slskd")))
        assertTrue(PluginRules.changedShape(list, emptySet()))
        assertEquals(1, PluginRules.updates(obj("""{"plugins":[{"installed":true,"update_available":true},{"installed":false,"update_available":true}]}""")))
        assertTrue(PluginRules.hasCard(obj("""{"installed":true,"has_settings":true,"settings_on":null}""")))
        assertFalse(PluginRules.hasCard(obj("""{"installed":true,"has_settings":true,"settings_on":"processing"}""")))
        assertFalse(PluginRules.hasCard(obj("""{"installed":false,"has_settings":true}""")))
    }
}
