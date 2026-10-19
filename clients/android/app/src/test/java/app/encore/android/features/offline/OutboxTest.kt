package app.encore.android.features.offline

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files

class OutboxTest {
    private val ana = "https://music.example.com|ana"

    @Test
    fun aLikeThenAnUnlikeCollapsesIntoTheUnlike() {
        val o = Outbox(ana).addLike(OutLike(1, true, 100)).addLike(OutLike(1, false, 101))
        assertEquals(listOf(OutLike(1, false, 101)), o.likes)
        // the same tap twice changes nothing
        assertEquals(o, o.addLike(OutLike(1, false, 200)))
        assertEquals(o, o.addLike(OutLike(0, true, 1)))
    }

    @Test
    fun aPlayIsKeptOnce() {
        val o = Outbox(ana).addPlay(OutPlay(1, 120.0, 100)).addPlay(OutPlay(1, 30.0, 100)).addPlay(OutPlay(1, 30.0, 400))
        assertEquals(2, o.plays.size)
        assertEquals(o, o.addPlay(OutPlay(-1, 1.0, 1)))
    }

    @Test
    fun atMostFiveThousandPlaysWaitTheOldestGoFirst() {
        var o = Outbox(ana)
        for (i in 1..5003) o = o.addPlay(OutPlay(7, 10.0, i.toLong()))
        assertEquals(Outbox.MAX_PLAYS, o.plays.size)
        assertEquals(4L, o.plays.first().playedAt)
    }

    @Test
    fun anotherAccountsOutboxIsDropped() {
        val o = Outbox(ana).addPlay(OutPlay(1, 60.0, 5))
        assertTrue(o.belongsTo(ana))
        assertFalse(o.belongsTo("https://music.example.com|bob"))
        assertFalse(Outbox("").belongsTo(""))
        val bob = o.own("https://music.example.com|bob")
        assertTrue(bob.isEmpty)
        assertEquals(o, o.own(ana))
    }

    @Test
    fun whatWentIsForgotten() {
        val o = Outbox(ana)
            .addPlay(OutPlay(1, 60.0, 5)).addPlay(OutPlay(2, 60.0, 6))
            .addLike(OutLike(3, true, 7)).addLike(OutLike(4, false, 8))
        // the like of 3 changed meanwhile: it stays
        val now = o.addLike(OutLike(3, false, 9))
        val left = now.forget(listOf(OutPlay(1, 60.0, 5)), listOf(OutLike(3, true, 7), OutLike(4, false, 8)))
        assertEquals(listOf(OutPlay(2, 60.0, 6)), left.plays)
        assertEquals(listOf(OutLike(3, false, 9)), left.likes)
    }

    @Test
    fun theDocumentReadsBack() {
        val o = Outbox(ana).addPlay(OutPlay(1, 241.5, 1792310400)).addLike(OutLike(9, true, 1792310460))
        assertEquals(o, Outbox.parse(o.toJson().toString()))
        assertEquals(Outbox(), Outbox.parse("not json"))
        assertEquals(Outbox(), Outbox.parse(null))
    }

    @Test
    fun thePlaysGoInTheListForm() {
        val body = Outbox.playsBody(listOf(OutPlay(1234, 241.56, 1792310400), OutPlay(1235, -3.0, 1792310500))).toString()
        assertEquals("""{"plays":[{"track_id":1234,"seconds":241.6,"played_at":1792310400},{"track_id":1235,"seconds":0.0,"played_at":1792310500}]}""", body)
    }

    @Test
    fun backOnlineSaysWhatWasSent() {
        assertEquals("", Outbox.countsText(0, 0))
        assertEquals("1 play", Outbox.countsText(1, 0))
        assertEquals("3 plays and 1 like", Outbox.countsText(3, 1))
        assertEquals("2 likes", Outbox.countsText(0, 2))
        assertEquals("Back online", Outbox.backOnlineText(0, 0))
        assertEquals("Back online – 3 plays sent", Outbox.backOnlineText(3, 0))
    }

    @Test
    fun theFileKeepsEveryChange() {
        val dir = Files.createTempDirectory("outbox").toFile()
        try {
            val f = File(dir, "outbox.json")
            val box = OutboxFile(f)
            box.update { it.own(ana).addPlay(OutPlay(1, 30.0, 10)) }
            box.update { it.own(ana).addLike(OutLike(2, true, 11)) }
            val again = OutboxFile(f).read()
            assertEquals(1, again.plays.size)
            assertEquals(1, again.likes.size)
            assertEquals(ana, again.owner)
        } finally {
            dir.deleteRecursively()
        }
    }
}
