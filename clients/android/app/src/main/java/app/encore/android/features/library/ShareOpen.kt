package app.encore.android.features.library

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.platform.LocalContext
import app.encore.android.core.bool
import app.encore.android.core.long
import app.encore.android.core.str
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.session.Session
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.screens.openArtistPage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject

// Share links opened in the app (0.42.4). A share link (`https://<server>/s/<token>`) opened in an Android browser is
// handed to the app by the server's share page as `intent://<server>/s/<token>#Intent;scheme=encore;…` (the server's
// routes/share_open.rs); the manifest's `encore://` filter brings it to MainActivity, and [ShareOpenHost] opens the
// song's album (playing the song), the album, the artist or the playlist once signed in. When the app can't open it
// (not signed in, another server's link, a link that ended) the link goes back to the browser with `?app=0`, which
// the server answers with the share page or the web player instead of the app again.

/** The pure rules of [ShareOpen]: reading the app link, the browser fallback, and where the server's answer leads. */
object ShareOpenRules {
    /** The scheme of the app links the server's share page sends (the manifest's intent filter). */
    const val SCHEME = "encore"

    /** The intent extra with the share link itself (`S.link` of the server's intent link), http or https. */
    const val EXTRA_LINK = "link"

    /** The query that tells the server not to send the link back to the app (its `NO_APP`). */
    const val NO_APP = "app=0"

    /** A share token: the characters the server makes them of. */
    private val TOKEN = Regex("^[A-Za-z0-9_-]{6,128}$")

    /** A share link the app opened: its [token] and the [link] to give back to the browser. */
    data class Pending(val token: String, val link: String)

    /**
     * The token of an app link, `encore://<host>[/<path>]/s/<token>[/]`; null for any other link (a group link, a
     * typed `encore://` without a token).
     */
    fun tokenOf(data: String?): String? {
        if (data == null || !data.startsWith("$SCHEME://", ignoreCase = true)) return null
        val path = data.substringAfter("://").substringBefore('?').substringBefore('#').trimEnd('/')
        val tok = path.substringAfterLast("/s/", "")
        return tok.takeIf { path.contains("/s/") && TOKEN.matches(it) }
    }

    /**
     * The share link for the browser: the intent's [extra] when it is this token's http(s) link, else the app link
     * made https (`encore://music.example/s/t` → `https://music.example/s/t`).
     */
    fun linkOf(data: String, token: String, extra: String?): String {
        val e = extra?.trim().orEmpty()
        if ((e.startsWith("https://") || e.startsWith("http://")) && e.substringBefore('?').trimEnd('/').endsWith("/s/$token")) return e
        val rest = data.substringAfter("://").substringBefore('?').substringBefore('#').trimEnd('/')
        return "https://$rest"
    }

    /** The app link of this intent's data and extra as a [Pending], or null when it isn't one. */
    fun pendingOf(data: String?, extra: String?): Pending? {
        val token = tokenOf(data) ?: return null
        return Pending(token, linkOf(data!!, token, extra))
    }

    /** [link] with [NO_APP] added, so the server shows the page or the web player instead of the app again. */
    fun noAppLink(link: String): String {
        val (head, frag) = link.indexOf('#').let { if (it >= 0) link.substring(0, it) to link.substring(it) else link to "" }
        if (head.substringAfter('?', "").split('&').contains(NO_APP)) return link
        return head + (if (head.contains('?')) "&" else "?") + NO_APP + frag
    }

    /** Where the server's answer (`GET /api/share/<token>/open`) leads in the app. */
    sealed interface Go {
        /** An album page; [playTrack] > 0: that song of it starts playing (a song link). */
        data class Album(val id: String, val playTrack: Long) : Go
        /** An artist page, by the library's name. */
        data class Artist(val name: String) : Go
        /** The listener's own playlist, or their entry for someone else's. */
        data class Playlist(val id: Long, val name: String) : Go
        /** Someone else's playlist not in the listener's playlists: "Add to my playlists?" ([PlaylistLinkUi]). */
        data object AddPlaylist : Go
        /** Nothing the app can open: the link goes to the browser. */
        data object Browser : Go
    }

    /** [Go] for the server's answer `o`. */
    fun goOf(o: JsonObject): Go = when (o.str("kind")) {
        "track" -> o.str("album_id").takeIf { it.isNotEmpty() }?.let { Go.Album(it, o.long("track_id")) } ?: Go.Browser
        "album" -> o.str("album_id").takeIf { it.isNotEmpty() }?.let { Go.Album(it, 0L) } ?: Go.Browser
        "artist" -> o.str("name").takeIf { it.isNotEmpty() }?.let { Go.Artist(it) } ?: Go.Browser
        "playlist" -> o.long("playlist_id").takeIf { it > 0L && (o.bool("own") || o.bool("added")) }?.let { Go.Playlist(it, o.str("name")) } ?: Go.AddPlaylist
        else -> Go.Browser
    }
}

/** The share link the app was opened with, waiting for [ShareOpenHost] (MainActivity sets it). */
object ShareOpen {
    /** Set by MainActivity; [ShareOpenHost] takes it once signed in. */
    val pending = MutableStateFlow<ShareOpenRules.Pending?>(null)

    /**
     * Opens [link] in the browser with [ShareOpenRules.NO_APP], so it shows the share page (or the web player for a
     * browser signed in there). The app's own filter is `encore://` only, so this never comes back here.
     */
    fun openInBrowser(context: Context, link: String) {
        val i = Intent(Intent.ACTION_VIEW, Uri.parse(ShareOpenRules.noAppLink(link)))
            .addCategory(Intent.CATEGORY_BROWSABLE)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(i)
        } catch (_: ActivityNotFoundException) {
            // no browser at all (a locked-down TV): nothing to send it to
        }
    }
}

/**
 * Opens the pending share link ([ShareOpen.pending]) once signed in: asks the server where it is in the library and
 * opens that page, or hands the link back to the browser. Drawn once at the root, like [PlaylistLinkHost].
 */
@Composable
fun ShareOpenHost(nav: Nav, session: Session) {
    val pending by ShareOpen.pending.collectAsState()
    val context = LocalContext.current
    // the host's own scope: clearing the pending link restarts the effect, which must not cancel the look-up
    val scope = rememberCoroutineScope()
    LaunchedEffect(pending, session) {
        val p = pending ?: return@LaunchedEffect
        ShareOpen.pending.value = null
        scope.launch { openShared(nav, context, p) }
    }
}

/** `GET /api/share/<token>/open` on the signed-in server, then the page it names (see [ShareOpenRules.goOf]). */
private suspend fun openShared(nav: Nav, context: Context, p: ShareOpenRules.Pending) {
    val go = try {
        ShareOpenRules.goOf(withContext(Dispatchers.IO) { nav.graph.api.getObject("/api/share/${EncoreApi.enc(p.token)}/open") })
    } catch (e: CancellationException) {
        throw e
    } catch (_: Exception) {
        // another server's link (404 here), a link that ended, or no connection: the browser shows what it can
        ShareOpenRules.Go.Browser
    }
    when (go) {
        is ShareOpenRules.Go.Album -> nav.open(Screen.Album(go.id, playTrack = go.playTrack))
        is ShareOpenRules.Go.Artist -> openArtistPage(nav, go.name)
        is ShareOpenRules.Go.Playlist -> nav.open(Screen.Playlist(go.id, go.name))
        // "Add to my playlists?", as for a playlist link shared into the app
        ShareOpenRules.Go.AddPlaylist -> PlaylistLinkUi.pendingLink.value = p.link
        ShareOpenRules.Go.Browser -> ShareOpen.openInBrowser(context, p.link)
    }
}
