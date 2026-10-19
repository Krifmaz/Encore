package app.encore.android.features.group

import app.encore.android.core.EncoreJson
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class GroupRulesTest {
    private fun state(json: String): GroupState = GroupState.from(EncoreJson.parseToJsonElement(json))!!

    private val two = """
        {"id": "g1", "code": "k7qm4x", "rev": 4,
         "host": {"user": 3, "name": "Alex", "avatar": "/api/users/3/avatar"},
         "queue": [
           {"id": 11, "title": "One", "artist": "A", "duration": 200, "qid": "q1", "added_by": {"user": 3, "name": "Alex"}},
           {"id": 12, "title": "Two", "artist": "B", "duration": 100, "qid": "q2", "added_by": {"user": 5, "name": "Sam"}}
         ],
         "index": 0, "playing": true, "position": 50.0, "anchor_ms": 1000000,
         "members": [
           {"member": "m3", "user": 3, "name": "Alex", "device_name": "Pixel", "host": true},
           {"member": "m5", "user": 5, "name": "Sam", "device_name": "Firefox"}
         ],
         "everyone_controls": false, "visibility": "anyone", "max_members": 20, "server_max_members": 20,
         "you": {"member": "m5", "host": false, "can_control": false}}
    """.trimIndent()

    @Test
    fun readsTheState() {
        val s = state(two)
        assertEquals("K7QM4X", s.code)
        assertEquals("Alex", s.hostName)
        assertEquals(2, s.queue.size)
        assertEquals("Sam", s.queue[1].addedBy)
        assertEquals("q2", s.queue[1].qid)
        assertEquals(11L, s.track?.id)
        assertEquals("m5", s.youMember)
        assertFalse(s.youCanControl)
        assertEquals("Group · 2", GroupRules.pill(s))
        assertEquals("Only Alex controls playback", GroupRules.onlyHost(s))
    }

    @Test
    fun anOlderAnswerWithoutYouLetsTheHostAndEveryoneControl() {
        val s = state("""{"id": "g", "everyone_controls": true, "queue": []}""")
        assertTrue(s.youCanControl)
        assertEquals(-1, s.index)
        assertNull(s.track)
        assertNull(GroupState.from(EncoreJson.parseToJsonElement("""{"code": "X"}""")))
    }

    @Test
    fun codesAndLinks() {
        assertEquals("K7QM4X", GroupRules.codeFrom("k7qm4x"))
        assertEquals("K7QM4X", GroupRules.codeFrom(" K7Q M4X "))
        assertEquals("K7QM4X", GroupRules.codeFrom("https://music.example.com/#/group/K7QM4X"))
        assertEquals("K7QM4X", GroupRules.codeFrom("Listen with me on Encore (code K7QM4X):\nhttps://music.example.com/#/group/k7qm4x"))
        assertNull(GroupRules.codeFrom("hello there"))
        assertNull(GroupRules.codeFrom("ab"))
        assertEquals("https://music.example.com", GroupRules.serverOf("Listen: https://music.example.com/#/group/K7QM4X"))
        assertEquals("http://10.0.0.2:8080/encore", GroupRules.serverOf("http://10.0.0.2:8080/encore/#/group/ABCDEF"))
        assertNull(GroupRules.serverOf("K7QM4X"))
        assertEquals("K7Q M4X", GroupRules.spaced("K7QM4X"))
    }

    @Test
    fun theInviteLinkIsTheServersOrBuiltLikeTheWeb() {
        val s = state(two)
        assertEquals("https://m.example/#/group/K7QM4X", GroupRules.inviteLink(s, "https://m.example/"))
        val withUrl = state("""{"id": "g", "code": "ABCDEF", "invite_url": "https://public.example/#/group/ABCDEF"}""")
        assertEquals("https://public.example/#/group/ABCDEF", GroupRules.inviteLink(withUrl, "http://lan:8080"))
    }

    @Test
    fun expectedPositionRunsOnFromTheAnchor() {
        val s = state(two)
        // 10 s after the anchor: 50 + 10
        val (i, pos) = GroupRules.expected(s, 1_010_000)
        assertEquals(0, i)
        assertEquals(60.0, pos, 0.001)
        // a clock behind the anchor never goes back
        assertEquals(50.0, GroupRules.expected(s, 999_000).second, 0.001)
    }

    @Test
    fun aSongThatWouldHaveEndedRollsIntoTheNext() {
        val s = state(two)
        // 160 s after: 50 + 160 = 210 → 10 s into the second song
        val (i, pos) = GroupRules.expected(s, 1_160_000)
        assertEquals(1, i)
        assertEquals(10.0, pos, 0.001)
        // past the end of the queue: the end of the last song
        val (j, end) = GroupRules.expected(s, 2_000_000)
        assertEquals(1, j)
        assertEquals(100.0, end, 0.001)
    }

    @Test
    fun aPausedGroupStaysPut() {
        val s = state(two).copy(playing = false)
        assertEquals(0 to 50.0, GroupRules.expected(s, 5_000_000))
    }

    @Test
    fun theSyncRule() {
        assertEquals(GroupRules.Drift.SEEK, GroupRules.drift(600, adjusting = false, speedOk = true))
        assertEquals(GroupRules.Drift.SEEK, GroupRules.drift(-501, adjusting = true, speedOk = true))
        assertEquals(GroupRules.Drift.SLOWER, GroupRules.drift(300, adjusting = false, speedOk = true))
        assertEquals(GroupRules.Drift.FASTER, GroupRules.drift(-300, adjusting = false, speedOk = true))
        assertEquals(GroupRules.Drift.NONE, GroupRules.drift(100, adjusting = false, speedOk = true))
        // once adjusting, on until within 0.05 s
        assertEquals(GroupRules.Drift.SLOWER, GroupRules.drift(100, adjusting = true, speedOk = true))
        assertEquals(GroupRules.Drift.NORMAL, GroupRules.drift(30, adjusting = true, speedOk = true))
        // bit-perfect or Cadenza here: seek only
        assertEquals(GroupRules.Drift.NONE, GroupRules.drift(300, adjusting = false, speedOk = false))
        assertEquals(GroupRules.Drift.SEEK, GroupRules.drift(700, adjusting = false, speedOk = false))
    }

    @Test
    fun theClockOffsetTakesTheShortestRoundTrip() {
        val samples = listOf(
            Triple(1000L, 6000L, 1400L), // 400 ms
            Triple(2000L, 7010L, 2040L), // 40 ms: offset 7010 − 2020 = 4990
            Triple(3000L, 8000L, 3300L),
        )
        assertEquals(4990L, GroupRules.clockOffset(samples))
        assertNull(GroupRules.clockOffset(emptyList()))
    }

    @Test
    fun joinsAndLeavesAreToasts() {
        val before = state(two)
        val after = before.copy(
            members = before.members.filter { it.key != "m3" } + GroupMember("m9", 9, "Kim", "", "Phone", 0.0, false, false),
            rev = 5,
        )
        val said = GroupRules.comings(before, after)
        assertTrue(said.contains("Kim joined"))
        assertTrue(said.contains("Alex left"))
        // this device is never announced, and there is nothing to compare at the start
        assertTrue(GroupRules.comings(null, after).isEmpty())
        val me = after.copy(members = after.members + GroupMember("m5b", 5, "Sam", "", "Tablet", 0.0, false, false), youMember = "m5b")
        assertFalse(GroupRules.comings(after.copy(youMember = "m5b"), me).any { it.startsWith("Sam") })
    }

    @Test
    fun theListAndTheEnd() {
        val g = GroupSummary.from(EncoreJson.parseToJsonElement(
            """{"id": "g1", "host": {"name": "Alex"}, "track": {"title": "One", "artist": "A", "album_id": "al"}, "playing": true, "members_count": 3, "visibility": "anyone", "mine": false}""",
        ) as kotlinx.serialization.json.JsonObject)
        assertNotNull(g)
        assertEquals("Alex · 3 listening", GroupRules.tileLine(g!!))
        assertEquals("The group listen ended (the server restarted)", GroupRules.endedText("restart", "Alex"))
        assertEquals("Alex ended the group listen", GroupRules.endedText("ended", "Alex"))
        assertEquals("Group listens are turned off on this server", GroupRules.endedText("off", ""))
    }

    @Test
    fun aSlowSeekIsLearntAndDoesNotLoop() {
        // a server-made stream that starts playing 0.8 s after every seek, 2 s behind the group at first; GroupSync
        // looks every 0.5 s, not within 2.5 s of its own seek
        val lag = SeekLag()
        val latency = 800L
        var seekAt = -1L
        var seekTarget = 0L
        fun group(x: Long) = 60_000L + x
        fun phone(x: Long) = when {
            seekAt < 0 -> 58_000L + x
            x < seekAt + latency -> seekTarget
            else -> seekTarget + (x - seekAt - latency)
        }
        var quietUntil = 0L
        var seeks = 0
        for (x in 500L..120_000L step 500L) {
            if (x < quietUntil) continue
            val d = phone(x) - group(x)
            if (lag.waiting) lag.landed("q1", d, x)
            if (GroupRules.drift(d, adjusting = false, speedOk = false, seekAboveMs = lag.seekAboveMs("q1")) == GroupRules.Drift.SEEK) {
                seekTarget = lag.target("q1", group(x), serverMade = true)
                seekAt = x
                quietUntil = x + 2500L
                seeks++
            }
        }
        // the first seek lands 0.8 s behind; the second one aims 0.8 s ahead and lands on the group's spot
        assertEquals(2, seeks)
        assertEquals(800L, lag.lagMs(serverMade = true))
        assertEquals(0L, lag.lagMs(serverMade = false))
        assertEquals(0L, phone(120_000L) - group(120_000L))
    }

    @Test
    fun seeksThatKeepMissingStopForTheSong() {
        val lag = SeekLag()
        lag.target("q1", 10_000L, serverMade = true)
        lag.landed("q1", -900L, 3_000L)
        assertEquals(900L, lag.lagMs(serverMade = true))
        assertEquals(500L, lag.seekAboveMs("q1"))
        // a second seek 6 s later misses too (the lag varies): no more seeking in this song below 3 s
        lag.target("q1", 20_000L, serverMade = true)
        lag.landed("q1", 700L, 9_000L)
        assertEquals(200L, lag.lagMs(serverMade = true))
        assertEquals(3000L, lag.seekAboveMs("q1"))
        assertEquals(GroupRules.Drift.FASTER, GroupRules.drift(-1200L, adjusting = false, speedOk = true, seekAboveMs = lag.seekAboveMs("q1")))
        assertEquals(GroupRules.Drift.SEEK, GroupRules.drift(-3500L, adjusting = false, speedOk = true, seekAboveMs = lag.seekAboveMs("q1")))
        // the next song seeks as usual; a result in another song teaches nothing; the lag stays within 0 … 3 s
        assertEquals(500L, lag.seekAboveMs("q2"))
        lag.target("q2", 0L, serverMade = false)
        lag.landed("q3", -2000L, 10_000L)
        assertEquals(0L, lag.lagMs(serverMade = false))
        lag.target("q2", 0L, serverMade = false)
        lag.landed("q2", -9000L, 40_000L)
        assertEquals(3000L, lag.lagMs(serverMade = false))
    }
}
