package app.encore.android.features.requests

import app.encore.android.core.EncoreJson
import app.encore.android.core.decodeModel
import app.encore.android.data.repo.VideoJob
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The server's answers about a video downloaded for this phone (GET /api/video-links/{id}), read leniently. */
class VideoJobTest {
    private fun job(json: String): VideoJob =
        (EncoreJson.parseToJsonElement(json) as JsonObject).decodeModel(VideoJob.serializer())!!

    @Test
    fun readyJobCarriesItsNameAndSize() {
        val j = job("""{"id":"abc","state":"ready","progress":100.0,"title":"My clip","name":"My clip.mp4","size":18,"error":null}""")
        assertEquals("My clip.mp4", j.name)
        assertEquals(18L, j.size)
        assertEquals("", j.error)
        assertFalse(j.running)
    }

    @Test
    fun onlyQueuedAndDownloadingAreStillRunning() {
        assertTrue(job("""{"id":"a","state":"queued","title":null,"name":null}""").running)
        assertTrue(job("""{"id":"a","state":"downloading","progress":42.5}""").running)
        assertFalse(job("""{"id":"a","state":"failed","error":"ERROR: Video unavailable"}""").running)
    }
}
