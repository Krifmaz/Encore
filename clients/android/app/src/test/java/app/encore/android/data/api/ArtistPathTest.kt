package app.encore.android.data.api

import okhttp3.HttpUrl.Companion.toHttpUrl
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * `/api/library/artists/{name}/liked` and `/tracks` (client-shapes §3.2, §3.3): the server takes the name from `name=`,
 * so the path segment only has to reach the route unchanged.
 */
class ArtistPathTest {
    @Test
    fun theNameGoesInThePathAndInTheQuery() {
        assertEquals("/api/library/artists/Gnarls%20Barkley/liked?name=Gnarls%20Barkley", EncoreApi.artistPath("Gnarls Barkley", "liked"))
        assertEquals("/api/library/artists/Sigur%20R%C3%B3s/tracks?name=Sigur%20R%C3%B3s", EncoreApi.artistPath("Sigur Rós", "tracks"))
    }

    @Test
    fun aSlashOrAPathStepPutsAnUnderscoreInThePath() {
        assertEquals("/api/library/artists/_/tracks?name=AC%2FDC", EncoreApi.artistPath("AC/DC", "tracks"))
        for (name in listOf(".", "..", "...", "a\\b", "", " ")) assertEquals(name, "_", EncoreApi.artistSegment(name))
        assertEquals(".38%20Special", EncoreApi.artistSegment(".38 Special"))
    }

    @Test
    fun theRequestReachesTheArtistRoute() {
        val base = "https://music.example.org"
        // OkHttp resolves "." and ".." before sending: the old path went to /api/library/liked
        assertEquals("/api/library/liked", "$base/api/library/artists/../liked".toHttpUrl().encodedPath)
        for (name in listOf("..", ".", "AC/DC", "Sigur Rós")) {
            val url = (base + EncoreApi.artistPath(name, "liked")).toHttpUrl()
            assertEquals(name, 5, url.pathSegments.size)
            assertEquals(name, "liked", url.pathSegments[4])
            assertEquals(name, name, url.queryParameter("name"))
        }
    }
}
