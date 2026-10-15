package app.encore.android.ui.screens

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withLink
import app.encore.android.data.api.ArtistCredit
import app.encore.android.data.api.Track
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CoroutineScope

/**
 * Tappable names on a song (web/js/ui.js `artistLinks` / `albumLink`): each artist of the song is its own link
 * ("Travis Scott feat. The Weeknd" → two; "A & B" when the library splits it), the words between them stay as
 * they are, and the album name opens the album.
 */
fun songLinks(
    track: Track,
    linkColor: Color,
    onArtist: (ArtistCredit) -> Unit,
    onAlbum: (() -> Unit)? = null,
    withAlbum: Boolean = false,
    suffix: String = "",
): AnnotatedString = buildAnnotatedString {
    val style = TextLinkStyles(SpanStyle(color = linkColor))
    val s = track.artist
    var at = 0
    // without the server's split (an older server, a saved queue) the whole text is one name, looked up like
    // the web's `/artist-name/` – it may be "A feat. B", which is no library artist
    val list = track.artists.ifEmpty { if (s.isNotEmpty()) listOf(ArtistCredit(s, inLibrary = false)) else emptyList() }
    for (a in list) {
        if (a.name.isEmpty()) continue
        val i = s.indexOf(a.name, at)
        if (i < 0) continue
        append(s.substring(at, i))
        withLink(LinkAnnotation.Clickable("artist:${a.name}", style) { onArtist(a) }) { append(a.name) }
        at = i + a.name.length
    }
    if (at == 0) {
        if (s.isNotEmpty()) withLink(LinkAnnotation.Clickable("artist:$s", style) { onArtist(ArtistCredit(s, inLibrary = false)) }) { append(s) }
    } else {
        append(s.substring(at))
    }
    if (withAlbum && track.album.isNotEmpty()) {
        if (length > 0) append(" · ")
        if (onAlbum != null && track.albumId.isNotEmpty()) {
            withLink(LinkAnnotation.Clickable("album:${track.albumId}", style) { onAlbum() }) { append(track.album) }
        } else {
            append(track.album)
        }
    }
    if (suffix.isNotEmpty()) append(suffix)
}

/**
 * Where an artist name goes: the library artist page when it is a library artist, else its MusicBrainz page
 * (looked up by name when the id isn't known; the library page when MusicBrainz doesn't know it either).
 */
fun openArtistCredit(nav: Nav, a: ArtistCredit, scope: CoroutineScope) {
    if (a.name.isEmpty() && a.mbid.isEmpty()) return
    if (a.inLibrary && a.name.isNotEmpty()) {
        nav.open(Screen.Artist(a.name))
        return
    }
    openCatalogArtist(nav, a.mbid, a.name, scope)
}

/** [songLinks] for a song row: artist links (and album) that open their pages; [before] runs first (closing a sheet). */
@Composable
fun rememberSongLinks(nav: Nav, track: Track, withAlbum: Boolean = true, suffix: String = "", before: () -> Unit = {}): AnnotatedString {
    // the app's scope: the lookup goes on after the player or sheet that showed the link has closed
    val scope = nav.graph.scope
    val color = MaterialTheme.palette.text
    return songLinks(
        track,
        color,
        onArtist = { a -> before(); openArtistCredit(nav, a, scope) },
        onAlbum = { before(); nav.open(Screen.Album(track.albumId)) },
        withAlbum = withAlbum,
        suffix = suffix,
    )
}
