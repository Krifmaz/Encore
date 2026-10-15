package app.encore.android.playback

import android.net.Uri
import android.os.Bundle
import app.encore.android.BuildConfig
import app.encore.android.data.api.ArtistCredit
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata

/**
 * Media items as the session, Android Auto and the phone UI share them.
 *
 * Media ids (kept from the old CarService): `track:{id}` or `track:{id}|{parent}` for a song listed under a
 * browse node (playing it plays the whole list from there); folders use their browse id (`album:{id}`, …).
 */
object MediaItems {
    const val EXTRA_TRACK_ID = "track_id"
    const val EXTRA_ALBUM_ID = "album_id"
    const val EXTRA_FORMAT = "format"
    const val EXTRA_DURATION = "duration"
    const val EXTRA_ALBUM_ARTIST = "albumartist"
    const val EXTRA_BITRATE = "bitrate"
    const val EXTRA_YEAR = "year"
    /** each artist of the song (names, MusicBrainz ids, in the library), for links in the player */
    const val EXTRA_ARTIST_NAMES = "artist_names"
    const val EXTRA_ARTIST_MBIDS = "artist_mbids"
    const val EXTRA_ARTIST_IN_LIBRARY = "artist_in_library"

    fun trackId(mediaId: String): Long? =
        if (mediaId.startsWith("track:")) mediaId.removePrefix("track:").substringBefore("|").toLongOrNull() else null

    fun parentOf(mediaId: String): String? =
        if (mediaId.startsWith("track:") && mediaId.contains("|")) mediaId.substringAfter("|") else null

    /** content:// cover for the session, notification and Android Auto (served by [CoverProvider]). */
    fun coverUri(albumId: String, size: Int = 600): Uri =
        Uri.parse("content://${BuildConfig.APPLICATION_ID}.covers/album/${Uri.encode(albumId)}?s=$size")

    fun track(api: EncoreApi, t: Track, parent: String? = null): MediaItem {
        val extras = Bundle().apply {
            putLong(EXTRA_TRACK_ID, t.id)
            putString(EXTRA_ALBUM_ID, t.albumId)
            putString(EXTRA_FORMAT, t.format)
            putDouble(EXTRA_DURATION, t.durationSec)
            putString(EXTRA_ALBUM_ARTIST, t.albumArtist)
            putInt(EXTRA_BITRATE, t.bitrate)
            putString(EXTRA_YEAR, t.year)
            if (t.artists.isNotEmpty()) {
                putStringArrayList(EXTRA_ARTIST_NAMES, ArrayList(t.artists.map { it.name }))
                putStringArrayList(EXTRA_ARTIST_MBIDS, ArrayList(t.artists.map { it.mbid }))
                putBooleanArray(EXTRA_ARTIST_IN_LIBRARY, t.artists.map { it.inLibrary }.toBooleanArray())
            }
        }
        val md = MediaMetadata.Builder()
            .setTitle(t.title)
            .setArtist(t.artist)
            .setAlbumTitle(t.album)
            .setAlbumArtist(t.albumArtist)
            .setTrackNumber(if (t.trackNo > 0) t.trackNo else null)
            .setDiscNumber(if (t.disc > 0) t.disc else null)
            .setDurationMs(if (t.durationSec > 0) (t.durationSec * 1000).toLong() else null)
            .setIsBrowsable(false)
            .setIsPlayable(true)
            .setMediaType(MediaMetadata.MEDIA_TYPE_MUSIC)
            .setExtras(extras)
        if (t.albumId.isNotEmpty()) md.setArtworkUri(coverUri(t.albumId))
        return MediaItem.Builder()
            .setMediaId("track:${t.id}" + (if (parent != null) "|$parent" else ""))
            // the bare stream address: StreamResolver adds the quality / transcode / Cadenza query when it loads
            .setUri(api.streamUrl(t.id, ""))
            .setMediaMetadata(md.build())
            .build()
    }

    /**
     * An item that arrived from a controller without its URI (MediaItem bundles drop the local configuration):
     * rebuilt from its id and metadata extras, without asking the server. Null if it isn't a song we know.
     */
    fun withUri(api: EncoreApi, item: MediaItem): MediaItem? {
        if (item.localConfiguration != null) return item
        val id = trackId(item.mediaId) ?: return null
        if (item.mediaMetadata.extras?.getString(EXTRA_FORMAT) == null) return null
        return item.buildUpon().setUri(api.streamUrl(id, "")).build()
    }

    /** The song as a Track again (queue snapshots, resumption). */
    fun toTrack(item: MediaItem): Track? {
        val id = trackId(item.mediaId) ?: return null
        val m = item.mediaMetadata
        val x = m.extras
        return Track(
            id = id,
            title = m.title?.toString() ?: "",
            artist = m.artist?.toString() ?: "",
            album = m.albumTitle?.toString() ?: "",
            albumArtist = x?.getString(EXTRA_ALBUM_ARTIST) ?: (m.albumArtist?.toString() ?: ""),
            albumId = x?.getString(EXTRA_ALBUM_ID) ?: "",
            trackNo = m.trackNumber ?: 0,
            disc = m.discNumber ?: 0,
            year = x?.getString(EXTRA_YEAR) ?: "",
            durationSec = x?.getDouble(EXTRA_DURATION) ?: 0.0,
            format = x?.getString(EXTRA_FORMAT) ?: "",
            bitrate = x?.getInt(EXTRA_BITRATE) ?: 0,
            artists = artistsOf(x),
        )
    }

    private fun artistsOf(x: Bundle?): List<ArtistCredit> {
        val names = x?.getStringArrayList(EXTRA_ARTIST_NAMES) ?: return emptyList()
        val mbids = x.getStringArrayList(EXTRA_ARTIST_MBIDS)
        val inLib = x.getBooleanArray(EXTRA_ARTIST_IN_LIBRARY)
        return names.mapIndexed { i, n -> ArtistCredit(n, mbids?.getOrNull(i) ?: "", inLib?.getOrNull(i) ?: false) }
    }
}
