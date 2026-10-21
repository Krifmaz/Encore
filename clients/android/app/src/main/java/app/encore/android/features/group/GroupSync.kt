package app.encore.android.features.group

import android.os.Handler
import android.os.Looper
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import app.encore.android.data.api.Track
import app.encore.android.playback.MediaItems
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.launch
import kotlinx.serialization.json.put

/**
 * The playback service's side of a group listen: the player follows the group's queue, song, position and play /
 * pause ([GroupListen.state]), and what the listener does on this phone – the app, the notification, the lock screen,
 * headphones, the car – goes to the group when this device may control it (else the player goes back to the group,
 * and a pause stays a pause here only: "Paused here · Catch up").
 *
 * Sync rule (the same on web, desktop and phone, Oct 21; [SyncRules]): expected position = position +
 * (server_now − anchor_ms) / 1000 while playing. Drift under 40 ms: nothing; 40 ms to 1.5 s: the playback speed is
 * nudged in proportion (at most 0.98–1.02, eased in and out, until under 20 ms; the audio track keeps the pitch);
 * over 1.5 s one hard seek, aimed ahead by what seeks take ([SeekLag]), then no other seek for 8 s. A server-made
 * stream (asked for again at `t=` to seek: a new conversion on the server) is never seeked for drift under 3 s. With
 * bit-perfect USB or Cadenza on this phone the speed stays at 1.0 and the seek bars are the same. Crossfade is off
 * in a group (the service asks [active]); gapless stays, and a song change within 3 s of the group's moves into the
 * pre-buffered next song by itself.
 *
 * Only a person's own seek or jump goes to the group: the session's player marks those ([fromUser]); what the service
 * does by itself – error recovery, a stall's lower rung, a stream opened again, the corrections made here – never does.
 *
 * Main thread only. Lives as long as the service.
 */
class GroupSync(
    private val player: Player,
    /** the position in the song, ms (a server-made stream's offset included) */
    private val positionMs: () -> Long,
    /** seek in song time (server-made streams reopen at `t=`) */
    private val seekTo: (Long) -> Unit,
    /** the songs as the service's media items (it remembers them for the stream resolver) */
    private val items: (List<Track>) -> List<MediaItem>,
    /** a new queue replaces the old one */
    private val newQueue: () -> Unit,
    /** the playback speed may change (not with bit-perfect USB or Cadenza on this phone) */
    private val speedOk: () -> Boolean,
    /**
     * the song plays as a server-made stream (a transcode, Data saver or adaptive AAC, the server's Cadenza): no
     * Range, so a seek asks for it again at `t=` ([seekTo]) instead of seeking in the player
     */
    private val serverMade: (Track) -> Boolean = { false },
    /** in a group or not (the car's subtitle says "Group listen") */
    private val onActive: (Boolean) -> Unit = {},
) {
    private val main = Handler(Looper.getMainLooper())
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var job: Job? = null
    /** > 0 while this class changes the player: those events are not the listener's */
    private var applying = 0
    /** > 0 while a person's own command runs on the session's player ([fromUser]): only those go to the group */
    private var userDepth = 0
    private var groupId: String? = null
    /** the playback speed set here (1.0: no nudge) */
    private var speed = 1.0
    /** no drift check until then (elapsed ms): a seek needs a moment to buffer */
    private var quietUntil = 0L
    /**
     * A change made here went to the group (play, pause, a jump, a seek): until the group's answer (a state newer than
     * [holdRev]) or [holdUntil], the player is not pulled back to the older state.
     */
    private var holdUntil = 0L
    private var holdRev = -1L
    private var lastNudge = 0
    /** the group's own spot as last applied: when it changes, someone moved the group (followed without the hold) */
    private var lastSpot: Spot? = null
    /** what this phone's seeks take to start playing, and when the last one was made */
    private val lag = SeekLag()

    private data class Spot(val index: Int, val position: Double, val anchor: Long, val playing: Boolean)

    /** In a group now: the service turns crossfade and the library shuffle off. */
    val active: Boolean get() = groupId != null

    private val listener = object : Player.Listener {
        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
            if (applying > 0 || userDepth == 0 || groupId == null) return
            if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_SEEK) userJump(player.currentMediaItemIndex)
        }

        override fun onPositionDiscontinuity(oldPosition: Player.PositionInfo, newPosition: Player.PositionInfo, reason: Int) {
            if (applying > 0 || userDepth == 0 || groupId == null) return
            if (reason == Player.DISCONTINUITY_REASON_SEEK && oldPosition.mediaItemIndex == newPosition.mediaItemIndex) userSeek(positionMs())
        }

        override fun onPlayWhenReadyChanged(playWhenReady: Boolean, reason: Int) {
            if (applying > 0 || groupId == null) return
            userPlayPause(playWhenReady, reason)
        }

        override fun onIsPlayingChanged(isPlaying: Boolean) {
            if (groupId != null) schedule(if (isPlaying) 700L else 500L)
        }
    }

    fun start() {
        player.addListener(listener)
        job = scope.launch {
            combine(GroupListen.state, GroupListen.localPaused, GroupListen.nudge) { s, _, n -> s to n }.collect { (s, n) ->
                // a refused command or Catch up: the group's state wins again at once
                if (n != lastNudge) { lastNudge = n; holdUntil = 0L }
                follow(s)
            }
        }
    }

    fun stop() {
        player.removeListener(listener)
        job?.cancel()
        scope.cancel()
        main.removeCallbacks(tick)
        if (speed != 1.0) setSpeed(1.0)
    }

    /** A seek asked in song time (the app's seek bar through the service's custom command): the group's when allowed. */
    fun userSeek(ms: Long) {
        val s = GroupListen.state.value ?: return
        if (!s.youCanControl) { GroupListen.deniedHere(); later { follow(GroupListen.state.value) }; return }
        // where the group is anyway: nothing to tell (only a person's own seeks come here, so any real move is told)
        val (ei, epos) = GroupRules.expected(s, GroupListen.serverNow())
        if (player.currentMediaItemIndex == ei && Math.abs(ms - Math.round(epos * 1000)) < SyncRules.DEAD_MS) return
        hold(s)
        GroupListen.command("seek") { put("position", ms / 1000.0) }
    }

    /** The group was told about a change made here: its answer is awaited before following it again. */
    private fun hold(s: GroupState) {
        holdRev = s.rev
        holdUntil = now() + 3000L
        quietUntil = now() + 2500L
        if (speed != 1.0) setSpeed(1.0)
        // the listener's own seek or jump did not aim ahead: nothing to learn from where it lands
        lag.cancel()
    }

    private fun holding(s: GroupState): Boolean = now() < holdUntil && s.rev <= holdRev

    /** Runs [block] as the group's own change: the listener leaves its events alone. */
    fun <T> applying(block: () -> T): T {
        applying++
        try { return block() } finally { applying-- }
    }

    /**
     * Runs [block] as a person's own command on the player (the session's player: the app, the notification, the
     * lock screen, the car): its seek or jump goes to the group. Every other seek is the service's own.
     */
    fun <T> fromUser(block: () -> T): T {
        userDepth++
        try { return block() } finally { userDepth-- }
    }

    /**
     * The service opens the song playing again by itself (error recovery, a stall's lower rung, server processing
     * failing): where to open it (song time, ms) so it lands where the group will be by then, ahead by the seek lag
     * of the stream it opens ([serverMade]). Null outside a playing group, or on another song than the group's (the
     * spot it had stands; the sync moves it). It is no seek of the listener's and never goes to the group.
     */
    fun reloadAt(serverMade: Boolean): Long? {
        val s = GroupListen.state.value ?: return null
        if (groupId != s.id || !s.playing || GroupListen.localPaused.value) return null
        val (ei, epos) = GroupRules.expected(s, GroupListen.serverNow())
        if (ei < 0 || player.currentMediaItemIndex != ei || !sameQueue(s)) return null
        if (speed != 1.0) setSpeed(1.0)
        quietUntil = now() + 2500L
        return lag.target(s.queue[ei].qid, Math.round(epos * 1000), serverMade, now())
    }

    // ---- what this phone's listener did ---------------------------------------------------------------------

    private fun userJump(index: Int) {
        val s = GroupListen.state.value ?: return
        if (!s.youCanControl) { GroupListen.deniedHere(); later { follow(GroupListen.state.value) }; return }
        val e = s.queue.getOrNull(index) ?: return
        val local = if (index in 0 until player.mediaItemCount) MediaItems.trackId(player.getMediaItemAt(index).mediaId) else null
        // the local queue no longer matches the group's: follow the group instead
        if (local != e.track.id) { later { follow(GroupListen.state.value) }; return }
        hold(s)
        if (index == s.index) GroupListen.command("seek") { put("position", positionMs() / 1000.0) }
        else GroupListen.command("jump") { put("index", index); put("qid", e.qid) }
    }

    private fun userPlayPause(playWhenReady: Boolean, reason: Int) {
        val s = GroupListen.state.value ?: return
        val byUser = reason == Player.PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST
        if (!playWhenReady) {
            // headphones out, another app took the sound, or a member's own pause: only here
            if (byUser && s.youCanControl && s.playing) { hold(s); GroupListen.command("pause") }
            else if (s.playing) GroupListen.setLocalPaused(true)
            return
        }
        if (!byUser) return
        when {
            GroupListen.localPaused.value -> GroupListen.catchUp()
            !s.playing && s.youCanControl -> { hold(s); GroupListen.command("play") }
            !s.playing -> { GroupListen.deniedHere(); later { follow(GroupListen.state.value) } }
        }
    }

    // ---- following the group ---------------------------------------------------------------------------------

    private fun later(block: () -> Unit) { main.post(block) }

    private fun follow(s: GroupState?) {
        if (s == null) {
            if (groupId != null) {
                groupId = null
                lastSpot = null
                main.removeCallbacks(tick)
                lag.cancel()
                // out of the group: the music stops here, the queue stays (play goes on alone)
                applying { player.pause() }
                if (speed != 1.0) setSpeed(1.0)
                onActive(false)
            }
            return
        }
        if (groupId != s.id) {
            groupId = s.id
            lastSpot = null
            onActive(true)
        }
        apply(s)
        schedule(500L)
    }

    private fun serverMadeAt(s: GroupState, ei: Int): Boolean = serverMade(s.queue[ei].track)

    /** The group's queue, song, position and play state on this player. */
    private fun apply(s: GroupState) {
        val (ei, epos) = GroupRules.expected(s, GroupListen.serverNow())
        if (ei < 0) {
            applying { if (player.playWhenReady) player.pause() }
            return
        }
        val posMs = Math.round(epos * 1000)
        // someone moved the group since the last state (a seek, play, pause, a jump): followed at once, not held back by
        // the pause after this phone's last seek
        val spot = Spot(s.index, s.positionSec, s.anchorMs, s.playing)
        if (lastSpot != null && lastSpot != spot) lag.release()
        lastSpot = spot
        // the group has not answered a change made here yet: only its queue is taken over
        if (holding(s)) {
            if (!sameQueue(s) && player.currentMediaItemIndex == ei) applying { setQueue(s, ei, posMs, want = false) }
            return
        }
        val want = s.playing && !GroupListen.localPaused.value
        applying {
            // the player was just put on the group's song and spot (no second seek below)
            var moved = true
            if (!sameQueue(s)) {
                moved = setQueue(s, ei, posMs, want)
            } else if (player.currentMediaItemIndex != ei) {
                if (want) seekGroup(s, ei, posMs) else toEntry(s, ei, posMs, playing = false)
                quietUntil = now() + 2500L
            } else {
                moved = false
            }
            val seekAbove = SyncRules.seekAboveMs(serverMadeAt(s, ei))
            if (want && !player.playWhenReady) {
                if (player.playbackState == Player.STATE_IDLE) player.prepare()
                // after a pause here (or the end of the queue here), back where the group is
                if (!moved && (player.playbackState == Player.STATE_ENDED || Math.abs(positionMs() - posMs) > seekAbove)) {
                    seekGroup(s, ei, posMs)
                }
                player.play()
                quietUntil = maxOf(quietUntil, now() + 1500L)
            } else if (!want && player.playWhenReady) {
                player.pause()
                lag.cancel()
                if (speed != 1.0) setSpeed(1.0)
            } else if (want && player.isPlaying && now() >= quietUntil && player.currentMediaItemIndex == ei) {
                // the host seeked: there at once (smaller drift is the tick's nudge). Only while playing: in a rebuffer
                // the position stands still and the next look decides
                val d = positionMs() - posMs
                if (lag.waiting) lag.landed(s.queue[ei].qid, d)
                if (Math.abs(d) > seekAbove && lag.sinceSeekMs(now()) >= SyncRules.HOLD_MS) {
                    if (speed != 1.0) setSpeed(1.0)
                    seekGroup(s, ei, posMs)
                }
            }
            // a paused group: everyone at the same spot
            if (!s.playing && player.currentMediaItemIndex == ei && Math.abs(positionMs() - posMs) > SyncRules.PAUSED_PLACE_MS) seekTo(posMs)
        }
    }

    /**
     * To where the playing group is in entry [ei] ([expMs]), ahead by what a seek of this kind of stream has been
     * taking to start playing (measured where the last ones landed). Within [applying].
     */
    private fun seekGroup(s: GroupState, ei: Int, expMs: Long) {
        val target = lag.target(s.queue[ei].qid, expMs, serverMadeAt(s, ei), now())
        // a server-made song started from its pre-buffered beginning instead: nothing to learn from where it lands
        if (!toEntry(s, ei, target)) lag.cancel()
        quietUntil = now() + 2500L
    }

    /**
     * The player on entry [ei] at [ms]: a whole file seeks there in the player (in another song, the next one's
     * pre-buffered data is used when it is that one); a server-made stream (no Range) is asked for again at `t=`
     * ([seekTo] in song time), as the listener's own seek does – or, another song with the spot within 3 s of its
     * start while the group plays, starts from what was pre-buffered of it and the nudge closes the rest (paused, a
     * nudge can't: the exact spot). False when it went to the song's start instead of [ms]. Within [applying].
     */
    private fun toEntry(s: GroupState, ei: Int, ms: Long, playing: Boolean = true): Boolean {
        if (player.currentMediaItemIndex == ei) {
            seekTo(ms)
            return true
        }
        val sm = serverMadeAt(s, ei)
        val at = if (playing) SyncRules.startAtMs(ms, sm) else ms
        if (at > 0L && sm) {
            player.seekTo(ei, 0L)
            seekTo(at)
        } else {
            player.seekTo(ei, at)
        }
        return at == ms
    }

    private fun sameQueue(s: GroupState): Boolean {
        if (player.mediaItemCount != s.queue.size) return false
        for (i in s.queue.indices) if (MediaItems.trackId(player.getMediaItemAt(i).mediaId) != s.queue[i].track.id) return false
        return true
    }

    /**
     * The group's queue on this player. While the song that plays stays the current one only what changed around it
     * is removed and added ([SyncRules.queueEdit]): no gap in the music, and a song added at the end leaves the next
     * one and what was pre-buffered of it alone. Else the whole queue, at the group's spot (ahead by the seek lag
     * when it plays, [want]). True when the player was put on the group's song and spot.
     */
    private fun setQueue(s: GroupState, ei: Int, posMs: Long, want: Boolean): Boolean {
        val tracks = s.queue.map { it.track }
        val n = player.mediaItemCount
        val cur = player.currentMediaItemIndex
        val curId = if (n > 0 && cur in 0 until n) MediaItems.trackId(player.getMediaItemAt(cur).mediaId) else null
        if (curId != null && curId == tracks[ei].id) {
            val local = (0 until n).map { MediaItems.trackId(player.getMediaItemAt(it).mediaId) ?: -1L }
            val e = SyncRules.queueEdit(local, cur, tracks.map { it.id }, ei)
            if (cur + 1 + e.keepAfter < n) player.removeMediaItems(cur + 1 + e.keepAfter, n)
            if (ei + 1 + e.keepAfter < tracks.size) player.addMediaItems(items(tracks.subList(ei + 1 + e.keepAfter, tracks.size)))
            if (cur - e.keepBefore > 0) player.removeMediaItems(0, cur - e.keepBefore)
            if (ei - e.keepBefore > 0) player.addMediaItems(0, items(tracks.subList(0, ei - e.keepBefore)))
            return false
        }
        newQueue()
        val list = items(tracks)
        val sm = serverMade(tracks[ei])
        val target = if (want && posMs > 0L) lag.target(s.queue[ei].qid, posMs, sm, now()) else posMs
        val at = if (want) SyncRules.startAtMs(target, sm) else target
        if (at != target) lag.cancel()
        if (at > 0L && sm) {
            // a server-made stream has no Range: the song is opened, then asked for again at t= (as a seek does)
            player.setMediaItems(list, ei, 0L)
            seekTo(at)
            if (player.playbackState == Player.STATE_IDLE) player.prepare()
        } else {
            player.setMediaItems(list, ei, at)
            player.prepare()
        }
        quietUntil = now() + 2500L
        return true
    }

    // ---- drift -----------------------------------------------------------------------------------------------

    private val tick = Runnable { check() }

    private fun schedule(ms: Long) {
        main.removeCallbacks(tick)
        main.postDelayed(tick, ms)
    }

    private fun check() {
        val s = GroupListen.state.value
        if (s == null || groupId != s.id) return
        schedule(500L)
        if (holding(s)) return
        if (!sameQueue(s)) {
            // a song change a moment before or after the group's is fine; anything else follows the group again
            apply(s)
            return
        }
        if (!s.playing || GroupListen.localPaused.value) {
            if (speed != 1.0) setSpeed(1.0)
            lag.cancel()
            if (player.playWhenReady && !GroupListen.localPaused.value) apply(s)
            return
        }
        if (!player.playWhenReady) { apply(s); return }
        if (now() < quietUntil || !player.isPlaying) return
        val (ei, epos) = GroupRules.expected(s, GroupListen.serverNow())
        val cur = player.currentMediaItemIndex
        val pos = positionMs()
        val expMs = Math.round(epos * 1000)
        if (cur != ei) {
            // gapless into the next song a little before (or after) the group: the player moves on by itself from what
            // it pre-buffered; past 3 s it goes to the group's song (the pre-buffered one when that is the next)
            if (!SyncRules.waitForSongChange(cur, pos, ei, expMs)) {
                if (speed != 1.0) setSpeed(1.0)
                applying { seekGroup(s, ei, expMs) }
            }
            return
        }
        val d = pos - expMs
        // the first look at a seek made here, playing again: what it took, for the next ones
        if (lag.waiting) lag.landed(s.queue[ei].qid, d)
        when (val act = SyncRules.decide(d, speed, serverMadeAt(s, ei), speedOk(), lag.sinceSeekMs(now()))) {
            SyncRules.Act.Seek -> {
                if (speed != 1.0) setSpeed(1.0)
                applying { seekGroup(s, ei, expMs) }
            }
            is SyncRules.Act.Speed -> setSpeed(act.rate)
            SyncRules.Act.None -> {}
        }
    }

    /** The playback speed (the audio track keeps the pitch); 1.0 ends a nudge. */
    private fun setSpeed(rate: Double) {
        speed = rate
        val f = rate.toFloat()
        if (player.playbackParameters.speed == f) return
        applying { player.setPlaybackSpeed(f) }
    }

    private fun now(): Long = android.os.SystemClock.elapsedRealtime()
}
