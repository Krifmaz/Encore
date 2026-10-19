package app.encore.android.features.settings

import app.encore.android.core.EncoreJson
import app.encore.android.features.settings.SettingsRules.Back
import app.encore.android.features.settings.SettingsRules.ToolAction
import app.encore.android.features.settings.SettingsRules.Wait
import app.encore.android.ui.theme.Appearance
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SettingsRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json).jsonObject

    // the web's update_tests (web-rs/src/settings/diagnostics.rs), word for word
    @Test
    fun notesBecomePlainLines() {
        val md = "## What's new\r\n\r\n* **Offline**: songs play without the server ([docs](https://x.y/docs))\n- Update from `Settings`\n\n\n\n### Fixes\n  * a nested ~~one~~ item\n> quoted *line*\n![shot](https://x.y/a.png)\n<!-- hidden\nnote -->\n---\n```sh\ndocker compose pull\n```\nSee <https://github.com/Krifmaz/Encore>."
        assertEquals(
            "What's new\n\n• Offline: songs play without the server (docs)\n• Update from Settings\n\nFixes\n  • a nested ~~one~~ item\nquoted line\nshot\n\ndocker compose pull\nSee https://github.com/Krifmaz/Encore.",
            SettingsRules.notesText(md),
        )
    }

    @Test
    fun notesKeepPlainTextAndOddBrackets() {
        assertEquals("", SettingsRules.notesText(""))
        assertEquals("Title", SettingsRules.notesText("\n\n# Title\n\n"))
        assertEquals("a [b] c (d) e!", SettingsRules.notesText("a [b] c (d) e!"))
        assertEquals("x < y and [unclosed](link", SettingsRules.notesText("x < y and [unclosed](link"))
        assertEquals("a and b", SettingsRules.notesText("[a](u1) and [b](u2)"))
        assertEquals("Ünïcode – “quotes” ok", SettingsRules.notesText("Ünïcode – “quotes” [ok](u)"))
    }

    @Test
    fun theWaitAfterARestartOrAnUpdate() {
        // a restart: back once the server went away and answers again
        assertEquals(Back.Waiting, SettingsRules.judge(Wait.RESTART, "0.28.0", "0.28.0", false, 0, 2000))
        assertEquals(Back.Waiting, SettingsRules.judge(Wait.RESTART, "0.28.0", null, true, 0, 4000))
        assertEquals(Back.Running("0.28.0"), SettingsRules.judge(Wait.RESTART, "0.28.0", "0.28.0", true, 0, 6000))
        // so quick that no poll saw it down: 30 s of answers count as back
        assertEquals(Back.Running("0.28.0"), SettingsRules.judge(Wait.RESTART, "0.28.0", "0.28.0", false, SettingsRules.QUIET_BACK_MS, 32_000))
        // an update: back when another version answers; the old one after a gap means it did not go through
        assertEquals(Back.Waiting, SettingsRules.judge(Wait.UPDATE, "0.28.0", "0.28.0", false, 40_000, 40_000))
        assertEquals(Back.Running("0.29.0"), SettingsRules.judge(Wait.UPDATE, "0.28.0", "0.29.0", false, 0, 8000))
        assertEquals(Back.SameVersion, SettingsRules.judge(Wait.UPDATE, "0.28.0", "0.28.0", true, 0, 30_000))
        // late after 90 s
        assertEquals(Back.Late, SettingsRules.judge(Wait.UPDATE, "0.28.0", null, true, 0, SettingsRules.BACK_BUDGET_MS))
        assertEquals(Back.Late, SettingsRules.judge(Wait.RESTART, "0.28.0", null, true, 0, SettingsRules.BACK_BUDGET_MS + 2000))
    }

    @Test
    fun vpnLines() {
        assertEquals("Off", SettingsRules.vpnText(null))
        assertEquals(
            "Connected · port 51234 · public IP 1.2.3.4 · last handshake 12 s ago",
            SettingsRules.vpnText(obj("""{"state":"up","forwarded_port":51234,"public_ip":"1.2.3.4","last_handshake_secs_ago":11.6}""")),
        )
        assertEquals("Connected · no forwarded port yet", SettingsRules.vpnText(obj("""{"state":"up","forwarded_port":null,"port_forwarding":true}""")))
        assertEquals("Connected · no forwarded port (timeout)", SettingsRules.vpnText(obj("""{"state":"up","port_forwarding":true,"natpmp_error":"timeout"}""")))
        assertEquals("Connecting…", SettingsRules.vpnText(obj("""{"state":"connecting"}""")))
        assertEquals("The VPN could not connect", SettingsRules.vpnText(obj("""{"state":"error"}""")))
        assertEquals("Port forwarding failed: no gateway", SettingsRules.vpnTestNote(obj("""{"natpmp":{"ok":false,"error":"no gateway"}}""")))
        assertEquals("", SettingsRules.vpnTestNote(obj("""{"natpmp":{"ok":true}}""")))
        val row = SettingsRules.vpnCheck(obj("""{"enabled":true,"state":"error","error":"no handshake","endpoint":"vpn.example:51820"}"""))
        assertEquals("false", (row["ok"] as JsonPrimitive).content)
        assertEquals("WireGuard vpn.example:51820", (row["request"] as JsonPrimitive).content)
    }

    @Test
    fun soulseekLines() {
        val running = SettingsRules.soulseekLine(obj("""{"state":"running","version":"v0.24.1","logged_in":true,"username":"alice"}"""))
        assertEquals("Running v0.24.1 · signed in as alice", running.text)
        assertFalse(running.error)
        val crashed = SettingsRules.soulseekLine(obj("""{"state":"crashed","error":"exit 1","log":["a","b"],"hint":"look at the log"}"""))
        assertEquals("Crashed – exit 1 · look at the log", crashed.text)
        assertTrue(crashed.error)
        assertEquals(listOf("a", "b"), crashed.log)
        assertEquals("Off – switch on Enable Soulseek and save", SettingsRules.soulseekLine(obj("""{"state":"off","enabled":false}""")).text)
        assertEquals("Running · signing in…", SettingsRules.soulseekLine(obj("""{"state":"running"}""")).text)
        assertTrue(SettingsRules.canStartNow("blocked"))
        assertFalse(SettingsRules.canStartNow("running"))
    }

    @Test
    fun cadenzaLines() {
        assertEquals("Cache: not available on this server", SettingsRules.cadenzaCacheText(null))
        assertEquals(
            "Cache: 812 MB of 4096 MB · 23 songs · renders at 6.2× real time",
            SettingsRules.cadenzaCacheText(obj("""{"used_mb":811.6,"max_mb":4096,"files":23,"last_factor":6.2}""")),
        )
        assertEquals("Cache: 0 MB of 0 MB · 1 song", SettingsRules.cadenzaCacheText(obj("""{"files":1}""")))
        assertEquals("Engine: Cadenza 2.1.0 (updated)", SettingsRules.cadenzaEngineText(obj("""{"source":"updated","version":"2.1.0"}""")))
        assertEquals("Engine: Cadenza (built in)", SettingsRules.cadenzaEngineText(null))
    }

    @Test
    fun engineSettingsKeepOnlyWhatDiffersFromTheDefault() {
        val engine = obj(
            """{"settings":[
                {"key":"taps","label":"Taps","type":"number","default":2048,"min":256},
                {"key":"fast","label":"Fast","type":"bool","default":false},
                {"key":"shape","label":"Shape","type":"select","default":"lin","options":[["lin","Linear"],["min","Minimum"]]},
                {"key":"odd","type":"text","default":""}
            ],"values":{"taps":4096,"gone":1}}""",
        )
        val settings = SettingsRules.engineSettings(engine)
        assertEquals(listOf("taps", "fast", "shape"), settings.map { it.key })
        assertEquals("4096", SettingsRules.engineInitial(settings[0], engine["values"] as JsonObject))
        assertEquals("Linear", SettingsRules.engineDefaultText(settings[2]))
        // the number back at its default goes; a switch turned on stays; a key this engine lacks stays as saved
        val v = SettingsRules.engineValue(settings, engine["values"] as JsonObject, mapOf("taps" to "2048", "fast" to "true", "shape" to "min"))
        assertEquals(obj("""{"gone":1,"fast":true,"shape":"min"}"""), obj(v))
        // a number that does not parse is left out
        assertEquals(obj("""{"gone":1}"""), obj(SettingsRules.engineValue(settings, engine["values"] as JsonObject, mapOf("taps" to "lots"))))
    }

    @Test
    fun engineSettingsAreSentOnlyWhenTheyChanged() {
        val engine = obj(
            """{"settings":[
                {"key":"taps","label":"Taps","type":"number","default":2048},
                {"key":"fast","label":"Fast","type":"bool","default":false}
            ],"values":{"taps":4096,"gone":1}}""",
        )
        val settings = SettingsRules.engineSettings(engine)
        val saved = engine["values"] as JsonObject
        // nothing edited, or edited back to what is saved: nothing to send
        assertNull(SettingsRules.engineChange(settings, saved, emptyMap()))
        assertNull(SettingsRules.engineChange(settings, saved, mapOf("taps" to "4096", "fast" to "false")))
        assertNull(SettingsRules.engineChange(settings, null, mapOf("fast" to "false")))
        // a real change: the whole value, as engineValue writes it
        assertEquals(obj("""{"taps":4096,"gone":1,"fast":true}"""), obj(SettingsRules.engineChange(settings, saved, mapOf("fast" to "true"))!!))
        assertEquals(obj("""{"gone":1}"""), obj(SettingsRules.engineChange(settings, saved, mapOf("taps" to "2048"))!!))
    }

    @Test
    fun toolRows() {
        assertEquals(ToolAction.Install("0.24.1"), SettingsRules.toolAction(obj("""{"id":"slskd","updatable":true,"latest":"0.24.1"}""")))
        assertEquals(ToolAction.Update("2025.1"), SettingsRules.toolAction(obj("""{"id":"ytdlp","installed":"2024.9","updatable":true,"update_available":true,"latest":"2025.1"}""")))
        assertEquals(ToolAction.Available("2025.1"), SettingsRules.toolAction(obj("""{"id":"ytdlp","installed":"2024.9","update_available":true,"latest":"2025.1"}""")))
        assertEquals(ToolAction.UpToDate, SettingsRules.toolAction(obj("""{"id":"ytdlp","installed":"2025.1","latest":"2025.1","updatable":true}""")))
        assertEquals(ToolAction.NotChecked, SettingsRules.toolAction(obj("""{"id":"ytdlp","installed":"2025.1","updatable":true}""")))
        assertEquals("Back to the bundled yt-dlp", SettingsRules.toolReset(obj("""{"id":"ytdlp","name":"yt-dlp"}""")).third)
        assertEquals(2, SettingsRules.toolList(obj("""{"tools":[{"id":"a"},{"id":"b"}]}""")).size)
        assertEquals(1, SettingsRules.toolList(EncoreJson.parseToJsonElement("""[{"id":"a"}]""")).size)
    }

    @Test
    fun tasksAndTimes() {
        val now = 1_000_000.0
        assertEquals("never", SettingsRules.ago(0.0, now))
        assertEquals("just now", SettingsRules.ago(now - 30, now))
        assertEquals("5 min ago", SettingsRules.ago(now - 300, now))
        assertEquals("in 2 h", SettingsRules.until(now + 7200, now))
        assertEquals("any moment", SettingsRules.until(now + 30, now))
        assertEquals(
            "Last run 5 min ago · took 12 s · Scanned 3 albums · next in 2 h",
            SettingsRules.taskLine(obj("""{"enabled":true,"last_run":${now - 300},"duration":12,"result":"Scanned 3 albums","next_run":${now + 7200}}"""), now),
        )
        assertEquals("Last run never · running…", SettingsRules.taskLine(obj("""{"running":true,"enabled":true,"next_run":${now + 60}}"""), now))
        // a task's own interval joins the usual ones, in order
        val opts = SettingsRules.everyOptions(45.0)
        assertEquals(listOf("15", "30", "45", "60"), opts.take(4).map { it.first })
        assertEquals("45 minutes", opts[2].second)
        assertEquals("hour", opts[3].second)
    }

    @Test
    fun numbersAsTheWebSendsThem() {
        assertEquals(JsonPrimitive(0), SettingsRules.jsPlus(""))
        assertEquals(JsonPrimitive(7), SettingsRules.jsPlus(" 7 "))
        assertEquals(JsonPrimitive(2.5), SettingsRules.jsPlus("2.5"))
        assertEquals(JsonNull, SettingsRules.jsPlus("seven"))
        assertEquals(12L, SettingsRules.parseIntOrZero("12abc"))
        assertEquals(0L, SettingsRules.parseIntOrZero("abc"))
        assertEquals(-3L, SettingsRules.parseIntOrZero("-3"))
    }

    @Test
    fun invitesAndReports() {
        val now = 1000.0
        assertTrue(SettingsRules.inviteDead(obj("""{"uses":1,"max_uses":1}"""), now))
        assertTrue(SettingsRules.inviteDead(obj("""{"uses":0,"max_uses":1,"expires_at":999}"""), now))
        assertFalse(SettingsRules.inviteDead(obj("""{"uses":0,"max_uses":1,"expires_at":null}"""), now))
        assertEquals("https://music.example/#/signup?invite=AB%20C", SettingsRules.inviteLink("https://music.example/", "AB C"))
        assertEquals("fake_lossless", SettingsRules.reasonFor(listOf("quiet", "upsampled")))
        assertEquals("loudness", SettingsRules.reasonFor(listOf("quiet")))
        assertEquals("wrong_version", SettingsRules.reasonFor(listOf("wrong_length")))
        assertEquals("bad_audio", SettingsRules.reasonFor(emptyList()))
        assertEquals("No replacement yet" to "err", SettingsRules.reportStatus("no_replacement"))
    }

    @Test
    fun connectionWords() {
        assertEquals("oauth", SettingsRules.connectionInfo("spotify").second)
        assertEquals("" to "", SettingsRules.connectionInfo("myspace"))
        assertEquals("https://music.youtube.com/playlist?list=LM", SettingsRules.likedUrl("ytmusic"))
        assertNull(SettingsRules.likedUrl("deezer"))
        val (msg, errors) = SettingsRules.syncToast(obj("""{"liked_added":2,"liked_removed":0,"pushed":3,"playlists":1,"errors":1}"""))
        assertEquals("Synced: +2 / −0 likes, 3 changes pushed, 1 playlist(s) · 1 error(s)", msg)
        assertTrue(errors)
        assertTrue(SettingsRules.isSetupError("Set the Public URL in Settings → General first (sign-in needs a callback address)"))
    }

    @Test
    fun signInInATabIsDoneWhenTheServiceSaysSo() {
        assertFalse(SettingsRules.signInDone("spotify", null))
        assertFalse(SettingsRules.signInDone("spotify", JsonNull))
        assertTrue(SettingsRules.signInDone("spotify", obj("""{"service":"spotify","account":"me"}""")))
        // Last.fm by username only is already there before the sign-in: not done until it can scrobble
        assertFalse(SettingsRules.signInDone("lastfm", obj("""{"service":"lastfm","account":"me","can_scrobble":false,"scrobble":true}""")))
        assertFalse(SettingsRules.signInDone("lastfm", obj("""{"service":"lastfm","account":"me"}""")))
        assertTrue(SettingsRules.signInDone("lastfm", obj("""{"service":"lastfm","account":"me","can_scrobble":true,"scrobble":true}""")))
        assertEquals(240_000L, SettingsRules.signInWaitMs(plex = true))
        assertEquals(120_000L, SettingsRules.signInWaitMs(plex = false))
    }

    @Test
    fun tidalAndYouTube() {
        assertEquals(true, SettingsRules.tidalState(obj("""{"partial":true}""")).second)
        assertEquals("Signed in – Tidal downloads use this account.", SettingsRules.tidalState(obj("""{"signed_in":true}""")).first)
        assertEquals("Step 2 of 2 · ", SettingsRules.tidalStepPrefix(obj("""{"steps":2,"step":2}""")))
        assertEquals("", SettingsRules.tidalStepPrefix(obj("""{"steps":1}""")))
        val yt = obj("""{"source":"handoff","sources":{"handoff":{"has_cookies":true,"signed_in":true,"account":"me@x"},"pasted":{}}}""")
        assertEquals("Signed in" to "ok", SettingsRules.youtubeBadge(yt))
        assertEquals("Handed-over sign-in: signed in as me@x", SettingsRules.youtubeSourceLine("handoff", yt["sources"]!!.jsonObject["handoff"]!!.jsonObject))
        assertEquals("Pasted cookies: none", SettingsRules.youtubeSourceLine("pasted", null))
        assertEquals("Everyday browser (extension): not connected", SettingsRules.youtubeSourceLine("extension", null))
        assertEquals("No cookies" to "", SettingsRules.youtubeBadge(obj("""{"source":null}""")))
        assertEquals(
            "Still signed in – Google sent no renewed cookies, so nothing was refreshed" to "warn",
            SettingsRules.youtubeRefreshToast(obj("""{"refreshed":true,"rotated":0,"source":"handoff","sources":{}}""")),
        )
    }

    @Test
    fun diagnosticsReport() {
        val d = obj(
            """{"version":"0.29.0","uid":1000,"checks":[{"kind":"service","name":"Prowlarr","ok":false,"request":"GET /api","status":401,"ms":12,"detail":"Unauthorized","body":"nope"}],
               "settings":[{"key":"base_url","source":"env","env":"ENCORE_BASE_URL","value":"https://x"},{"key":"app_name","value":null}],
               "machine":{"cpus":4,"memory":2147483648,"memory_source":"cgroup","config":{"path":"/config","fs":"ext4","network":false},"music":null,"database_bytes":null,"db_readers":4}}""",
        )
        val text = SettingsRules.diagnosticsReport(d) { b -> "${(b / 1073741824).toLong()} GB" }
        assertTrue(text.startsWith("Encore 0.29.0 diagnostics (uid 1000)\nFAIL Prowlarr – GET /api → HTTP 401 (12 ms)\n     Unauthorized\n     body: nope"))
        assertTrue(text.contains("  Memory: 2 GB (container limit)"))
        assertTrue(text.contains("  Config folder: /config · ext4 (local)"))
        assertTrue(text.contains("  Music folder: not set"))
        assertTrue(text.contains("  Database read connections: 4 (default)"))
        assertTrue(text.contains("  base_url = https://x  [env ENCORE_BASE_URL]"))
        assertTrue(text.contains("  app_name = null  [saved]"))
        assertEquals("Killed by the system", SettingsRules.crashKind("signaled"))
        assertEquals("Crash", SettingsRules.crashKind(""))
        // an app crash as its dialog and Copy have it: what, where, when, then the whole report
        assertEquals(
            "java.lang.IllegalStateException: boom\nCrash\nPixel 8 · 0.30.0 · Android 16 · caz\n<time>\n\nat a.b.C.d(C.kt:1)\nat e.f.G.h(G.kt:2)",
            SettingsRules.crashText("java.lang.IllegalStateException: boom", "Crash", "Pixel 8 · 0.30.0 · Android 16 · caz", "<time>", "at a.b.C.d(C.kt:1)\nat e.f.G.h(G.kt:2)\n"),
        )
        assertEquals("Crash\nNot responding\n<time>", SettingsRules.crashText("Crash", "Not responding", "", "<time>", "  "))
    }

    @Test
    fun appearanceColours() {
        assertEquals("#ff7a00", Appearance.cleanHex("FF7A00"))
        assertEquals("#aabbcc", Appearance.cleanHex("#abc"))
        assertNull(Appearance.cleanHex("#abcd"))
        assertNull(Appearance.cleanHex("orange"))
        // the same partner colour as web-rs/src/appearance.rs
        assertEquals("#ebff00", Appearance.partner("#ff7a00"))
        assertEquals("#e5e5e5", Appearance.partner("#e5e5e5"))
        assertEquals("#e75cf6", Appearance.partner("#8b5cf6"))
        assertEquals("#8b5cf6" to "#ec4899", Appearance.accentHex("violet", "#000000"))
        assertEquals("#ff7a00" to "#ebff00", Appearance.accentHex("custom", "#ff7a00"))
    }
}
