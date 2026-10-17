package app.encore.android.ui.screens

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
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
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull

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
 * Where an artist name goes (web `artistUrl` / `#/artist-name/`): the full artist page (MusicBrainz: library
 * songs, Play/Shuffle, discography) by its id, else looked up by name (aliases too); the library artist list
 * only when MusicBrainz doesn't know the artist. The Library tab's own artist grid keeps the library view.
 */
fun openArtistCredit(nav: Nav, a: ArtistCredit, scope: CoroutineScope) {
    if (a.name.isEmpty() && a.mbid.isEmpty()) return
    openCatalogArtist(nav, a.mbid, a.name, scope)
}

/** The lookup shows a hint once it takes this long (a library artist answers at once: no flash). */
private const val HINT_AFTER_MS = 300L
/** A lookup that takes longer than this (a dead Wi-Fi, a slow MusicBrainz) gives way to the library page. */
private const val LOOKUP_TIMEOUT_MS = 8_000L

/**
 * Opens the catalogue page of an artist; without an id the name is looked up on the server (the web's
 * `/artist-name/`: the library's MusicBrainz id, else a MusicBrainz search with aliases). While the lookup
 * runs a "Looking up …" hint shows; with no network at all the library artist page opens at once instead of
 * waiting for the request to fail, and a lookup that fails or takes too long ends there as well.
 */
fun openCatalogArtist(nav: Nav, mbid: String, name: String, scope: CoroutineScope) {
    if (mbid.isNotEmpty()) {
        nav.open(Screen.CatalogArtist(mbid))
        return
    }
    if (name.isEmpty()) return
    if (!online(nav)) {
        // offline: the library knows this artist's songs; MusicBrainz can wait for the network
        nav.open(Screen.Artist(name))
        return
    }
    scope.launch {
        val hint = launch {
            delay(HINT_AFTER_MS)
            Toasts.show("Looking up $name…")
        }
        val id = try {
            withTimeoutOrNull(LOOKUP_TIMEOUT_MS) { withContext(Dispatchers.IO) { nav.features.artistLookup(name) } } ?: ""
        } catch (_: Exception) {
            ""
        }
        hint.cancel()
        if (id.isNotEmpty()) nav.open(Screen.CatalogArtist(id)) else nav.open(Screen.Artist(name))
    }
}

/** Is there a network with internet access right now? (ACCESS_NETWORK_STATE; true when the system can't say.) */
private fun online(nav: Nav): Boolean = try {
    val cm = nav.graph.app.getSystemService(Context.CONNECTIVITY_SERVICE) as? ConnectivityManager
    if (cm == null) {
        true
    } else {
        val net = cm.activeNetwork
        net != null && cm.getNetworkCapabilities(net)?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
    }
} catch (_: Exception) {
    true
}

/** [openArtistCredit] for a plain name (an album's artist, a search hit). */
fun openArtistPage(nav: Nav, name: String, scope: CoroutineScope = nav.graph.scope) {
    if (name.isEmpty()) return
    openCatalogArtist(nav, "", name, scope)
}

/** "Go to artist" for a song: its first credited artist (the album artist when the song has no split). */
fun openTrackArtist(nav: Nav, t: Track, scope: CoroutineScope = nav.graph.scope) {
    val a = t.artists.firstOrNull { it.name.isNotEmpty() || it.mbid.isNotEmpty() }
    if (a != null) openArtistCredit(nav, a, scope) else openArtistPage(nav, t.albumArtist.ifEmpty { t.artist }, scope)
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
