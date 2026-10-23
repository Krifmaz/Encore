package app.encore.android.data.api

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Which answers offline mode keeps (ApiCache.kt): the signed-in server's `GET /api/…`, but never sign-in, the health
 * check, live state of other devices or group listens, song streams or covers, and nothing of another host.
 */
class ApiCacheTest {
    private val base = "https://music.example.org"

    @Test
    fun keepsTheServersApiPages() {
        assertTrue(ApiCache.cacheable("$base/api/library/recent?limit=12", base))
        assertTrue(ApiCache.cacheable("$base/api/playlists", base))
        assertTrue(ApiCache.cacheable("$base/api/auth/me", base))
        assertTrue(ApiCache.cacheable("$base/api/settings/schema", base))
    }

    @Test
    fun neverKeepsLiveStateSignInStreamsOrOtherHosts() {
        assertFalse(ApiCache.cacheable("$base/api/health", base))
        assertFalse(ApiCache.cacheable("$base/api/auth/login", base))
        assertFalse(ApiCache.cacheable("$base/api/remote/devices", base))
        assertFalse(ApiCache.cacheable("$base/api/group/abc", base))
        assertFalse(ApiCache.cacheable("$base/api/library/tracks/4/stream", base))
        assertFalse(ApiCache.cacheable("$base/api/library/albums/x/cover?s=400", base))
        assertFalse(ApiCache.cacheable("https://other.example.org/api/library/recent", base))
        assertFalse(ApiCache.cacheable("$base/s/token", base))
        assertFalse(ApiCache.cacheable("$base/api/library/recent", ""))
    }
}
