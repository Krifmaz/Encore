package app.encore.android.features.offline

import dev.krifmaz.kit.compose.offline.Outbox
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

/** Encore's plays and likes in the kit's outbox: merging, the plays body, the words and the earlier file. */
class EncoreOutboxTest {
    private val ana = Outbox.account("https://music.example.com", "ana")

    @Test
    fun aPlayIsKeptOnceAndALikeKeepsTheLastTap() {
        var o = Outbox(ana)
        o = EncoreOutbox.addPlay(o, EncoreOutbox.Play(7, 31.0, 100))
        // the same song at the same second is the same play
        assertSame(o, EncoreOutbox.addPlay(o, EncoreOutbox.Play(7, 31.0, 100)))
        o = EncoreOutbox.addPlay(o, EncoreOutbox.Play(7, 12.0, 200))
        o = EncoreOutbox.addLike(o, EncoreOutbox.Like(7, true, 101))
        o = EncoreOutbox.addLike(o, EncoreOutbox.Like(7, false, 102))
        // the same tap again changes nothing; a song id of 0 is never kept
        assertSame(o, EncoreOutbox.addLike(o, EncoreOutbox.Like(7, false, 103)))
        assertSame(o, EncoreOutbox.addPlay(o, EncoreOutbox.Play(0, 5.0, 300)))
        assertEquals(listOf(EncoreOutbox.Play(7, 31.0, 100), EncoreOutbox.Play(7, 12.0, 200)), EncoreOutbox.plays(o).map { it.second })
        assertEquals(listOf(EncoreOutbox.Like(7, false, 102)), EncoreOutbox.likes(o).map { it.second })
        // what went is forgotten; the rest stays
        val sent = EncoreOutbox.plays(o).take(1).map { it.first }
        assertEquals(2, o.forget(sent).size)
    }

    @Test
    fun thePlaysBodyAndTheWords() {
        val body = EncoreOutbox.playsBody(listOf(EncoreOutbox.Play(7, 31.26, 100), EncoreOutbox.Play(8, -1.0, 101)))
        assertEquals(
            """{"plays":[{"track_id":7,"seconds":31.3,"played_at":100},{"track_id":8,"seconds":0.0,"played_at":101}]}""",
            body.toString(),
        )
        assertEquals("", EncoreOutbox.countsText(0, 0))
        assertEquals("1 play", EncoreOutbox.countsText(1, 0))
        assertEquals("2 likes", EncoreOutbox.countsText(0, 2))
        assertEquals("3 plays and 1 like", EncoreOutbox.countsText(3, 1))
        assertEquals("Back online", EncoreOutbox.backOnlineText(0, 0))
        assertEquals("Back online – 3 plays sent", EncoreOutbox.backOnlineText(3, 0))
    }

    @Test
    fun theEarlierOutboxFileIsRead() {
        val text = """{"owner":"$ana","plays":[{"track_id":7,"seconds":31.5,"played_at":100},{"track_id":0,"seconds":1,"played_at":1}],""" +
            """"likes":[{"track_id":9,"liked":true,"at":120}]}"""
        val o = EncoreOutbox.fromLegacy(text)!!
        assertEquals(ana, o.owner)
        assertEquals(listOf(EncoreOutbox.Play(7, 31.5, 100)), EncoreOutbox.plays(o).map { it.second })
        assertEquals(listOf(EncoreOutbox.Like(9, true, 120)), EncoreOutbox.likes(o).map { it.second })
        // the kit's own form, an empty file and junk are not the earlier file
        assertNull(EncoreOutbox.fromLegacy(o.toJson().toString()))
        assertNull(EncoreOutbox.fromLegacy("{}"))
        assertNull(EncoreOutbox.fromLegacy("not json"))
    }
}
