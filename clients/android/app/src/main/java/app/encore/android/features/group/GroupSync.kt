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
 * Sync rule (the same on web, desktop and phone): expected position = position + (server_now − anchor_ms) / 1000
 * while playing. Drift above 0.5 s → seek; between 0.15 and 0.5 s → playback speed 0.97 / 1.03 (Media3, the audio
 * track keeps the pitch) until within 0.05 s; with bit-perfect USB or Cadenza on this phone the speed stays at 1.0
 * and only the seek applies. Crossfade is off in a group (the service asks [active]); gapless stays.
 *
 * A seek made here aims ahead by what seeks have been taking to start playing ([SeekLag], learnt per kind of stream),
 * so a slow seek (a server-made stream asked for again at `t=` on mobile data) does not land behind the group and seek
 * again every few seconds; where seeks keep missing, that song is left to the speed nudge below 3 s.
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
    private var groupId: String? = null
    private var adjusting = false
    /** no drift check until then (elapsed ms): a seek needs a moment to buffer */
    private var quietUntil = 0L
    /**
     * A change made here went to the group (play, pause, a jump, a seek): until the group's answer (a state newer than
     * [holdRev]) or [holdUntil], the player is not pulled back to the older state.
     */
    private var holdUntil = 0L
    private var holdRev = -1L
    private var lastNudge = 0
    /** what this phone's seeks take to start playing, and the songs where they stopped */
    private val lag = SeekLag()

    /** In a group now: the service turns crossfade and the library shuffle off. */
    val active: Boolean get() = groupId != null

    private val listener = object : Player.Listener {
        override fun onMediaItemTransition(mediaItem: MediaItem?, reason: Int) {
            if (applying > 0 || groupId == null) return
            if (reason == Player.MEDIA_ITEM_TRANSITION_REASON_SEEK) userJump(player.currentMediaItemIndex)
        }

        override fun onPositionDiscontinuity(oldPosition: Player.PositionInfo, newPosition: Player.PositionInfo, reason: Int) {
            if (applying > 0 || groupId == null) return
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
        if (adjusting) setSpeed(1f)
    }

    /** A seek asked in song time (the app's seek bar through the service's custom command): the group's when allowed. */
    fun userSeek(ms: Long) {
        val s = GroupListen.state.value ?: return
        if (!s.youCanControl) { GroupListen.deniedHere(); later { follow(GroupListen.state.value) }; return }
        // close to where the group is anyway (a quality switch reopening the stream): nothing to tell
        val (ei, epos) = GroupRules.expected(s, GroupListen.serverNow())
        if (player.currentMediaItemIndex == ei && Math.abs(ms - Math.round(epos * 1000)) < 1500L) return
        hold(s)
        GroupListen.command("seek") { put("position", ms / 1000.0) }
    }

    /** The group was told about a change made here: its answer is awaited before following it again. */
    private fun hold(s: GroupState) {
        holdRev = s.rev
        holdUntil = now() + 3000L
        quietUntil = now() + 2500L
        // the listener's own seek or jump did not aim ahead: nothing to learn from where it lands
        lag.cancel()
    }

    private fun holding(s: GroupState): Boolean = now() < holdUntil && s.rev <= holdRev

    /** Runs [block] as the group's own change: the listener leaves its events alone. */
    fun <T> applying(block: () -> T): T {
        applying++
        try { return block() } finally { applying-- }
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
                main.removeCallbacks(tick)
                lag.cancel()
                // out of the group: the music stops here, the queue stays (play goes on alone)
                applying { player.pause() }
                if (adjusting) setSpeed(1f)
                onActive(false)
            }
            return
        }
        if (groupId != s.id) {
            groupId = s.id
            onActive(true)
        }
        apply(s)
        schedule(500L)
    }

    /** The group's queue, song, position and play state on this player. */
    private fun apply(s: GroupState) {
        val (ei, epos) = GroupRules.expected(s, GroupListen.serverNow())
        if (ei < 0) {
            applying { if (player.playWhenReady) player.pause() }
            return
        }
        val posMs = Math.round(epos * 1000)
        // the group has not answered a change made here yet: only its queue is taken over
        if (holding(s)) {
            if (!sameQueue(s) && player.currentMediaItemIndex == ei) applying { setQueue(s, ei, posMs) }
            return
        }
        val want = s.playing && !GroupListen.localPaused.value
        applying {
            // the player was just put on the group's song and spot (no second seek below)
            var moved = true
            if (!sameQueue(s)) {
                moved = setQueue(s, ei, posMs)
            } else if (player.currentMediaItemIndex != ei) {
                if (want) seekGroup(s, ei, posMs) else toEntry(s, ei, posMs)
                quietUntil = now() + 2500L
            } else {
                moved = false
            }
            if (want && !player.playWhenReady) {
                if (player.playbackState == Player.STATE_IDLE) player.prepare()
                // after a pause here (or the end of the queue here), back where the group is
                if (!moved && (player.playbackState == Player.STATE_ENDED || Math.abs(positionMs() - posMs) > SeekLag.SEEK_MS)) {
                    seekGroup(s, ei, posMs)
                }
                player.play()
                quietUntil = maxOf(quietUntil, now() + 1500L)
            } else if (!want && player.playWhenReady) {
                player.pause()
                lag.cancel()
            } else if (want && player.isPlaying && now() >= quietUntil && player.currentMediaItemIndex == ei) {
                // the host seeked: there at once (smaller drift is the tick's). Only while playing: in a rebuffer the
                // position stands still and the next look decides
                val song = s.queue[ei].qid
                val d = positionMs() - posMs
                if (lag.waiting) lag.landed(song, d, now())
                if (Math.abs(d) > lag.seekAboveMs(song)) {
                    if (adjusting) setSpeed(1f)
                    seekGroup(s, ei, posMs)
                }
            }
            // a paused group: everyone at the same spot
            if (!s.playing && player.currentMediaItemIndex == ei && Math.abs(positionMs() - posMs) > SeekLag.SEEK_MS) seekTo(posMs)
        }
    }

    /**
     * To where the playing group is in entry [ei] ([expMs]), ahead by what a seek of this kind of stream has been
     * taking to start playing (learnt from where the last one landed, at the next look). Within [applying].
     */
    private fun seekGroup(s: GroupState, ei: Int, expMs: Long) {
        val target = lag.target(s.queue[ei].qid, expMs, serverMade(s.queue[ei].track))
        toEntry(s, ei, target)
        quietUntil = now() + 2500L
    }

    /**
     * The player on entry [ei] at [ms]: a whole file seeks there in the player; a server-made stream (no Range) is
     * opened on its song and asked for again at `t=` ([seekTo] in song time), as the listener's own seek does, so it
     * does not start at 0 and need a second reopen. Within [applying].
     */
    private fun toEntry(s: GroupState, ei: Int, ms: Long) {
        when {
            player.currentMediaItemIndex == ei -> seekTo(ms)
            ms > 0L && serverMade(s.queue[ei].track) -> {
                player.seekTo(ei, 0L)
                seekTo(ms)
            }
            else -> player.seekTo(ei, ms)
        }
    }

    private fun sameQueue(s: GroupState): Boolean {
        if (player.mediaItemCount != s.queue.size) return false
        for (i in s.queue.indices) if (MediaItems.trackId(player.getMediaItemAt(i).mediaId) != s.queue[i].track.id) return false
        return true
    }

    /** The group's queue on this player; the song that plays keeps playing when it stays the current one. */
    private fun setQueue(s: GroupState, ei: Int, posMs: Long): Boolean {
        val tracks = s.queue.map { it.track }
        val cur = player.currentMediaItemIndex
        val curId = if (player.mediaItemCount > 0 && cur in 0 until player.mediaItemCount) MediaItems.trackId(player.getMediaItemAt(cur).mediaId) else null
        if (curId != null && curId == tracks[ei].id) {
            // only what is around the song changed (a song added, removed, moved): no gap in the music
            if (cur + 1 < player.mediaItemCount) player.removeMediaItems(cur + 1, player.mediaItemCount)
            if (cur > 0) player.removeMediaItems(0, cur)
            if (ei > 0) player.addMediaItems(0, items(tracks.subList(0, ei)))
            if (ei + 1 < tracks.size) player.addMediaItems(items(tracks.subList(ei + 1, tracks.size)))
            return false
        }
        newQueue()
        val list = items(tracks)
        if (posMs > 0L && serverMade(tracks[ei])) {
            // a server-made stream has no Range: the song is opened, then asked for again at t= (as a seek does)
            player.setMediaItems(list, ei, 0L)
            seekTo(posMs)
            if (player.playbackState == Player.STATE_IDLE) player.prepare()
        } else {
            player.setMediaItems(list, ei, posMs)
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
            if (adjusting) setSpeed(1f)
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
            // gapless into the next song a little before (or after) the group: wait for the song change to settle
            val nearStart = (cur == ei + 1 && pos < 1500L) || (cur + 1 == ei && expMs < 1500L)
            if (!nearStart) applying { seekGroup(s, ei, expMs) }
            return
        }
        val song = s.queue[ei].qid
        // the first look at a seek made here, playing again: what it took, for the next one
        if (lag.waiting) lag.landed(song, pos - expMs, now())
        when (GroupRules.drift(pos - expMs, adjusting, speedOk(), lag.seekAboveMs(song))) {
            GroupRules.Drift.SEEK -> {
                if (adjusting) setSpeed(1f)
                applying { seekGroup(s, ei, expMs) }
            }
            GroupRules.Drift.FASTER -> setSpeed(1.03f)
            GroupRules.Drift.SLOWER -> setSpeed(0.97f)
            GroupRules.Drift.NORMAL -> setSpeed(1f)
            GroupRules.Drift.NONE -> {}
        }
    }

    private fun setSpeed(speed: Float) {
        adjusting = speed != 1f
        if (player.playbackParameters.speed == speed) return
        applying { player.setPlaybackSpeed(speed) }
    }

    private fun now(): Long = android.os.SystemClock.elapsedRealtime()
}
