package app.encore.android.features.bugs

import app.encore.android.data.api.ApiException
import app.encore.android.features.bugs.BugRules.Filter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException
import java.net.ConnectException
import java.net.SocketTimeoutException
import java.net.UnknownHostException

class BugRulesTest {
    @Test
    fun labelsAndBadgesAreTheWebs() {
        assertEquals("Not a bug", BugRules.kindLabel("not_bug"))
        assertEquals("How do I…", BugRules.kindLabel("howto"))
        assertEquals("Bug", BugRules.kindLabel("something else"))
        assertEquals("err", BugRules.kindBadge("bug"))
        assertEquals("info", BugRules.kindBadge("howto"))
        assertEquals("", BugRules.kindBadge("not_bug"))
        assertEquals("Looking into it", BugRules.statusLabel("looking"))
        assertEquals("Won't fix", BugRules.statusLabel("wontfix"))
        assertEquals("New", BugRules.statusLabel(""))
        assertEquals("accent", BugRules.statusBadge("answered"))
        assertEquals(listOf("All", "New", "Looking", "Fixed", "Won't fix", "Answered"), BugRules.TABS.map { it.second })
    }

    @Test
    fun listQueryHasOnlyWhatIsSet() {
        assertEquals("", BugRules.listQuery(Filter(), admin = true))
        assertEquals("?status=fixed&kind=howto", BugRules.listQuery(Filter(status = "fixed", kind = "howto"), admin = false))
        assertEquals("?q=no%20sound%20%26%20more", BugRules.listQuery(Filter(q = "  no sound & more "), admin = false))
        // Mine is an admin's choice; everyone else only ever sees their own reports anyway
        assertEquals("?mine=1", BugRules.listQuery(Filter(mine = true), admin = true))
        assertEquals("", BugRules.listQuery(Filter(mine = true), admin = false))
    }

    @Test
    fun exportTakesStatusAndKindOnly() {
        val f = Filter(status = "new", kind = "bug", q = "crash", mine = true)
        assertEquals("?status=new&kind=bug&format=txt", BugRules.exportQuery(f))
        assertEquals("?format=txt", BugRules.exportQuery(Filter(q = "crash")))
        assertEquals("?ids=12&format=txt", BugRules.exportOneQuery(12))
        assertEquals("encore-bug-reports-2026-10-19.txt", BugRules.exportName("2026-10-19"))
        assertEquals("encore-bug-report-12-2026-10-19.txt", BugRules.exportName("2026-10-19", 12))
    }

    @Test
    fun keptFilterIsMadeValid() {
        assertEquals(Filter(), BugRules.normalize(Filter(status = "bogus", kind = "nope", mine = true), admin = false))
        assertEquals(Filter("looking", "not_bug", "x", true), BugRules.normalize(Filter("looking", "not_bug", "x", true), admin = true))
        assertFalse(Filter().active)
        assertFalse(Filter(q = "   ", mine = true).active)
        assertTrue(Filter(kind = "bug").active)
        assertTrue(Filter(status = "new").active)
    }

    @Test
    fun wordsOfTheListAndTheReport() {
        assertEquals("No replies", BugRules.countWords(0, 0))
        assertEquals("1 reply · 1 file", BugRules.countWords(1, 1))
        assertEquals("3 replies · 2 files", BugRules.countWords(3, 2))
        assertEquals("by ann · today", BugRules.metaLine("ann", "today", "", "", ""))
        assertEquals(
            "by ann · today · page Home · app 0.30.0 · EncoreAndroid/0.30.0",
            BugRules.metaLine("ann", "today", "Home", "0.30.0", "EncoreAndroid/0.30.0"),
        )
        assertEquals(
            "Sent along: the page you came from (Home), the app version (0.30.0) and this phone.",
            BugRules.sentAlong("Home", "0.30.0"),
        )
        assertEquals("Sent along: the app version (unknown) and this phone.", BugRules.sentAlong("", ""))
    }

    @Test
    fun picturesAreKnownByTheirBytes() {
        fun bytes(vararg b: Int) = ByteArray(b.size) { b[it].toByte() }
        assertEquals("png", BugRules.sniffImage(bytes(0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0)))
        assertEquals("jpg", BugRules.sniffImage(bytes(0xFF, 0xD8, 0xFF, 0xE1)))
        assertEquals("gif", BugRules.sniffImage("GIF89a....".toByteArray()))
        assertEquals("webp", BugRules.sniffImage("RIFF\u0000\u0000\u0000\u0000WEBPVP8 ".toByteArray()))
        // HEIC (an ISO box "ftypheic"), a short file and nothing at all are not taken as they are
        assertNull(BugRules.sniffImage(bytes(0, 0, 0, 0x18) + "ftypheic".toByteArray()))
        assertNull(BugRules.sniffImage(bytes(0xFF, 0xD8)))
        assertNull(BugRules.sniffImage(ByteArray(0)))
    }

    @Test
    fun whatGoesAsItIs() {
        val mb = 1024L * 1024
        assertTrue(BugRules.sendsAsIs("jpg", 3 * mb, 4000, 3000))
        assertTrue(BugRules.sendsAsIs("png", 8 * mb, 1080, 2400))
        assertFalse(BugRules.sendsAsIs(null, 3 * mb, 4000, 3000))
        assertFalse(BugRules.sendsAsIs("jpg", 8 * mb + 1, 4000, 3000))
        assertFalse(BugRules.sendsAsIs("jpg", 3 * mb, 9000, 1000))
        // 50 MP decodes to more than the server's 128 MiB
        assertFalse(BugRules.sendsAsIs("jpg", 7 * mb, 8160, 6144))
        assertFalse(BugRules.sendsAsIs("jpg", -1, 4000, 3000))
        assertFalse(BugRules.sendsAsIs("jpg", 3 * mb, 0, 0))
    }

    @Test
    fun smallerPicturesKeepTheirShape() {
        assertEquals(4096 to 3072, BugRules.fitInside(8160, 6120, 4096))
        assertEquals(1536 to 2048, BugRules.fitInside(3000, 4000, 2048))
        assertEquals(1000 to 800, BugRules.fitInside(1000, 800, 4096))
        assertEquals(4096 to 1, BugRules.fitInside(20000, 2, 4096))
    }

    @Test
    fun namesGetTheExtensionOfWhatIsSent() {
        assertEquals("IMG_1234.jpg", BugRules.fileName("IMG_1234.HEIC", "jpg"))
        assertEquals("Screenshot 2026.png", BugRules.fileName("Screenshot 2026.png", "png"))
        assertEquals("image.jpg", BugRules.fileName("", "jpg"))
        assertEquals("image.jpg", BugRules.fileName(".hidden", "jpg"))
        assertEquals("passwd.png", BugRules.fileName("../../etc/passwd", "png"))
    }

    @Test
    fun aCallThatNeverReachedTheServerSaysSoAsTheWeb() {
        assertEquals("Not connected to the server", BugRules.errorWords(UnknownHostException("Unable to resolve host \"x\"")))
        assertEquals("Not connected to the server", BugRules.errorWords(ConnectException("Failed to connect to x/10.0.0.1:8080")))
        assertEquals("Not connected to the server", BugRules.errorWords(SocketTimeoutException("timeout")))
        assertEquals("Not connected to the server", BugRules.errorWords(IOException("unexpected end of stream", java.io.EOFException())))
        assertEquals("Report not found", BugRules.errorWords(ApiException(404, "Report not found", null)))
        assertEquals("Unexpected reply from the server", BugRules.errorWords(IOException("Unexpected reply from the server")))
        assertEquals("Request failed", BugRules.errorWords(IllegalStateException()))
    }
}
