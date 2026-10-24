package app.encore.android.remote

import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.str
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.data.prefs.NormPrefs
import app.encore.android.features.group.GroupListen
import app.encore.android.playback.MediaItems
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * The playback service's side of the remote (web/js/player.js `snapshot()` / `command()`): what plays here, for
 * the check-ins, and the commands other devices send, run on the service's player so they work in the background
 * and from the lock screen. Lives as long as the service; everything runs on the main thread.
 *
 * The service hands in what only it knows: the position in song time (server-made streams start at an offset),
 * seeking in song time, the quality label of the stream, and how to turn songs into its media items.
 */
class ServiceHost(
    private val remote: RemoteDevice,
    /** the session's player (CarControls' CarPlayer): its shuffle is the app shuffle the car and the phone show */
    private val player: Player,
    /** the position in the song, ms (a server-made stream's offset included) */
    private val positionMs: () -> Long,
    /** the song's length in seconds (the library's for a server-made stream) */
    private val durationSec: () -> Double,
    /** seek in song time (server-made streams reopen at `t=`) */
    private val seekTo: (Long) -> Unit,
    /** the signal path as player.js sends it: label (the quality badge), transport kind, output rate (Hz) */
    private val signal: () -> Triple<String, String, Int>,
    /** the songs as the service's media items (it remembers them for the stream resolver) */
    private val items: (List<Track>) -> List<MediaItem>,
    /** a new queue replaces the old one: the service forgets what it knew about the previous streams */
    private val newQueue: () -> Unit,
    /** the volume to report: during a song transition the player's own level ramps, this is the level it ramps to */
    private val volume: () -> Float = { player.volume },
    /** a song transition is running: its volume steps are not news for the other devices */
    private val fading: () -> Boolean = { false },
    /** volume normalization on this device (Settings → Playback → Sound), reported as the snapshot's `norm` */
    private val norm: () -> NormPrefs = { NormPrefs() },
    /** the `prefs` command of another device (client-shapes §5.1): new normalization settings for this device */
    private val setNorm: (NormPrefs) -> Unit = {},
    /** the Signal path's normalization value for the song that plays ("" while normalization is off) */
    private val normLine: () -> String = { "" },
) : PlayerHost {

    private val listener = object : Player.Listener {
        override fun onEvents(player: Player, events: Player.Events) {
            // play / pause / another song / a seek / the queue / the volume: the other devices see it at once
            if (events.containsAny(
                    Player.EVENT_IS_PLAYING_CHANGED,
                    Player.EVENT_PLAY_WHEN_READY_CHANGED,
                    Player.EVENT_MEDIA_ITEM_TRANSITION,
                    Player.EVENT_POSITION_DISCONTINUITY,
                    Player.EVENT_TIMELINE_CHANGED,
                    Player.EVENT_SHUFFLE_MODE_ENABLED_CHANGED,
                    Player.EVENT_REPEAT_MODE_CHANGED,
                ) || (events.contains(Player.EVENT_VOLUME_CHANGED) && !fading())
            ) remote.beatSoon()
            if (events.contains(Player.EVENT_IS_PLAYING_CHANGED)) remote.setPlaying(player.isPlaying)
            if (events.contains(Player.EVENT_PLAY_WHEN_READY_CHANGED)) stayQuietWhileLinked()
        }

        // the app shuffle (CarPlayer.setShuffled) calls this directly, with no onEvents after it
        override fun onShuffleModeEnabledChanged(shuffleModeEnabled: Boolean) {
            remote.beatSoon()
        }
    }

    /**
     * While this phone is a remote for another device it plays nothing itself: a play that reaches the service
     * anyway (the notification, the lock screen, a headset or car button, Android Auto) is paused again at once
     * and sent to the linked device as its play. Without this the song went on here as well as there.
     * Commands from other devices that start the music here end the link first (RemoteDevice.runNow), so they
     * are not caught here.
     */
    private fun stayQuietWhileLinked() {
        if (!player.playWhenReady || remote.link.value == null) return
        player.pause()
        remote.sendToLink(buildJsonObject { put("action", "play") })
    }

    fun start() {
        player.addListener(listener)
        remote.attach(this)
        remote.setPlaying(player.isPlaying)
    }

    fun stop() {
        player.removeListener(listener)
        remote.detach(this)
    }

    /** `!audio.paused` on the web: playing, or about to (buffering with play pressed). */
    private fun playingNow(): Boolean =
        player.playWhenReady && player.playbackState != Player.STATE_ENDED && player.playbackState != Player.STATE_IDLE

    /** `{mode, target, clip}`: the normalization settings as the snapshot reports them, also while nothing plays. */
    private fun normJson(): JsonObject {
        val n = norm()
        return buildJsonObject {
            put("mode", n.mode)
            put("target", n.target)
            put("clip", n.clip)
        }
    }

    /** Nothing plays here: the idle state, with this device's normalization settings. */
    private fun idle(): JsonObject = JsonObject(RemoteDevice.IDLE + ("norm" to normJson()))

    override fun snapshot(): JsonObject {
        val item = player.currentMediaItem ?: return idle()
        val t = MediaItems.toTrack(item) ?: return idle()
        val all = ArrayList<Track>(player.mediaItemCount)
        var index = 0
        for (i in 0 until player.mediaItemCount) {
            val q = MediaItems.toTrack(player.getMediaItemAt(i)) ?: continue
            if (i == player.currentMediaItemIndex) index = all.size
            all.add(q)
        }
        val (queue, at) = RemoteDevice.window(all, index)
        val (label, transport, rate) = signal()
        val dur = durationSec().takeIf { it > 0 } ?: t.durationSec
        return buildJsonObject {
            put("id", t.id)
            put("title", t.title)
            put("artist", t.artist)
            if (t.artists.isNotEmpty()) put("artists", buildJsonArray {
                for (a in t.artists) add(buildJsonObject {
                    put("name", a.name)
                    if (a.mbid.isNotEmpty()) put("mbid", a.mbid)
                    put("in_library", a.inLibrary)
                })
            })
            put("album", t.album)
            put("album_id", t.albumId)
            if (t.format.isNotEmpty()) put("format", t.format)
            put("duration", dur)
            if (t.albumId.isNotEmpty()) put("cover", "/api/library/albums/${EncoreApi.enc(t.albumId)}/cover")
            put("playing", playingNow())
            put("position", positionMs().coerceAtLeast(0L) / 1000.0)
            put("volume", volume().toDouble())
            put("shuffle", player.shuffleModeEnabled)
            put("repeat", repeatName(player.repeatMode))
            put("index", at)
            put("queue", RemoteDevice.slimList(queue))
            put("signal", buildJsonObject {
                put("label", label)
                if (transport.isNotEmpty()) put("transport", transport) else put("transport", null as String?)
                if (rate > 0) put("output_rate", rate) else put("output_rate", null as Int?)
                normLine().takeIf { it.isNotEmpty() }?.let { put("norm", it) }
            })
            put("norm", normJson())
        }
    }

    override fun pauseLocal() {
        player.pause()
    }

    private fun play() {
        if (player.mediaItemCount == 0) return
        if (player.playbackState == Player.STATE_IDLE) player.prepare()
        if (player.playbackState == Player.STATE_ENDED) player.seekTo(player.currentMediaItemIndex, 0L)
        player.play()
    }

    override fun run(c: JsonObject) {
        // in a group listen the queue is the group's: songs sent from another device go to the group (or are refused)
        if (GroupListen.inGroup) {
            when (c.str("action")) {
                "play_queue" -> { GroupListen.playList(RemoteDevice.tracksOf(c["tracks"]), c.dbl("index").toInt(), c.dbl("position").coerceAtLeast(0.0)); return }
                "enqueue" -> { GroupListen.enqueue(RemoteDevice.tracksOf(c["tracks"])); return }
                "play_next" -> { GroupListen.playNext(RemoteDevice.tracksOf(c["tracks"])); return }
                "shuffle", "repeat" -> return
            }
        }
        when (c.str("action")) {
            "toggle" -> if (player.isPlaying || player.playWhenReady && player.playbackState == Player.STATE_BUFFERING) player.pause() else play()
            "play" -> play()
            "pause", "stop" -> player.pause()
            "next" -> if (player.hasNextMediaItem()) player.seekToNextMediaItem()
            // like the web player: back to the start after 3 s, otherwise the previous song
            "prev" -> if (positionMs() > 3000L || !player.hasPreviousMediaItem()) seekTo(0L) else player.seekToPreviousMediaItem()
            "seek" -> seekTo(Math.round(c.dbl("value").coerceAtLeast(0.0) * 1000))
            "volume" -> {
                val v = c.dbl("value")
                player.volume = (if (v.isNaN()) 0.9 else v).toFloat().coerceIn(0f, 1f)
            }
            // the app shuffle (the player is the session's, so this is the shuffle the car and the phone show)
            "shuffle" -> player.shuffleModeEnabled = c.bool("value")
            "repeat" -> when (c.str("value")) {
                "off" -> player.repeatMode = Player.REPEAT_MODE_OFF
                "all" -> player.repeatMode = Player.REPEAT_MODE_ALL
                "one" -> player.repeatMode = Player.REPEAT_MODE_ONE
                else -> {}
            }
            "enqueue" -> {
                val list = items(RemoteDevice.tracksOf(c["tracks"]))
                if (list.isEmpty()) return
                val was = player.mediaItemCount
                player.addMediaItems(list)
                if (was == 0) { player.prepare(); player.play() }
            }
            "play_next" -> {
                val list = items(RemoteDevice.tracksOf(c["tracks"]))
                if (list.isEmpty()) return
                if (player.mediaItemCount == 0) {
                    newQueue()
                    player.setMediaItems(list, 0, 0L)
                    player.prepare()
                    player.play()
                } else {
                    player.addMediaItems(player.currentMediaItemIndex + 1, list)
                }
            }
            // new normalization settings from the device controlling this one; a field left out keeps its value
            "prefs" -> {
                val n = c["norm"] as? JsonObject ?: return
                val was = norm()
                val target = (n["target"] as? JsonPrimitive)?.content?.trim()?.toDoubleOrNull()?.takeIf { it.isFinite() }
                val clip = (n["clip"] as? JsonPrimitive)?.takeIf { !it.isString }?.content?.toBooleanStrictOrNull()
                setNorm(
                    NormPrefs(
                        mode = n.str("mode").trim().ifEmpty { was.mode },
                        target = target?.let { Math.round(it).toInt() } ?: was.target,
                        clip = clip ?: was.clip,
                    ).clean(),
                )
                remote.beatSoon()
            }
            "play_queue" -> {
                val tracks = RemoteDevice.tracksOf(c["tracks"])
                if (tracks.isEmpty()) return
                val index = c.dbl("index").toInt().coerceIn(0, tracks.size - 1)
                val at = c.dbl("position")
                newQueue()
                val list = items(tracks)
                player.setMediaItems(list, index, if (at > 1) Math.round(at * 1000) else 0L)
                player.prepare()
                player.play()
            }
            else -> {}
        }
    }

    companion object {
        /** Media3's repeat mode as the remote says it: "off" | "all" | "one". */
        fun repeatName(mode: Int): String = when (mode) {
            Player.REPEAT_MODE_ALL -> "all"
            Player.REPEAT_MODE_ONE -> "one"
            else -> "off"
        }
    }
}
