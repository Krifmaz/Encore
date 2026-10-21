package app.encore.android.features.group

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The group listen's sync contract (Oct 21) on the phone: drift → what to do, the seek lag, the clock and the rung.
 * The owner: group listen is choppy on every client, "small skips/jumps".
 */
class SyncRulesTest {
    private val long = 60_000L
    private fun speed(a: SyncRules.Act): Double = (a as SyncRules.Act.Speed).rate

    @Test
    fun underFortyMillisecondsNothingHappens() {
        assertEquals(SyncRules.Act.None, SyncRules.decide(0, 1.0, serverMade = false, speedOk = true, sinceSeekMs = long))
        assertEquals(SyncRules.Act.None, SyncRules.decide(39, 1.0, serverMade = false, speedOk = true, sinceSeekMs = long))
        assertEquals(SyncRules.Act.None, SyncRules.decide(-39, 1.0, serverMade = false, speedOk = true, sinceSeekMs = long))
    }

    @Test
    fun aSmallDriftIsANudgeEasedInNeverASeek() {
        // 40 ms ahead: 0.4 % slower, at once (one step)
        assertEquals(0.996, speed(SyncRules.decide(40, 1.0, false, true, long)), 1e-9)
        // 300 ms ahead wants 2 % slower (the most), reached a step of 0.4 % at a time
        assertEquals(0.98, SyncRules.targetRate(300, nudging = false), 1e-9)
        assertEquals(0.996, speed(SyncRules.decide(300, 1.0, false, true, long)), 1e-9)
        assertEquals(0.992, speed(SyncRules.decide(298, 0.996, false, true, long)), 1e-9)
        // behind: faster
        assertEquals(1.004, speed(SyncRules.decide(-300, 1.0, false, true, long)), 1e-9)
        // in proportion: 100 ms → 1 %
        assertEquals(1.01, SyncRules.targetRate(-100, nudging = false), 1e-9)
        // up to 1.5 s it never seeks
        assertEquals(0.996, speed(SyncRules.decide(1500, 1.0, false, true, long)), 1e-9)
        assertEquals(1.004, speed(SyncRules.decide(-1500, 1.0, false, true, long)), 1e-9)
    }

    @Test
    fun aNudgeGoesOnUntilUnderTwentyMillisecondsAndEasesOut() {
        // nudging at 0.3 %, 30 ms ahead: as it is
        assertEquals(SyncRules.Act.None, SyncRules.decide(30, 0.997, false, true, long))
        // a change under 0.2 % is not worth setting
        assertEquals(SyncRules.Act.None, SyncRules.decide(25, 0.997, false, true, long))
        // under 20 ms: back to 1.0, by 0.4 % a step at most
        assertEquals(1.0, speed(SyncRules.decide(19, 0.997, false, true, long)), 1e-9)
        assertEquals(0.994, speed(SyncRules.decide(19, 0.99, false, true, long)), 1e-9)
        // not nudging: 30 ms is left alone
        assertEquals(SyncRules.Act.None, SyncRules.decide(30, 1.0, false, true, long))
    }

    @Test
    fun overOneAndAHalfSecondsOneSeekThenEightSecondsOfNudgesOnly() {
        assertEquals(SyncRules.Act.Seek, SyncRules.decide(1501, 1.0, false, true, long))
        assertEquals(SyncRules.Act.Seek, SyncRules.decide(-4000, 1.02, false, true, SyncRules.HOLD_MS))
        // 5 s after a seek: the nudge, at most 2 %
        assertEquals(1.004, speed(SyncRules.decide(-4000, 1.0, false, true, 5000)), 1e-9)
        assertEquals(SyncRules.Act.None, SyncRules.decide(-4000, 1.02, false, true, 5000))
    }

    @Test
    fun aServerMadeStreamIsNeverSeekedUnderThreeSeconds() {
        assertEquals(1.004, speed(SyncRules.decide(-2999, 1.0, serverMade = true, speedOk = true, sinceSeekMs = long)), 1e-9)
        assertEquals(SyncRules.Act.Seek, SyncRules.decide(-3001, 1.0, serverMade = true, speedOk = true, sinceSeekMs = long))
        assertEquals(3000L, SyncRules.seekAboveMs(serverMade = true))
    }

    @Test
    fun withoutSpeedControlTheSeekBarsStayTheSame() {
        // bit-perfect USB or Cadenza on this phone: no nudge, and never a seek between 40 ms and 1.5 s (each one is a
        // flush of the audio track, a skip)
        assertEquals(SyncRules.Act.None, SyncRules.decide(400, 1.0, serverMade = false, speedOk = false, sinceSeekMs = long))
        assertEquals(SyncRules.Act.None, SyncRules.decide(1499, 1.0, serverMade = false, speedOk = false, sinceSeekMs = long))
        assertEquals(SyncRules.Act.Seek, SyncRules.decide(1501, 1.0, serverMade = false, speedOk = false, sinceSeekMs = long))
        assertEquals(1500L, SyncRules.seekAboveMs(serverMade = false))
        assertEquals(SyncRules.Act.None, SyncRules.decide(2000, 1.0, serverMade = true, speedOk = false, sinceSeekMs = long))
        // a nudge that was on when the speed stopped being allowed ends at once
        assertEquals(1.0, speed(SyncRules.decide(100, 1.01, serverMade = false, speedOk = false, sinceSeekMs = long)), 1e-9)
    }

    /** Looks every 0.5 s as GroupSync makes them; [lagMs] is what a seek really takes at that moment. */
    private class Run(start: Long, val serverMade: Boolean, val lagMs: (Long) -> Long) {
        var drift = start.toDouble()
        var rate = 1.0
        var seeks = 0
        var maxNudge = 0.0
        var maxStep = 0.0
        var smallSeek = false
        val lag = SeekLag()

        fun go(untilMs: Long = 180_000L): Run {
            var quietUntil = 0L
            var t = 500L
            while (t <= untilMs) {
                if (t >= quietUntil) {
                    val d = Math.round(drift)
                    if (lag.waiting) lag.landed("q1", d)
                    when (val a = SyncRules.decide(d, rate, serverMade, true, lag.sinceSeekMs(t))) {
                        SyncRules.Act.Seek -> {
                            if (Math.abs(d) <= SyncRules.seekAboveMs(serverMade)) smallSeek = true
                            val lead = lag.target("q1", 0L, serverMade, t)
                            drift = (lead - lagMs(t)).toDouble()
                            rate = 1.0
                            quietUntil = t + 2500L
                            seeks++
                        }
                        is SyncRules.Act.Speed -> {
                            maxStep = maxOf(maxStep, Math.abs(a.rate - rate))
                            rate = a.rate
                        }
                        SyncRules.Act.None -> {}
                    }
                }
                maxNudge = maxOf(maxNudge, Math.abs(rate - 1.0))
                drift += (rate - 1.0) * 500.0
                t += 500L
            }
            return this
        }
    }

    @Test
    fun aDriftUnderASecondAndAHalfClosesWithNoSeekAndNoJump() {
        for (start in listOf(300L, -300L, 1200L, -1200L, 60L)) {
            val r = Run(start, serverMade = false) { 0L }.go()
            assertEquals("no seek from $start ms", 0, r.seeks)
            assertTrue(r.maxNudge <= 0.02 + 1e-9)
            assertTrue("eased: ${r.maxStep}", r.maxStep <= 0.004 + 1e-9)
            assertTrue("settled from $start: ${r.drift}", Math.abs(r.drift) < SyncRules.SETTLED_MS)
            assertEquals(1.0, r.rate, 0.0)
        }
    }

    @Test
    fun aBusyServerCostsOneSeekNotOneEveryThreeSeconds() {
        // a server-made stream 5 s behind; restarts take 0.6 to 2.5 s, varying: one seek, then the nudge
        val takes = listOf(600L, 2500L, 900L, 1400L)
        var i = 0
        val r = Run(-5000L, serverMade = true) { takes[i++ % takes.size] }.go()
        assertEquals(1, r.seeks)
        assertFalse(r.smallSeek)
        assertTrue(Math.abs(r.drift) < SyncRules.SETTLED_MS)
        // a whole file 2.5 s behind: one seek, and the 0.15 s it took is learnt
        val w = Run(-2500L, serverMade = false) { 150L }.go()
        assertEquals(1, w.seeks)
        assertEquals(150L, w.lag.lagMs(serverMade = false))
    }

    // ---- the seek lag --------------------------------------------------------------------------------------

    /** One seek that really takes [took] ms, measured where it lands. */
    private fun SeekLag.seek(took: Long, serverMade: Boolean = true, song: String = "q1", at: Long = 0L) {
        val lead = target(song, 0L, serverMade, at)
        landed(song, lead - took)
    }

    @Test
    fun theLagIsTheMedianOfTheLastFiveSeeks() {
        val lag = SeekLag()
        // nothing measured yet: a server-made stream counts with 0.7 s, a whole file with 0
        assertEquals(700L, lag.lagMs(serverMade = true))
        assertEquals(0L, lag.lagMs(serverMade = false))
        lag.seek(1000)
        assertEquals(1000L, lag.lagMs(serverMade = true))
        lag.seek(800)
        assertEquals(900L, lag.lagMs(serverMade = true))
        lag.seek(900)
        // one slow restart does not teach the next seek to overshoot
        lag.seek(3000)
        lag.seek(850)
        assertEquals(900L, lag.lagMs(serverMade = true))
        // the oldest (1000) goes: 800 900 3000 850 820
        lag.seek(820)
        assertEquals(850L, lag.lagMs(serverMade = true))
        // whole files apart
        assertEquals(0L, lag.lagMs(serverMade = false))
        // within 0 … 3 s
        val slow = SeekLag()
        slow.seek(9000, serverMade = false)
        assertEquals(3000L, slow.lagMs(serverMade = false))
    }

    @Test
    fun aSeekLandingInAnotherSongOrCancelledTeachesNothing() {
        val lag = SeekLag()
        lag.target("q1", 10_000L, serverMade = true, nowMs = 0L)
        assertTrue(lag.waiting)
        lag.landed("q2", -2000L)
        assertFalse(lag.waiting)
        assertEquals(700L, lag.lagMs(serverMade = true))
        lag.target("q1", 10_000L, serverMade = true, nowMs = 0L)
        lag.cancel()
        lag.landed("q1", -2000L)
        assertEquals(700L, lag.lagMs(serverMade = true))
    }

    @Test
    fun theHoldAfterASeekAndTheGroupMovingItself() {
        val lag = SeekLag()
        assertTrue(lag.sinceSeekMs(1000L) >= SyncRules.HOLD_MS)
        assertEquals(10_700L, lag.target("q1", 10_000L, serverMade = true, nowMs = 1000L))
        assertEquals(4000L, lag.sinceSeekMs(5000L))
        // the host seeked: followed at once
        lag.release()
        assertTrue(lag.sinceSeekMs(5000L) >= SyncRules.HOLD_MS)
    }

    // ---- the clock -----------------------------------------------------------------------------------------

    /** A round trip of [rtt] ms at [at] whose server time says [offset]. */
    private fun sample(rtt: Long, offset: Long, at: Long = 1000L) = ClockSample(at, at + rtt / 2 + offset, at + rtt)

    @Test
    fun theJoinTakesTheMedianOfTheThreeFastestOfEight() {
        val c = GroupClock()
        assertFalse(c.measured)
        assertFalse(c.join(emptyList()))
        val eight = listOf(
            sample(400, 5600), sample(40, 5000), sample(300, 5400), sample(50, 4990),
            sample(60, 5200), sample(500, 6000), sample(45, 5010), sample(700, 7000),
        )
        assertTrue(c.join(eight))
        assertTrue(c.measured)
        assertEquals(5000L, c.offsetMs)
        assertEquals(40L, c.bestRttMs)
        assertEquals(5000L, sample(40, 5000).offsetMs)
    }

    @Test
    fun aRefreshIsBlendedInAndNeverStepsMoreThanFiftyMilliseconds() {
        val c = GroupClock()
        c.join(listOf(sample(40, 5000), sample(40, 5000), sample(40, 5000)))
        // 60 ms is kept (under 2 × 40), 90 and 200 ms are not (a busy server's late answer)
        assertTrue(c.refresh(listOf(sample(60, 5100), sample(90, 9000), sample(200, 9000))))
        assertEquals(5030L, c.offsetMs)
        // far off: 30 % of the way, but 50 ms at most at once
        assertTrue(c.refresh(listOf(sample(40, 6000))))
        assertEquals(5080L, c.offsetMs)
        assertTrue(c.refresh(listOf(sample(40, 4000))))
        assertEquals(5030L, c.offsetMs)
        // nothing kept: nothing changes
        assertFalse(c.refresh(listOf(sample(200, 0), sample(300, 0), sample(400, 0))))
        assertEquals(5030L, c.offsetMs)
    }

    @Test
    fun theBarForSlowAnswersFollowsTheNetwork() {
        val c = GroupClock()
        c.join(listOf(sample(2, 100)))
        // twice 2 ms would turn every sample away: at least 20 ms
        assertTrue(c.refresh(listOf(sample(15, 110))))
        assertEquals(103L, c.offsetMs)
        // Wi-Fi to mobile data: five refreshes with nothing kept, then the best of the last becomes the best
        val m = GroupClock()
        m.join(listOf(sample(10, 0)))
        repeat(4) { assertFalse(m.refresh(listOf(sample(150, 100)))) }
        assertFalse(m.refresh(listOf(sample(120, 100))))
        assertEquals(120L, m.bestRttMs)
        assertTrue(m.refresh(listOf(sample(150, 100))))
        assertEquals(30L, m.offsetMs)
        // a refresh before any join measures as a join
        val f = GroupClock()
        assertTrue(f.refresh(listOf(sample(80, 777))))
        assertEquals(777L, f.offsetMs)
        assertEquals(2L, GroupClock.median(listOf(3L, 1L, 2L)))
        assertEquals(1L, GroupClock.median(listOf(1L, 2L)))
    }

    // ---- quality, song changes, the queue ------------------------------------------------------------------

    @Test
    fun inAGroupTheRungIsFixedForTheSongAndTheOriginalPreferred() {
        // 1000 kbps for a 900 kbps FLAC: alone that is AAC 256 (25 % margin), in a group the original
        assertEquals(256, app.encore.android.data.prefs.Quality.chooseRung(1000.0, 900))
        assertEquals(0, GroupQuality.startRung(1000.0, 900))
        assertEquals(0, GroupQuality.startRung(null, 900))
        // a link that can't carry the original: as alone
        assertEquals(256, GroupQuality.startRung(500.0, 900))
        // mid-song: alone anything; in a group only a stall, once a song
        assertTrue(GroupQuality.mayChange(inGroup = false, stall = false, droppedThisSong = false))
        assertFalse(GroupQuality.mayChange(inGroup = true, stall = false, droppedThisSong = false))
        assertTrue(GroupQuality.mayChange(inGroup = true, stall = true, droppedThisSong = false))
        assertFalse(GroupQuality.mayChange(inGroup = true, stall = true, droppedThisSong = true))
    }

    @Test
    fun aSongChangeWithinThreeSecondsUsesWhatWasPreBuffered() {
        // the group is 2.5 s into the next song, this phone at the end of this one: it moves on by itself
        assertTrue(SyncRules.waitForSongChange(cur = 0, posMs = 200_000L, groupIndex = 1, groupPosMs = 2500L))
        assertFalse(SyncRules.waitForSongChange(cur = 0, posMs = 200_000L, groupIndex = 1, groupPosMs = 3500L))
        // this phone a moment early into the next song
        assertTrue(SyncRules.waitForSongChange(cur = 1, posMs = 1000L, groupIndex = 0, groupPosMs = 199_000L))
        assertFalse(SyncRules.waitForSongChange(cur = 1, posMs = 3200L, groupIndex = 0, groupPosMs = 199_000L))
        assertFalse(SyncRules.waitForSongChange(cur = 0, posMs = 1000L, groupIndex = 2, groupPosMs = 1000L))
        // a server-made song within 3 s starts from its pre-buffered beginning; a whole file at the spot
        assertEquals(0L, SyncRules.startAtMs(2000L, serverMade = true))
        assertEquals(3500L, SyncRules.startAtMs(3500L, serverMade = true))
        assertEquals(2000L, SyncRules.startAtMs(2000L, serverMade = false))
    }

    @Test
    fun aQueueEditKeepsWhatMatches() {
        // a song added at the end: the next song (pre-buffered) stays
        assertEquals(SyncRules.QueueEdit(2, 1), SyncRules.queueEdit(listOf(1L, 2L, 3L, 4L), 1, listOf(1L, 2L, 3L, 4L, 5L), 1))
        // Play next: what comes next changed
        assertEquals(SyncRules.QueueEdit(0, 1), SyncRules.queueEdit(listOf(1L, 2L, 3L, 4L), 1, listOf(1L, 2L, 9L, 3L, 4L), 1))
        // a song added before the playing one
        assertEquals(SyncRules.QueueEdit(2, 1), SyncRules.queueEdit(listOf(1L, 2L, 3L, 4L), 1, listOf(0L, 1L, 2L, 3L, 4L), 2))
        // the playing song moved down one: 6 goes before it, 7 is added again after it
        assertEquals(SyncRules.QueueEdit(0, 0), SyncRules.queueEdit(listOf(5L, 6L, 7L), 0, listOf(6L, 5L, 7L), 1))
        // a song removed further down: the next one stays
        assertEquals(SyncRules.QueueEdit(1, 0), SyncRules.queueEdit(listOf(5L, 6L, 7L, 8L), 0, listOf(5L, 6L, 8L), 0))
        // cleared
        assertEquals(SyncRules.QueueEdit(0, 0), SyncRules.queueEdit(listOf(5L, 6L, 7L), 0, listOf(5L), 0))
    }
}
