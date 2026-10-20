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

    /**
     * The first `state` event the 0.31 server sent in a smoke run of the real binary (two devices of one account, three
     * songs; song rows straight from SQLite, with nulls). The next two events of that run came 3 s and 6 s later with
     * `index` 1 and 2 at `position` 0: the server moves on at a song's end with `anchor_ms` at that moment, which is
     * exactly where the phone's own prediction puts the group.
     */
    private val fromTheServer = """{"anchor_ms":1792503497853,"boot":"0e9336","code":"J68ANU","everyone_controls":false,"guests_allowed":false,"guests_possible":false,"host":{"avatar":"","name":"admin","user":1},"id":"g_b442cdbc96998cf60bc8","index":0,"invite_url":"http://localhost:18931/#/group/J68ANU","max_members":20,"members":[{"avatar":"","device_name":"Smoke PC","guest":false,"host":true,"joined_at":1792503497,"member":"m1","name":"admin","user":1},{"avatar":"","device_name":"Smoke phone","guest":false,"host":false,"joined_at":1792503497,"member":"m2","name":"admin","user":1}],"playing":true,"position":0.0,"queue":[{"added_by":{"guest":false,"name":"admin","user":1},"album":"Record","album_id":"al1","albumartist":null,"artist":"Band","artists":[{"in_library":false,"name":"Band"}],"bitrate":null,"disc":null,"duration":3.0,"format":"mp3","id":1,"qid":"q1","title":"Song 1","track":null,"year":null},{"added_by":{"guest":false,"name":"admin","user":1},"album":"Record","album_id":"al1","albumartist":null,"artist":"Band","artists":[{"in_library":false,"name":"Band"}],"bitrate":null,"disc":null,"duration":3.0,"format":"mp3","id":2,"qid":"q2","title":"Song 2","track":null,"year":null},{"added_by":{"guest":false,"name":"admin","user":1},"album":"Record","album_id":"al1","albumartist":null,"artist":"Band","artists":[{"in_library":false,"name":"Band"}],"bitrate":null,"disc":null,"duration":100.0,"format":"mp3","id":3,"qid":"q3","title":"Song 3","track":null,"year":null}],"rev":2,"server_max_members":20,"started_at":1792503497,"track":{"added_by":{"guest":false,"name":"admin","user":1},"album":"Record","album_id":"al1","albumartist":null,"artist":"Band","artists":[{"in_library":false,"name":"Band"}],"bitrate":null,"disc":null,"duration":3.0,"format":"mp3","id":1,"qid":"q1","title":"Song 1","track":null,"year":null},"visibility":"anyone","you":{"can_control":true,"host":true,"member":"m2"}}"""

    @Test
    fun readsTheServersOwnState() {
        val s = state(fromTheServer)
        assertEquals("g_b442cdbc96998cf60bc8", s.id)
        assertEquals("J68ANU", s.code)
        assertEquals(2L, s.rev)
        assertEquals("0e9336", s.boot)
        assertEquals("http://localhost:18931/#/group/J68ANU", GroupRules.inviteLink(s, "https://elsewhere.example"))
        assertEquals("admin", s.hostName)
        assertEquals(1L, s.hostUser)
        assertEquals(listOf("q1", "q2", "q3"), s.queue.map { it.qid })
        assertEquals("admin", s.queue[1].addedBy)
        assertEquals(1L, s.queue[1].addedByUser)
        // the row's nulls read as empty: no track number, the artist for the album artist, no bitrate
        val t = s.queue[0].track
        assertEquals("Song 1", t.title)
        assertEquals("Band", t.albumArtist)
        assertEquals(0, t.trackNo)
        assertEquals(0, t.bitrate)
        assertEquals("mp3", t.format)
        assertEquals(3.0, t.durationSec, 0.0)
        assertNull(t.gain)
        assertEquals(0, t.sampleRate)
        assertEquals(1L, s.track?.id)
        // one account on two devices: two members; `you` is this device's membership, the account hosts
        assertEquals(2, s.count)
        assertEquals("m2", s.youMember)
        assertTrue(s.youHost)
        assertTrue(s.youCanControl)
        assertEquals(20, s.maxMembers)
        assertEquals(20, s.serverMax)
        assertFalse(s.guestsPossible)
        assertEquals("anyone", s.visibility)
        assertEquals(1792503497853L, s.anchorMs)
        // where the server's next two events put the group
        assertEquals(1 to 0.0, GroupRules.expected(s, 1792503500853L))
        assertEquals(2 to 0.0, GroupRules.expected(s, 1792503503853L))
        assertEquals(1 to 0.5, GroupRules.expected(s, 1792503501353L))
    }

    /** §2.1's three fields on a queue row, as the server's enrichment (encore_library::loudness::stamp) writes them. */
    @Test
    fun queueRowsCarryLoudnessAndFormat() {
        fun entry(json: String) = GroupEntry.from(EncoreJson.parseToJsonElement(json) as kotlinx.serialization.json.JsonObject, 0)!!
        val hires = entry(
            """{"id": 7, "title": "Crazy", "album_id": "al_7", "format": "flac", "duration": 178.4, "qid": "q4",
                "added_by": {"user": 5, "name": "Sam", "guest": false},
                "gain": {"track": -7.21, "album": -6.84, "track_peak": 0.988, "album_peak": 1.0}, "sample_rate": 96000, "bit_depth": 24}""",
        ).track
        assertEquals(-7.21, hires.gain!!.track!!, 0.0)
        assertEquals(-6.84, hires.gain!!.album!!, 0.0)
        assertEquals(0.988, hires.gain!!.trackPeak!!, 0.0)
        assertEquals(1.0, hires.gain!!.albumPeak!!, 0.0)
        assertEquals(96000, hires.sampleRate)
        assertEquals(24, hires.bitDepth)
        // an album whose songs are all measured but this one has only the album's gain; a lossy file has no bit depth
        val lossy = entry("""{"id": 8, "title": "x", "album_id": "a", "format": "mp3", "qid": "q5", "gain": {"album": -3.5}, "sample_rate": 44100}""").track
        assertNull(lossy.gain!!.track)
        assertEquals(-3.5, lossy.gain!!.album!!, 0.0)
        assertEquals(0, lossy.bitDepth)
    }

    @Test
    fun joiningAgainAfterBeingDroppedIsNotALeave() {
        // dropped after 2 minutes without the stream, this phone joins again with the code: a new membership id
        val before = state(fromTheServer)
        val pc = before.members.first { it.key == "m1" }
        val again = before.copy(
            rev = 7,
            members = listOf(pc, GroupMember("m4", 1, "admin", "", "Smoke phone", 1792503700.0, false, false)),
            youMember = "m4",
        )
        assertTrue(GroupRules.comings(before, again).isEmpty())
        // someone else who left meanwhile is still said
        val kim = GroupMember("m3", 9, "Kim", "", "Firefox", 1792503600.0, false, false)
        assertEquals(listOf("Kim left"), GroupRules.comings(before.copy(members = before.members + kim), again))
    }

    @Test
    fun aRefusedJoinAgainSaysWhy() {
        // the server's sentence as it sends it (§3.9)
        assertEquals("This group listen is full", GroupRules.rejoinRefused(409, "This group listen is full"))
        // a proxy's answer without one
        assertEquals("Couldn't join", GroupRules.rejoinRefused(409, "HTTP 409"))
        assertEquals("Too many tries. Wait a moment and try again.", GroupRules.rejoinRefused(429, null))
        assertEquals("Group listens are turned off on this server", GroupRules.rejoinRefused(403, ""))
        assertEquals("Couldn't join", GroupRules.rejoinRefused(400, " "))
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
