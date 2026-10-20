package app.encore.android.data.api

import okhttp3.HttpUrl.Companion.toHttpUrl
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The session stays on the server's host: what SessionOffHostInterceptor strips on a redirect, and what it keeps. */
class SessionRulesTest {
    private val server = "https://music.example.org".toHttpUrl()

    @Test
    fun sameHostIsSchemeHostAndPort() {
        assertTrue(SessionRules.sameHost("https://music.example.org/api/library/tracks".toHttpUrl(), server))
        assertTrue("a stored address in capitals with the default port", SessionRules.sameHost("https://music.example.org/x".toHttpUrl(), "HTTPS://Music.Example.org:443".toHttpUrl()))
        assertFalse("another host", SessionRules.sameHost("https://cdn.example.net/cover.jpg".toHttpUrl(), server))
        assertFalse("a look-alike host", SessionRules.sameHost("https://music.example.org.evil.test/".toHttpUrl(), server))
        assertFalse("plain http", SessionRules.sameHost("http://music.example.org/".toHttpUrl(), server))
        assertFalse("another port", SessionRules.sameHost("https://music.example.org:8443/".toHttpUrl(), server))
        val lan = "http://192.168.1.20:8080/encore".toHttpUrl()
        assertTrue(SessionRules.sameHost("http://192.168.1.20:8080/api/health".toHttpUrl(), lan))
        assertFalse(SessionRules.sameHost("http://192.168.1.20:9090/".toHttpUrl(), lan))
    }

    @Test
    fun onlyEncoresOwnCookieGoes() {
        assertNull("the session alone: no Cookie header left", SessionRules.cookieWithout("encore_session=tok123", "tok123"))
        assertEquals("a=1; b=2", SessionRules.cookieWithout("a=1; encore_session=tok123; b=2", "tok123"))
        assertEquals("a=1", SessionRules.cookieWithout("a=1;encore_session=tok123", "tok123"))
        assertEquals("another value is not Encore's session", "encore_session=other", SessionRules.cookieWithout("encore_session=other", "tok123"))
        assertEquals("a=1; b=2", SessionRules.cookieWithout("a=1; b=2", "tok123"))
        assertEquals("a longer value that starts with the token", "encore_session=tok1234", SessionRules.cookieWithout("encore_session=tok1234", "tok123"))
    }

    @Test
    fun onlyTheAppsOwnBearerGoes() {
        assertTrue(SessionRules.isOwnBearer("Bearer tok123", "tok123"))
        assertTrue(SessionRules.isOwnBearer("bearer tok123", "tok123"))
        assertFalse("another service's token", SessionRules.isOwnBearer("Bearer ghp_abc", "tok123"))
        assertFalse("GitHub's own scheme", SessionRules.isOwnBearer("token tok123", "tok123"))
        assertFalse("no token signed in", SessionRules.isOwnBearer("Bearer ", ""))
    }
}
