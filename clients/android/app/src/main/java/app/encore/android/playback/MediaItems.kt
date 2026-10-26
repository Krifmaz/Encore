package app.encore.android.playback

import android.net.Uri
import android.os.Bundle
import app.encore.android.BuildConfig
import app.encore.android.data.api.ArtistCredit
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Track
import app.encore.android.data.api.TrackGain
import app.encore.android.data.api.Tr
import app.encore.android.data.api.Translations
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
    /** the file's sample rate (Hz) and bit depth, for the car's quality badge (0.31; absent when unknown) */
    const val EXTRA_SAMPLE_RATE = "sample_rate"
    const val EXTRA_BIT_DEPTH = "bit_depth"
    /** each artist of the song (names, MusicBrainz ids, in the library), for links in the player */
    const val EXTRA_ARTIST_NAMES = "artist_names"
    const val EXTRA_ARTIST_MBIDS = "artist_mbids"
    const val EXTRA_ARTIST_IN_LIBRARY = "artist_in_library"
    /** the song's featured artists (Track.featured): their names and MusicBrainz ids, in order */
    const val EXTRA_FEATURED_NAMES = "featured_names"
    const val EXTRA_FEATURED_MBIDS = "featured_mbids"
    /** the original title / artist / album and their translations (the metadata shows them as Settings → Language asks) */
    const val EXTRA_TITLE = "orig_title"
    const val EXTRA_ARTIST = "orig_artist"
    const val EXTRA_ALBUM = "orig_album"
    const val EXTRA_TR = "tr"
    /** the song's loudness for volume normalization: [track, album, track_peak, album_peak] in dB / linear, NaN = unknown */
    const val EXTRA_GAIN = "gain"

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
            if (t.sampleRate > 0) putInt(EXTRA_SAMPLE_RATE, t.sampleRate)
            if (t.bitDepth > 0) putInt(EXTRA_BIT_DEPTH, t.bitDepth)
            t.gain?.let { putDoubleArray(EXTRA_GAIN, it.toArray()) }
            putString(EXTRA_TITLE, t.title)
            putString(EXTRA_ARTIST, t.artist)
            putString(EXTRA_ALBUM, t.album)
            if (!t.titleTr.isEmpty || !t.artistTr.isEmpty || !t.albumTr.isEmpty) {
                putStringArrayList(EXTRA_TR, arrayListOf(t.titleTr.t, t.titleTr.r, t.artistTr.t, t.artistTr.r, t.albumTr.t, t.albumTr.r))
            }
            if (t.artists.isNotEmpty()) {
                putStringArrayList(EXTRA_ARTIST_NAMES, ArrayList(t.artists.map { it.name }))
                putStringArrayList(EXTRA_ARTIST_MBIDS, ArrayList(t.artists.map { it.mbid }))
                putBooleanArray(EXTRA_ARTIST_IN_LIBRARY, t.artists.map { it.inLibrary }.toBooleanArray())
            }
            if (t.featured.isNotEmpty()) {
                putStringArrayList(EXTRA_FEATURED_NAMES, ArrayList(t.featured.map { it.name }))
                putStringArrayList(EXTRA_FEATURED_MBIDS, ArrayList(t.featured.map { it.mbid }))
            }
        }
        // the notification, the lock screen and the car show the titles the listener asked for
        val md = MediaMetadata.Builder()
            .setTitle(Translations.inline(t.title, t.titleTr))
            .setArtist(t.shownArtist())
            .setAlbumTitle(t.shownAlbum())
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
        val tr = x?.getStringArrayList(EXTRA_TR)?.takeIf { it.size == 6 }
        return Track(
            id = id,
            title = x?.getString(EXTRA_TITLE) ?: (m.title?.toString() ?: ""),
            artist = x?.getString(EXTRA_ARTIST) ?: (m.artist?.toString() ?: ""),
            album = x?.getString(EXTRA_ALBUM) ?: (m.albumTitle?.toString() ?: ""),
            albumArtist = x?.getString(EXTRA_ALBUM_ARTIST) ?: (m.albumArtist?.toString() ?: ""),
            albumId = x?.getString(EXTRA_ALBUM_ID) ?: "",
            trackNo = m.trackNumber ?: 0,
            disc = m.discNumber ?: 0,
            year = x?.getString(EXTRA_YEAR) ?: "",
            durationSec = x?.getDouble(EXTRA_DURATION) ?: 0.0,
            format = x?.getString(EXTRA_FORMAT) ?: "",
            bitrate = x?.getInt(EXTRA_BITRATE) ?: 0,
            artists = artistsOf(x),
            titleTr = if (tr != null) Tr(tr[0], tr[1]) else Tr.NONE,
            artistTr = if (tr != null) Tr(tr[2], tr[3]) else Tr.NONE,
            albumTr = if (tr != null) Tr(tr[4], tr[5]) else Tr.NONE,
            gain = TrackGain.fromArray(x?.getDoubleArray(EXTRA_GAIN)),
            sampleRate = x?.getInt(EXTRA_SAMPLE_RATE) ?: 0,
            bitDepth = x?.getInt(EXTRA_BIT_DEPTH) ?: 0,
            featured = featuredOf(x),
        )
    }

    /** The featured artists saved with an item (see [EXTRA_FEATURED_NAMES]); empty when it has none. */
    private fun featuredOf(x: Bundle?): List<ArtistCredit> {
        val names = x?.getStringArrayList(EXTRA_FEATURED_NAMES) ?: return emptyList()
        val mbids = x.getStringArrayList(EXTRA_FEATURED_MBIDS)
        return names.mapIndexed { i, n -> ArtistCredit(n, mbids?.getOrNull(i) ?: "", true) }
    }

    private fun artistsOf(x: Bundle?): List<ArtistCredit> {
        val names = x?.getStringArrayList(EXTRA_ARTIST_NAMES) ?: return emptyList()
        val mbids = x.getStringArrayList(EXTRA_ARTIST_MBIDS)
        val inLib = x.getBooleanArray(EXTRA_ARTIST_IN_LIBRARY)
        return names.mapIndexed { i, n -> ArtistCredit(n, mbids?.getOrNull(i) ?: "", inLib?.getOrNull(i) ?: false) }
    }
}
