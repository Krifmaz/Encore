package app.encore.android.playback

import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer

/**
 * The song the service's player is on, as a stream: which variant it plays ([StreamResolver]), where it is in song
 * time (a server-made stream starts at an offset, its clock at 0), and how to open it again at a spot on another
 * variant (a quality switch, a seek in a server-made stream, server processing failing, a new output path).
 * Shared by the parts of the playback service; main thread only.
 */
class CurrentStream(private val player: ExoPlayer, private val resolver: StreamResolver) {
    /** The song the player is on (null: nothing, or not a library song). */
    var id: Long? = null

    /** A reopen of the same song is on its way: its media item transition starts nothing anew (no play, no now-playing). */
    var switchingTo: Long? = null

    /** After the item was replaced by a reopen (the service notes a seek and publishes the new variant). */
    var onReplaced: () -> Unit = {}

    /** The song as the resolver knows it (or as its media item says). */
    fun track(): StreamTrack? {
        val item = player.currentMediaItem ?: return null
        val id = MediaItems.trackId(item.mediaId) ?: return null
        return resolver.track(id) ?: MediaItems.toTrack(item)?.let { StreamTrack.of(it) }
    }

    fun variant(): StreamVariant? = id?.let { resolver.variantOf(it) }

    /** Where a server-made stream started (a whole file is seeked in the player, so its offset is 0). */
    fun offsetMs(): Long {
        val v = variant() ?: return 0L
        return if (v.serverStream) Math.round(v.offsetSec * 1000) else 0L
    }

    /** The position in the song, including where a server-made stream started. */
    fun absPositionMs(): Long = player.currentPosition.coerceAtLeast(0L) + offsetMs()

    /** The song's length in seconds: the library's for a server-made stream (it reports what it has made so far). */
    fun durationSec(): Double {
        val item = player.currentMediaItem ?: return 0.0
        val v = variant()
        val known = item.mediaMetadata.extras?.getDouble(MediaItems.EXTRA_DURATION) ?: 0.0
        if (v != null && v.serverStream) return known
        return if (player.duration > 0) player.duration / 1000.0 else known
    }

    /**
     * onMediaItemTransition: is `id` the same song again after a reopen? Then it is the current one once more and
     * nothing starts anew. Clears the pending reopen either way.
     */
    fun arrivedAgain(id: Long?): Boolean {
        val same = id != null && id == switchingTo
        switchingTo = null
        if (same) this.id = id
        return same
    }

    /**
     * The same song again at `offsetSec`: on `rung` (null = the rung it has), with or without the server's Cadenza.
     * A server-made stream starts at `t=offset` and the player at 0; a whole file is seeked in the player.
     */
    fun reopen(t: StreamTrack, rung: Int?, dsp: Boolean?, offsetSec: Double, prepareFirst: Boolean = false) {
        val v = resolver.reopenVariant(t, rung, dsp, offsetSec)
        resolver.force(t.id, v)
        switchingTo = t.id
        replace(prepare = prepareFirst)
    }

    /** The current item again with the variant the resolver now holds for it. */
    fun replace(prepare: Boolean) {
        val index = player.currentMediaItemIndex
        val item = player.currentMediaItem ?: return
        val id = MediaItems.trackId(item.mediaId) ?: return
        val v = resolver.variantOf(id) ?: return
        val wasPlaying = player.playWhenReady
        // a different URI, or ExoPlayer keeps the running stream (replaceMediaItem only updates metadata for an
        // unchanged one); the resolver rewrites it anyway, so the nonce never reaches the server
        val base = resolver.url(id, v)
        val fresh = item.buildUpon().setUri(base + (if (base.contains('?')) "&" else "?") + "r=" + System.nanoTime()).build()
        player.replaceMediaItem(index, fresh)
        if (!v.serverStream && v.offsetSec > 0) player.seekTo(index, Math.round(v.offsetSec * 1000))
        if (prepare || player.playbackState == Player.STATE_IDLE) player.prepare()
        player.playWhenReady = wasPlaying
        onReplaced()
    }
}
