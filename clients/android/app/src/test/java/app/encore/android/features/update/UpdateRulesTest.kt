package app.encore.android.features.update

import app.encore.android.core.parseJson
import app.encore.android.core.parseJsonObject
import app.encore.android.core.str
import app.encore.android.features.update.UpdateRules.Asset
import app.encore.android.features.update.UpdateRules.Check
import app.encore.android.features.update.UpdateRules.Phase
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class UpdateRulesTest {
    private val dl = UpdateRules.DOWNLOADS

    private fun json(text: String): JsonObject = parseJsonObject(text) ?: error("not JSON: $text")

    /** GitHub's answer for a release with these files (all under the project's downloads). */
    private fun release(tag: String, vararg names: String, extra: String = "", body: String = "## Changes\\n- one"): JsonObject {
        val assets = names.joinToString(",") { n ->
            """{"name":"$n","browser_download_url":"$dl$tag/$n","size":1234,"uploader":{"login":"x"}}"""
        }
        return json(
            """{"tag_name":"$tag","html_url":"https://github.com/Krifmaz/Encore/releases/tag/$tag","body":"$body",""" +
                """"published_at":"2026-10-12T10:00:00Z","draft":false,"prerelease":false,"assets":[$assets]$extra}""",
        )
    }

    private fun newer(c: Check): UpdateRules.Release = (c as? Check.Newer)?.release ?: error("expected a newer release, got $c")

    @Test
    fun a_newer_release_brings_its_apk_and_the_sums() {
        val r = newer(UpdateRules.newest(release("v0.30.1", "Encore-android-0.30.1.apk", "SHA256SUMS", "Encore-0.30.1-windows-x86_64.exe"), "0.30.0"))
        assertEquals("0.30.1", r.version)
        assertEquals("v0.30.1", r.tag)
        assertEquals("Encore-android-0.30.1.apk", r.apk.name)
        assertEquals("${dl}v0.30.1/Encore-android-0.30.1.apk", r.apk.url)
        assertEquals(1234L, r.apk.size)
        assertEquals("${dl}v0.30.1/SHA256SUMS", r.sums?.url)
        assertEquals("https://github.com/Krifmaz/Encore/releases/tag/v0.30.1", r.page)
        assertEquals("2026-10-12T10:00:00Z", r.publishedAt)
        assertEquals("Changes\n• one", UpdateRules.notesText(r.notes))
        assertFalse(r.notesCut)
    }

    @Test
    fun same_older_and_odd_releases_are_not_offered() {
        assertEquals(Check.None, UpdateRules.newest(release("v0.30.0", "Encore-android-0.30.0.apk"), "0.30.0"))
        assertEquals(Check.None, UpdateRules.newest(release("v0.29.9", "Encore-android-0.29.9.apk"), "0.30.0"))
        // a server-only release has nothing for the phone
        assertEquals(Check.None, UpdateRules.newest(release("v0.30.1", "encore-0.30.1-linux-x86_64.tar.gz", "SHA256SUMS"), "0.30.0"))
        assertEquals(Check.None, UpdateRules.newest(release("v0.31.0", "Encore-android-0.31.0.apk", extra = ""","prerelease":true"""), "0.30.0"))
        assertEquals(Check.None, UpdateRules.newest(release("v0.31.0", "Encore-android-0.31.0.apk", extra = ""","draft":true"""), "0.30.0"))
        // the legacy Tauri APK and other names are not the app's file
        assertEquals(Check.None, UpdateRules.newest(release("v0.31.0", "Encore-android-legacy-0.31.0.apk", "Encore-android-0.31.0.apk.sig"), "0.30.0"))
        // a file outside the project's release downloads is never used
        val elsewhere = json("""{"tag_name":"v0.31.0","assets":[{"name":"Encore-android-0.31.0.apk","browser_download_url":"https://example.com/Encore-android-0.31.0.apk"}]}""")
        assertEquals(Check.None, UpdateRules.newest(elsewhere, "0.30.0"))
        // the page link only to github.com
        val page = newer(UpdateRules.newest(release("v0.31.0", "Encore-android-0.31.0.apk").let { r ->
            json(r.toString().replace("https://github.com/Krifmaz/Encore/releases/tag/v0.31.0", "https://example.com/x"))
        }, "0.30.0")).page
        assertEquals(UpdateRules.RELEASES, page)
    }

    @Test
    fun only_files_of_the_projects_releases_are_fetched_as_okhttp_reads_them() {
        assertTrue(UpdateRules.isReleaseFile("${dl}v0.30.1/Encore-android-0.30.1.apk"))
        assertTrue(UpdateRules.isReleaseFile("${dl}v0.30.1/SHA256SUMS"))
        // dot segments that OkHttp resolves into another repository
        assertFalse(UpdateRules.isReleaseFile("${dl}../../../other/repo/releases/download/v1/Encore-android-9.9.9.apk"))
        assertFalse(UpdateRules.isReleaseFile("${dl}%2e%2e/%2E%2E/%2e%2e/other/repo/releases/download/v1/Encore-android-9.9.9.apk"))
        assertFalse(UpdateRules.isReleaseFile("${dl}v1/a/Encore-android-9.9.9.apk"))
        assertFalse(UpdateRules.isReleaseFile("${dl}v1%2F..%2Fx/Encore-android-9.9.9.apk"))
        assertFalse(UpdateRules.isReleaseFile("${dl}v0.30.1/"))
        assertFalse(UpdateRules.isReleaseFile("https://github.com.evil.example/Krifmaz/Encore/releases/download/v1/x.apk"))
        assertFalse(UpdateRules.isReleaseFile("http://github.com/Krifmaz/Encore/releases/download/v1/x.apk"))
        val sneaky = release("v0.31.0", "Encore-android-0.31.0.apk").let { r ->
            json(r.toString().replace("download/v0.31.0/", "download/../../../other/repo/releases/download/v0.31.0/"))
        }
        assertEquals(Check.None, UpdateRules.newest(sneaky, "0.30.0"))
    }

    @Test
    fun a_release_made_without_the_apps_offers_the_apk_it_carries_over() {
        val r = newer(UpdateRules.newest(release("v0.30.2", "Encore-android-0.30.1.apk", "SHA256SUMS"), "0.30.0"))
        assertEquals("0.30.1", r.version)
        assertEquals("v0.30.2", r.tag)
        assertEquals(Check.None, UpdateRules.newest(release("v0.30.2", "Encore-android-0.30.1.apk"), "0.30.1"))
    }

    @Test
    fun versions_compare_by_number_and_a_build_without_one_takes_any_release() {
        assertTrue(UpdateRules.isNewer("0.30.10", "0.30.9"))
        assertFalse(UpdateRules.isNewer("0.30.9", "0.30.10"))
        assertTrue(UpdateRules.isNewer("1.0.0", "0.99.99"))
        assertFalse(UpdateRules.isNewer("0.30.0", "0.30.0"))
        assertTrue(UpdateRules.isNewer("0.30.0", "0.0.1-dev"))
        assertTrue(UpdateRules.isNewer("0.30.0", "dev.12"))
        assertFalse(UpdateRules.isNewer("dev.13", "0.30.0"))
        assertTrue(UpdateRules.isReleaseVersion("0.30.0"))
        assertFalse(UpdateRules.isReleaseVersion("0.0.1-dev"))
        assertEquals(1, UpdateRules.compareVersions("v0.31.0", "0.30.99"))
        assertTrue(UpdateRules.newest(release("v0.30.0", "Encore-android-0.30.0.apk"), "dev.12") is Check.Newer)
        assertEquals("0.30.1", UpdateRules.apkVersion("Encore-android-0.30.1.apk"))
        assertNull(UpdateRules.apkVersion("Encore-android-dev.5.apk"))
    }

    @Test
    fun an_answer_without_a_tag_is_an_error_in_githubs_words() {
        assertEquals(Check.Error("Couldn't check for updates: Not Found"), UpdateRules.newest(json("""{"message":"Not Found"}"""), "0.30.0"))
        assertEquals(Check.Error("Couldn't check for updates: no release found"), UpdateRules.newest(json("{}"), "0.30.0"))
    }

    @Test
    fun the_kept_answer_reads_the_same_and_long_notes_are_cut() {
        val full = release("v0.30.1", "Encore-android-0.30.1.apk", "SHA256SUMS", extra = ""","author":{"login":"x"},"zipball_url":"z"""")
        assertEquals(UpdateRules.newest(full, "0.30.0"), UpdateRules.newest(UpdateRules.trim(full), "0.30.0"))
        val long = release("v0.30.1", "Encore-android-0.30.1.apk", body = "x".repeat(5000))
        val r = newer(UpdateRules.newest(UpdateRules.trim(long), "0.30.0"))
        assertEquals(UpdateRules.NOTES_MAX, r.notes.length)
        assertTrue(r.notesCut)
    }

    @Test
    fun sums_are_read_in_both_sha256sum_styles() {
        val a = "a".repeat(64)
        val b = "B".repeat(64)
        val m = UpdateRules.parseSums("$a  Encore-android-0.30.1.apk\r\n$b *./other.txt\nnot a line\n${"c".repeat(63)}  short.apk\n\n")
        assertEquals(a, m["Encore-android-0.30.1.apk"])
        assertEquals(b.lowercase(), m["other.txt"])
        assertEquals(2, m.size)
    }

    @Test
    fun a_file_that_does_not_match_its_sum_is_refused() {
        val h = "ab".repeat(32)
        val sums = "$h  Encore-android-0.30.1.apk\n"
        assertNull(UpdateRules.checkSums(sums, "Encore-android-0.30.1.apk", h.uppercase()))
        val bad = UpdateRules.checkSums(sums, "Encore-android-0.30.1.apk", "cd".repeat(32))
        assertNotNull(bad)
        assertTrue(bad!!.startsWith("The download does not match the release's SHA256SUMS, so it was not installed"))
        assertEquals(
            "Encore-android-0.30.2.apk is not listed in the release's SHA256SUMS, so it was not installed.",
            UpdateRules.checkSums(sums, "Encore-android-0.30.2.apk", h),
        )
    }

    @Test
    fun githubs_own_digest_is_checked_when_the_release_has_it() {
        val h = "ab".repeat(32)
        assertEquals(h, UpdateRules.digestHex("sha256:" + h.uppercase()))
        assertEquals("", UpdateRules.digestHex("sha512:$h"))
        assertEquals("", UpdateRules.digestHex(""))
        val a = Asset("x.apk", "u", 1, h)
        assertNull(UpdateRules.checkDigest(a, h))
        assertNotNull(UpdateRules.checkDigest(a, "cd".repeat(32)))
        assertNull(UpdateRules.checkDigest(a.copy(sha256 = ""), "cd".repeat(32)))
        val withDigest = json(
            """{"tag_name":"v0.30.1","assets":[{"name":"Encore-android-0.30.1.apk","browser_download_url":"${dl}v0.30.1/Encore-android-0.30.1.apk","size":5,"digest":"sha256:$h"}]}""",
        )
        assertEquals(h, newer(UpdateRules.newest(withDigest, "0.30.0")).apk.sha256)
        assertEquals(h, newer(UpdateRules.newest(UpdateRules.trim(withDigest), "0.30.0")).apk.sha256)
    }

    @Test
    fun only_the_same_key_or_a_proven_rotation_of_it_may_replace_the_app() {
        assertTrue(UpdateRules.signersMatch(setOf("k1"), setOf("k1"), listOf("k1")))
        assertFalse(UpdateRules.signersMatch(setOf("k1"), setOf("k2"), emptyList()))
        assertTrue(UpdateRules.signersMatch(setOf("k1"), setOf("k2"), listOf("k1", "k2")))
        assertFalse(UpdateRules.signersMatch(setOf("k1"), setOf("k2"), listOf("k0", "k2")))
        assertFalse(UpdateRules.signersMatch(emptySet(), emptySet(), emptyList()))
        assertFalse(UpdateRules.signersMatch(setOf("k1"), emptySet(), listOf("k1")))
        assertFalse(UpdateRules.signersMatch(setOf("k1", "k2"), setOf("k1"), emptyList()))
        assertTrue(UpdateRules.signersMatch(setOf("k1", "k2"), setOf("k2", "k1"), emptyList()))
    }

    @Test
    fun notes_read_as_plain_text() {
        val md = "<!-- hidden -->\n## Encore 0.30.1\n\n- **Bold** fix, see [the page](https://x)\n  * nested `code`\n\n---\n```\ncode\n```\n> quoted <https://a.b>\n![pic](https://p)\n"
        assertEquals("Encore 0.30.1\n\n• Bold fix, see the page\n  • nested code\n\ncode\nquoted https://a.b\npic", UpdateRules.notesText(md))
        assertEquals("", UpdateRules.notesText("<!-- only a comment"))
    }

    @Test
    fun the_words_for_each_phase() {
        val size: (Long) -> String = { "$it B" }
        assertEquals("", UpdateRules.describe(Phase.Idle, size))
        assertEquals("Encore is up to date.", UpdateRules.describe(Phase.UpToDate, size))
        assertEquals("Encore 0.30.1 is ready to install.", UpdateRules.describe(Phase.Available("0.30.1"), size))
        assertEquals("Downloading Encore 0.30.1: 5 B of 10 B…", UpdateRules.describe(Phase.Downloading("0.30.1", 5, 10), size))
        assertEquals("Downloading Encore 0.30.1: 5 B…", UpdateRules.describe(Phase.Downloading("0.30.1", 5, 0), size))
        assertEquals("no", UpdateRules.describe(Phase.Failed("no"), size))
        val all = listOf(
            Phase.Checking, Phase.UpToDate, Phase.Available("1.0.0"), Phase.Downloading("1.0.0", 1, 2), Phase.Verifying("1.0.0"),
            Phase.NeedsPermission("1.0.0"), Phase.Installing("1.0.0"), Phase.Installing("1.0.0", confirm = true),
        )
        for (p in all) assertFalse("plain words: $p", UpdateRules.describe(p, size).contains('!'))
        assertTrue(UpdateRules.busy(Phase.Checking))
        assertTrue(UpdateRules.busy(Phase.Installing("1.0.0")))
        assertFalse(UpdateRules.busy(Phase.NeedsPermission("1.0.0")))
        assertFalse(UpdateRules.busy(Phase.Failed("no")))
        assertEquals("00ff10", UpdateRules.hex(byteArrayOf(0, -1, 16)))
    }

    @Test
    fun hotfixesAndPrereleasesArePassedOverForTheNewestFullRelease() {
        val list = parseJson(
            """[{"tag_name":"hotfix-0.35.2.17"},{"tag_name":"v0.36.0","prerelease":true},{"tag_name":"v0.35.2"},{"tag_name":"v0.35.1"}]""",
        ) as JsonArray
        assertEquals("v0.35.2", UpdateRules.fullRelease(list)?.str("tag_name"))
        assertNull(UpdateRules.fullRelease(parseJson("""[{"tag_name":"hotfix-0.35.2.17"}]""") as JsonArray))
    }
}
