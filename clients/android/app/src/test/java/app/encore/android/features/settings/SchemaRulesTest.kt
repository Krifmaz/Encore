package app.encore.android.features.settings

import app.encore.android.core.EncoreJson
import app.encore.android.features.settings.SchemaRules.Note
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
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

class SchemaRulesTest {
    // one card of the answer as Config::schema writes it (docs/contracts/settings-schema.md): every attribute present
    private val torznab = """
        {"sections":[{"id":"apps","title":"Apps & sources","aliases":["downloads","tagging"],"groups":[
          {"id":"torznab","title":"Torrents & Usenet (Prowlarr)","part":"Download sources","desc":"Searches your indexers",
           "test":"torznab","test_label":"Test connection","fields":[
            {"key":"torrent_client","label":"Torrent client","help":null,"type":"select",
             "options":[{"value":"qbittorrent","label":"qBittorrent"},{"value":"transmission","label":"Transmission"}],
             "when":null,"secret":false,"env":"TORRENT_CLIENT","env_locked":false,"default":"qbittorrent","unit":null,
             "min":null,"max":null,"step":null,"full":false,"advanced":false},
            {"key":"qbit_password","label":"qBittorrent password","help":null,"type":"password","options":[],
             "when":"torrent_client=qbittorrent","secret":true,"env":"QBIT_PASSWORD","env_locked":false,"default":"",
             "unit":null,"min":null,"max":null,"step":null,"full":false,"advanced":false},
            {"key":"transmission_url","label":"Transmission address","help":"Where Transmission answers","type":"url",
             "options":[],"when":"torrent_client=transmission","secret":false,"env":null,"env_locked":false,"default":"",
             "unit":null,"min":null,"max":null,"step":null,"full":false,"advanced":false},
            {"key":"soulseek_port","label":"Listening port","help":null,"type":"number","options":[],"when":null,
             "secret":false,"env":"SOULSEEK_PORT","env_locked":false,"default":2234,"unit":null,"min":1,"max":65535,
             "step":null,"full":false,"advanced":false},
            {"key":"musicbrainz_rate","label":"MusicBrainz requests per second","help":null,"type":"number","options":[],
             "when":null,"secret":false,"env":"MUSICBRAINZ_RATE","env_locked":true,"default":1.0,"unit":"requests per second",
             "min":0,"max":null,"step":0.1,"full":false,"advanced":false},
            {"key":"release_types","label":"Kinds to request","help":null,"type":"checks",
             "options":[{"value":"Album","label":"Album"},{"value":"EP","label":"EP"},{"value":"Spokenword","label":"Spoken word"}],
             "when":null,"secret":false,"env":null,"env_locked":false,"default":"Album,EP","unit":null,"min":null,"max":null,
             "step":null,"full":true,"advanced":false},
            {"key":"auto_approve","label":"Approve requests on their own","help":null,"type":"bool","options":[],"when":null,
             "secret":false,"env":null,"env_locked":false,"default":false,"unit":null,"min":null,"max":null,"step":null,
             "full":false,"advanced":false},
            {"key":"streamrip_command","label":"Command template (advanced)","help":null,"type":"text","options":[],
             "when":null,"secret":false,"env":null,"env_locked":false,"default":"rip","unit":null,"min":null,"max":null,
             "step":null,"full":true,"advanced":true,"some_later_attribute":{"x":1}}
          ]}]},
          {"id":"blocked","title":"Blocked","aliases":[],"groups":[]}]}
    """.trimIndent()

    private val schema: SettingsSchema = EncoreJson.decodeFromString(SettingsSchema.serializer(), torznab)
    private val card: SchemaGroup = schema.group("torznab")!!
    private fun f(key: String): SchemaField = card.field(key)!!

    // GET /api/admin/settings: values with `_meta`
    private val values: JsonObject = EncoreJson.parseToJsonElement(
        """
        {"torrent_client":"qbittorrent","qbit_password":"•••• (saved)","transmission_url":"","soulseek_port":2234,
         "musicbrainz_rate":1.0,"release_types":"Album,EP","auto_approve":false,"streamrip_command":"rip",
         "_meta":{"torrent_client":{"source":"ui","env":"TORRENT_CLIENT"},
                  "qbit_password":{"source":"ui","env":"QBIT_PASSWORD"},
                  "transmission_url":{"source":"default","env":null},
                  "soulseek_port":{"source":"default","env":"SOULSEEK_PORT"},
                  "musicbrainz_rate":{"source":"env","env":"MUSICBRAINZ_RATE"},
                  "release_types":{"source":"default","env":null},
                  "auto_approve":{"source":"env","env":"ENCORE_AUTO_APPROVE"},
                  "streamrip_command":{"source":"default","env":null}}}
        """,
    ).jsonObject

    @Test
    fun theAnswerDecodesWithEveryAttribute() {
        assertEquals(listOf("apps", "blocked"), schema.sections.map { it.id })
        assertEquals(listOf("downloads", "tagging"), schema.section("apps")!!.aliases)
        assertTrue(schema.section("blocked")!!.groups.isEmpty())
        assertEquals("Download sources", card.part)
        assertEquals("torznab", card.test)
        assertEquals("Test connection", SchemaRules.testLabel(card))
        val pw = f("qbit_password")
        assertEquals("password", pw.kind)
        assertEquals("torrent_client=qbittorrent", pw.rule)
        assertTrue(pw.secret && pw.isSecret)
        assertEquals("", pw.help) // null reads as ""
        assertEquals("", f("transmission_url").env)
        val port = f("soulseek_port")
        assertEquals(1.0, port.min!!, 0.0)
        assertEquals(65535.0, port.max!!, 0.0)
        assertNull(port.step)
        assertEquals(JsonPrimitive(2234), port.default)
        assertEquals(0.1, f("musicbrainz_rate").step!!, 0.0)
        assertTrue(f("musicbrainz_rate").envLocked)
        assertEquals(listOf("Album", "EP", "Spokenword"), f("release_types").options.map { it.value })
        assertTrue(f("streamrip_command").advanced) // and an attribute this app doesn't know is skipped
        assertEquals(JsonNull, SchemaField(key = "x").default)
    }

    @Test
    fun linksOpenThePageOfAnAliasOrACard() {
        // this answer is a server's before R2: downloads was an alias of Apps & sources
        assertEquals("apps", schema.pageOf("apps"))
        assertEquals("apps", schema.pageOf("downloads"))
        assertEquals("apps", schema.pageOf("torznab"))
        assertEquals("users", schema.pageOf("users"))
    }

    @Test
    fun aPageIdWinsOverAnAliasOrACardOfTheSameName() {
        // R2: Library holds a card called library too; a page's own id is looked up first, then aliases, then cards
        val split = EncoreJson.decodeFromString(
            SettingsSchema.serializer(),
            """
            {"sections":[
              {"id":"general","title":"General","aliases":["library"],"groups":[{"id":"downloads","title":"x"}]},
              {"id":"library","title":"Library","aliases":["tagging"],"groups":[{"id":"verify","title":"y"}]},
              {"id":"downloads","title":"Downloads","aliases":["apps"],"groups":[{"id":"tagging","title":"z"}]}]}
            """.trimIndent(),
        )
        assertEquals("library", split.pageOf("library"))
        assertEquals("downloads", split.pageOf("downloads"))
        assertEquals("library", split.pageOf("tagging"))
        assertEquals("downloads", split.pageOf("apps"))
        assertEquals("library", split.pageOf("verify"))
    }

    @Test
    fun whenRulesParseAndFollowTheUnsavedSelect() {
        assertEquals(SchemaRules.When("torrent_client", "qbittorrent", true), SchemaRules.parseWhen("torrent_client=qbittorrent"))
        assertEquals(SchemaRules.When("nzb_client", "none", false), SchemaRules.parseWhen("nzb_client!=none"))
        assertEquals(SchemaRules.When("a", "b", true), SchemaRules.parseWhen(" a = b "))
        for (bad in listOf(null, "", "   ", "novalue", "=x", "k=", "k!=")) assertNull(bad, SchemaRules.parseWhen(bad))
        assertTrue(SchemaRules.When("k", "v", true).shows("v"))
        assertFalse(SchemaRules.When("k", "v", true).shows("w"))
        assertTrue(SchemaRules.When("k", "v", false).shows("w"))
        assertFalse(SchemaRules.When("k", "v", false).shows("v"))

        val drafts = HashMap<String, String>()
        val shown = { key: String -> SchemaRules.isShown(f(key)) { k -> SchemaRules.current(k, values, drafts) } }
        assertTrue(shown("qbit_password"))
        assertFalse(shown("transmission_url"))
        assertTrue(shown("soulseek_port")) // no rule
        drafts["torrent_client"] = "transmission" // chosen, not saved yet
        assertFalse(shown("qbit_password"))
        assertTrue(shown("transmission_url"))
    }

    @Test
    fun valuesReadAsTheWebReadsThem() {
        assertEquals("2", SchemaRules.textOf(JsonPrimitive(2)))
        assertEquals("1", SchemaRules.textOf(EncoreJson.parseToJsonElement("1.0")))
        assertEquals("0.85", SchemaRules.textOf(EncoreJson.parseToJsonElement("0.85")))
        assertEquals("true", SchemaRules.textOf(JsonPrimitive(true)))
        assertEquals("", SchemaRules.textOf(JsonNull))
        assertEquals("", SchemaRules.textOf(null))
        assertEquals("007", SchemaRules.textOf(JsonPrimitive("007")))
        assertFalse(SchemaRules.truthy(JsonPrimitive("")))
        assertFalse(SchemaRules.truthy(JsonPrimitive(0)))
        assertFalse(SchemaRules.truthy(JsonPrimitive(false)))
        assertTrue(SchemaRules.truthy(JsonPrimitive("•••• (saved)")))
        assertTrue(SchemaRules.truthy(JsonPrimitive("0"))) // a non-empty string, as in JavaScript

        assertEquals("", SchemaRules.initialText(f("qbit_password"), values)) // a secret starts empty
        assertEquals("false", SchemaRules.initialText(f("auto_approve"), values))
        assertEquals("2234", SchemaRules.initialText(f("soulseek_port"), values))
        assertEquals("1", SchemaRules.initialText(f("musicbrainz_rate"), values))
    }

    @Test
    fun numbersParseAsTyped() {
        assertEquals(5.0, SchemaRules.parseNumber(" 5 ")!!, 0.0)
        assertEquals(0.5, SchemaRules.parseNumber("0,5")!!, 0.0) // a decimal comma
        assertEquals(-2.5, SchemaRules.parseNumber("-2.5")!!, 0.0)
        assertEquals(1000.0, SchemaRules.parseNumber("1e3")!!, 0.0)
        for (bad in listOf("", "  ", "abc", "1.2.3", "1,2,3", "1,5.0", "NaN", "Infinity", "0x10", "5 MB")) {
            assertNull(bad, SchemaRules.parseNumber(bad))
        }
        assertNull(SchemaRules.numberError(""))
        assertNull(SchemaRules.numberError("12"))
        assertEquals("Enter a number", SchemaRules.numberError("twelve"))
        assertEquals(JsonPrimitive(65535L), SchemaRules.numberJson(65535.0))
        assertEquals(JsonPrimitive(0.25), SchemaRules.numberJson(0.25))
        assertNull(SchemaRules.rangeNote(f("soulseek_port"), "2234"))
        assertEquals("Usually 1 to 65535", SchemaRules.rangeNote(f("soulseek_port"), "70000"))
        assertEquals("Usually 0 or more", SchemaRules.rangeNote(f("musicbrainz_rate"), "-1"))
        assertNull(SchemaRules.rangeNote(f("soulseek_port"), "x"))
        assertTrue(SchemaRules.needsDecimalKeys(f("musicbrainz_rate")))
        assertFalse(SchemaRules.needsDecimalKeys(f("soulseek_port")))
    }

    @Test
    fun saveSendsOnlyWhatChanged() {
        val fields = card.fields
        val drafts = HashMap<String, String>()
        assertEquals(JsonObject(emptyMap()), SchemaRules.changes(fields, values, drafts))

        // typed back to what is saved: nothing to send
        drafts["torrent_client"] = "qbittorrent"
        drafts["soulseek_port"] = "2234.0"
        drafts["release_types"] = "EP,Album"
        drafts["transmission_url"] = ""
        drafts["streamrip_command"] = "rip"
        assertEquals(JsonObject(emptyMap()), SchemaRules.changes(fields, values, drafts))

        drafts["torrent_client"] = "transmission"
        drafts["transmission_url"] = "http://transmission:9091" // hidden until the select changed; saved with its card
        drafts["soulseek_port"] = "50300"
        drafts["release_types"] = "Album,EP,Spokenword"
        drafts["streamrip_command"] = "rip --quiet" // an advanced field behind Show advanced is still saved
        val body = SchemaRules.changes(fields, values, drafts)
        assertEquals(
            listOf("torrent_client", "transmission_url", "soulseek_port", "release_types", "streamrip_command"),
            body.keys.toList(),
        ) // the card's order
        assertEquals(JsonPrimitive("transmission"), body["torrent_client"])
        assertEquals(JsonPrimitive(50300L), body["soulseek_port"]) // a number as a number
        assertEquals(JsonPrimitive("Album,EP,Spokenword"), body["release_types"]) // checks as comma-separated text
    }

    @Test
    fun switchesChecksAndNumbers() {
        val open = values.toMutableMap().also { it["_meta"] = JsonObject(emptyMap()) }.let(::JsonObject)
        val sw = f("auto_approve")
        assertNull(SchemaRules.changedValue(sw, open, "false"))
        assertEquals(JsonPrimitive(true), SchemaRules.changedValue(sw, open, SchemaRules.textFor(true)))

        // checks compare without case, spaces and punctuation: a saved "Spoken word" ticks "Spokenword"
        val rt = f("release_types")
        val saved = JsonObject(mapOf("release_types" to JsonPrimitive("album, Spoken word")))
        assertEquals(setOf("Album", "Spokenword"), SchemaRules.ticked(rt, "album, Spoken word"))
        assertNull(SchemaRules.changedValue(rt, saved, "Album,Spokenword"))
        assertEquals("Album,EP", SchemaRules.toggle(rt, "EP", "Album", true)) // in the options' order
        assertEquals("EP", SchemaRules.toggle(rt, "Album,EP", "Album", false))
        assertEquals(JsonPrimitive(""), SchemaRules.changedValue(rt, saved, ""))

        // an emptied number sends "" (the server's default, like the web); one that doesn't parse sends nothing
        val port = f("soulseek_port")
        assertEquals(JsonPrimitive(""), SchemaRules.changedValue(port, open, " "))
        assertNull(SchemaRules.changedValue(port, open, "lots"))
        assertEquals(listOf("soulseek_port"), SchemaRules.invalid(card.fields, open, mapOf("soulseek_port" to "lots")).map { it.key })
        assertTrue(SchemaRules.invalid(card.fields, open, mapOf("soulseek_port" to "")).isEmpty())
        assertEquals(JsonPrimitive(0.5), SchemaRules.changedValue(port, open, "0,5"))
    }

    @Test
    fun secretsKeepTheSavedValue() {
        val pw = f("qbit_password")
        assertNull(SchemaRules.changedValue(pw, values, null))
        assertNull(SchemaRules.changedValue(pw, values, ""))
        assertNull(SchemaRules.changedValue(pw, values, "   ")) // empty keeps the saved one
        assertNull(SchemaRules.changedValue(pw, values, "•••• (saved)")) // never echo the mask
        assertEquals(JsonPrimitive("hunter2"), SchemaRules.changedValue(pw, values, "  hunter2 ")) // trimmed
        // what Show put in the field goes back only when it was changed
        assertNull(SchemaRules.changedValue(pw, values, "hunter2", revealed = "hunter2"))
        assertEquals(JsonPrimitive("hunter3"), SchemaRules.changedValue(pw, values, "hunter3", revealed = "hunter2"))
    }

    @Test
    fun environmentLocksAField() {
        // _meta says env (auto_approve), or the schema does (musicbrainz_rate)
        assertTrue(SchemaRules.isLocked(f("auto_approve"), values))
        assertTrue(SchemaRules.isLocked(f("musicbrainz_rate"), values))
        assertFalse(SchemaRules.isLocked(f("soulseek_port"), values))
        assertNull(SchemaRules.changedValue(f("auto_approve"), values, "true"))
        assertNull(SchemaRules.changedValue(f("musicbrainz_rate"), values, "3"))
        assertTrue(SchemaRules.invalid(card.fields, values, mapOf("musicbrainz_rate" to "x")).isEmpty())
        assertEquals("ENCORE_AUTO_APPROVE", SchemaRules.envOf(f("auto_approve"), values)) // _meta first
        // the hand-written pages read the same `_meta` by key
        assertEquals("env", SchemaRules.source(values, "auto_approve"))
        assertEquals("ENCORE_AUTO_APPROVE", SchemaRules.envVar(values, "auto_approve"))
        assertEquals("", SchemaRules.envVar(values, "release_types"))
        assertEquals("", SchemaRules.envVar(values, "no_such_key"))
        assertEquals("set by environment", SchemaRules.passwordPlaceholder(f("qbit_password").copy(envLocked = true), values))
    }

    @Test
    fun theNoteUnderAField() {
        assertEquals(Note.LOCKED, SchemaRules.note(f("auto_approve"), values))
        assertEquals(Note.SAVED_SECRET, SchemaRules.note(f("qbit_password"), values))
        val unset = JsonObject(values + ("qbit_password" to JsonPrimitive("")))
        assertEquals(Note.UNSET_SECRET, SchemaRules.note(f("qbit_password"), unset))
        assertEquals(Note.NONE, SchemaRules.note(f("torrent_client"), values)) // saved in Settings
        assertEquals(Note.DEFAULT_ENV, SchemaRules.note(f("soulseek_port"), values))
        assertEquals(Note.NONE, SchemaRules.note(f("transmission_url"), values)) // no variable

        assertEquals("saved – paste a new value to replace", SchemaRules.passwordPlaceholder(f("qbit_password"), values))
        assertEquals("not set", SchemaRules.passwordPlaceholder(f("qbit_password"), unset))
        assertTrue(SchemaRules.canShow(f("qbit_password"), values, canReveal = true))
        assertFalse(SchemaRules.canShow(f("qbit_password"), values, canReveal = false))
        assertFalse(SchemaRules.canShow(f("qbit_password"), unset, canReveal = true))
        assertFalse(SchemaRules.canShow(f("transmission_url"), values, canReveal = true)) // not a secret
        val wg = SchemaField(key = "vpn_wireguard_config", kind = "textarea", secret = true)
        assertFalse(SchemaRules.canShow(wg, JsonObject(mapOf(wg.key to JsonPrimitive("•••• (saved)"))), canReveal = true))
        assertEquals("Saved – paste to replace", SchemaRules.textareaPlaceholder(wg, JsonObject(mapOf(wg.key to JsonPrimitive("•••• (saved)")))))
    }

    @Test
    fun cardsAndTheVpnBlock() {
        // a field the server's values lack is left out; a card with none left goes
        val few = JsonObject(mapOf("torrent_client" to JsonPrimitive("qbittorrent"), "auto_approve" to JsonPrimitive(false)))
        val cards = SchemaRules.visibleCards(schema.section("apps")!!, few)
        assertEquals(listOf("torznab"), cards.map { it.first.id })
        assertEquals(listOf("torrent_client", "auto_approve"), cards[0].second.map { it.key })
        assertTrue(SchemaRules.visibleCards(schema.section("apps")!!, JsonObject(emptyMap())).isEmpty())
        assertTrue(SchemaRules.hasAdvanced(card.fields))

        val k = { key: String -> SchemaField(key = key) }
        val l = SchemaRules.layout(listOf(k("slskd_enabled"), k("vpn_enabled"), k("vpn_wireguard_config"), k("slskd_min_bitrate"), k("vpn_kill_switch")))
        assertEquals(listOf("slskd_enabled"), l.before.map { it.key })
        assertEquals(listOf("vpn_enabled", "vpn_wireguard_config", "vpn_kill_switch"), l.vpn.map { it.key })
        assertEquals(listOf("slskd_min_bitrate"), l.after.map { it.key })
        val none = SchemaRules.layout(listOf(k("a"), k("b")))
        assertEquals(listOf("a", "b"), none.before.map { it.key })
        assertTrue(none.vpn.isEmpty() && none.after.isEmpty())
    }

    @Test
    fun testResultsAndSaveWarnings() {
        fun o(s: String) = EncoreJson.parseToJsonElement(s).jsonObject
        assertEquals(ResultLine(true, "Connected"), SchemaRules.testLine(o("""{"ok":true}""")))
        assertEquals(ResultLine(false, "Failed"), SchemaRules.testLine(o("""{"ok":false,"message":""}""")))
        assertEquals(ResultLine(true, "Prowlarr 1.2 · 3 indexers"), SchemaRules.testLine(o("""{"ok":true,"message":"Prowlarr 1.2 · 3 indexers"}""")))
        assertEquals(ResultLine(false, "Test failed"), SchemaRules.testFailed(" "))
        assertEquals(ResultLine(false, "Admin only"), SchemaRules.testFailed("Admin only"))

        val answer = o("""{"library_path":"/music","_warnings":[{"key":"library_path","message":"Encore can't write to /music"},{"key":"x"}]}""")
        assertEquals(listOf(ResultLine(false, "Encore can't write to /music")), SchemaRules.warnings(answer))
        assertEquals(setOf("library_path"), SchemaRules.withoutWarnings(answer).keys)
        assertTrue(SchemaRules.warnings(o("{}")).isEmpty())
        assertEquals("Transmission", SchemaRules.optionLabel(f("torrent_client"), "transmission"))
        assertEquals("deluge", SchemaRules.optionLabel(f("torrent_client"), "deluge")) // not an option: as it is
    }

    /** The answer the web's tests use, recorded from GET /api/admin/settings/schema (read when the repo is there). */
    @Test
    fun theRecordedServerAnswerDecodes() {
        val file = listOf("../../../web-rs", "../../web-rs", "web-rs", "../web-rs")
            .map { File(it, "src/settings/fixtures/settings-schema.json") }
            .firstOrNull { it.isFile }
        assumeTrue("web-rs fixture not found from ${File(".").absolutePath}", file != null)
        val raw = EncoreJson.parseToJsonElement(file!!.readText()).jsonObject
        val s = EncoreJson.decodeFromString(SettingsSchema.serializer(), file.readText())
        val rawSections = raw["sections"] as JsonArray
        assertEquals(rawSections.size, s.sections.size)
        assertEquals("general", s.sections.first().id)
        val all = s.sections.flatMap { it.groups }.flatMap { it.fields }
        val rawCount = rawSections.sumOf { sec -> (sec.jsonObject["groups"] as JsonArray).sumOf { g -> (g.jsonObject["fields"] as JsonArray).size } }
        assertEquals(rawCount, all.size) // no field dropped
        val kinds = setOf("text", "path", "url", "password", "textarea", "number", "bool", "select", "checks")
        assertTrue(all.map { it.kind }.toSet().toString(), kinds.containsAll(all.map { it.kind }.toSet()))
        // every rule parses and names a select whose options hold the value (the server's build checks the same)
        for (fld in all.filter { it.rule.isNotEmpty() }) {
            val w = SchemaRules.parseWhen(fld.rule)!!
            val select = all.first { it.key == w.key }
            assertEquals("select", select.kind)
            assertTrue(fld.key, select.options.any { it.value == w.value })
        }
        // every password is a secret, and every select and checks has options
        assertTrue(all.filter { it.kind == "password" }.all { it.secret })
        assertTrue(all.filter { it.kind == "select" || it.kind == "checks" }.all { it.options.isNotEmpty() })
        assertTrue(s.sections.flatMap { it.groups }.filter { it.test.isNotEmpty() }.all { it.testLabel.isNotEmpty() })
        // Apps & sources is four pages since R2: its old names and its cards open the page that holds them
        assertEquals(listOf("general", "library", "downloads", "integrations", "processing"), s.sections.take(5).map { it.id })
        assertEquals("downloads", s.pageOf("apps"))
        assertEquals("downloads", s.pageOf("downloads"))
        assertEquals("downloads", s.pageOf("slskd"))
        assertEquals("library", s.pageOf("tagging"))
        assertEquals("library", s.pageOf("verify"))
        assertEquals("integrations", s.pageOf("services"))
        assertEquals("processing", s.pageOf("cadenza"))
        // the Download tools card and How requests are fulfilled go on the page with the pipeline card
        assertEquals(listOf("downloads"), s.sections.filter { sec -> sec.groups.any { it.id == "pipeline" } }.map { it.id })
        val vpn = SchemaRules.layout(s.group("slskd")!!.fields)
        assertTrue(vpn.vpn.map { it.key }.contains("vpn_wireguard_config"))
    }
}
