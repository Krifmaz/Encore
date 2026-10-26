package app.encore.android.data.api

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import okhttp3.HttpUrl.Companion.toHttpUrl
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The local network address switch: which URLs move to the address in use, and which address is picked. */
class RouteRulesTest {
    private val atHome = Route(
        home = "https://encore.example.uk",
        local = "http://10.0.1.7:8080",
        public = "https://encore.example.uk",
        serverId = "abc",
        via = Via.LOCAL,
    )

    @Test
    fun requestsOnTheHomeAddressMoveToTheLocalOne() {
        val moved = RouteRules.rewrite(atHome, "https://encore.example.uk/api/library/tracks/5/stream?transcode=true".toHttpUrl())
        assertEquals("http://10.0.1.7:8080/api/library/tracks/5/stream?transcode=true", moved.toString())
    }

    @Test
    fun otherHostsAndTheHomeRouteStayAsTheyAre() {
        assertNull("a cover on a CDN", RouteRules.rewrite(atHome, "https://cdn.example.net/a.jpg".toHttpUrl()))
        assertNull("a look-alike host", RouteRules.rewrite(atHome, "https://encore.example.uk.evil.test/api".toHttpUrl()))
        assertNull("the home address in use", RouteRules.rewrite(atHome.copy(via = Via.HOME), "https://encore.example.uk/api/x".toHttpUrl()))
    }

    @Test
    fun signedInLocallyAwayFromHomeUsesThePublicAddress() {
        val away = atHome.copy(home = "http://10.0.1.7:8080", via = Via.PUBLIC)
        assertEquals("https://encore.example.uk/api/auth/me", RouteRules.rewrite(away, "http://10.0.1.7:8080/api/auth/me".toHttpUrl()).toString())
        assertEquals("encore.example.uk (public address)", RouteRules.summary(away, "http://10.0.1.7:8080"))
    }

    @Test
    fun aServerBehindAPathKeepsTheRestOfThePath() {
        val r = atHome.copy(home = "https://example.uk/encore")
        assertEquals("http://10.0.1.7:8080/api/x", RouteRules.rewrite(r, "https://example.uk/encore/api/x".toHttpUrl()).toString())
        assertNull("outside the server's path", RouteRules.rewrite(r, "https://example.uk/other".toHttpUrl()))
    }

    @Test
    fun theLocalAddressWinsWhenItAnswers() {
        assertEquals(Via.LOCAL, RouteRules.pick(atHome, { true }, { true }))
        assertEquals("public is home already", Via.HOME, RouteRules.pick(atHome, { false }, { true }))
        val signedInLocally = atHome.copy(home = "http://10.0.1.7:8080")
        assertEquals("at home: the home address is the local one", Via.HOME, RouteRules.pick(signedInLocally, { true }, { true }))
        assertEquals("away", Via.PUBLIC, RouteRules.pick(signedInLocally, { false }, { true }))
        assertEquals("nothing answers", Via.HOME, RouteRules.pick(signedInLocally, { false }, { false }))
    }

    @Test
    fun theServerAnswerIsCleaned() {
        val answer = JsonObject(mapOf("local" to JsonPrimitive(" http://10.0.1.7:8080/ "), "public" to JsonPrimitive("encore.example.uk"), "server_id" to JsonPrimitive("abc")))
        val r = RouteRules.withAnswer(Route(home = "https://x"), answer)
        assertEquals("http://10.0.1.7:8080", r.local)
        assertEquals("not an http(s) address", "", r.public)
        assertEquals("abc", r.serverId)
        assertTrue(RouteRules.same("HTTP://10.0.1.7:8080/", "http://10.0.1.7:8080"))
        assertFalse(RouteRules.same("", ""))
        assertEquals("encore.example.uk", RouteRules.summary(atHome.copy(via = Via.HOME), "https://encore.example.uk"))
        assertEquals("10.0.1.7:8080 (local network)", RouteRules.summary(atHome, "https://encore.example.uk"))
    }
}
