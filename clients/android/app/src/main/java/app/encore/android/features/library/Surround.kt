package app.encore.android.features.library

import androidx.compose.runtime.Composable
import app.encore.android.AppGraph
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.SurroundState
import app.encore.android.data.api.Track
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.features
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.Toasts
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The Surround library (0.35, web-rs/src/pages/library.rs): the Library's Surround tab, the album pages' links
 * between the stereo and surround versions, and "Get Dolby Atmos version" (admins). Surround songs have no likes,
 * playlists or plays stats, and none of the stereo library's correcting tools.
 */
object SurroundUi {
    private val _available = MutableStateFlow(false)
    /** The server has the Surround library (its `surround` feature); false until it said so, and on older servers. */
    val available: StateFlow<Boolean> = _available.asStateFlow()

    /** Asks the signed-in server's features (GET /api/auth/info), once per session at the root. */
    suspend fun probe(graph: AppGraph) {
        _available.value = false
        _available.value = try {
            withContext(Dispatchers.IO) { "surround" in graph.features.info().features }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            false
        }
    }

    /**
     * "Get Dolby Atmos version" (admins): [post] asks for it; the toast says how it went, and [done] runs once a
     * request was made.
     */
    fun getAtmos(scope: CoroutineScope, post: () -> Long, done: () -> Unit = {}) {
        Toasts.show("Looking for a Dolby Atmos version on Apple Music…", "info")
        scope.launch {
            try {
                val id = withContext(Dispatchers.IO) { post() }
                Toasts.show(SurroundRules.requestedToast(id), "ok")
                done()
            } catch (e: CancellationException) {
                throw e
            } catch (e: ApiException) {
                Toasts.show(e.message ?: "That didn't work", SurroundRules.errorKind(e.code))
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }

    /**
     * The song menu's surround entry for a stereo song: "Play the surround version" when the album page said it has
     * one, else "Get Dolby Atmos version" for admins. Empty for a surround song, or on a server without the feature.
     */
    fun entries(nav: Nav, t: Track, me: Me?, scope: CoroutineScope): List<MenuEntry?> {
        if (t.isSurround || !_available.value) return emptyList()
        if (t.surroundId > 0L) {
            return listOf(MenuEntry("Play the surround version", R.drawable.ic_play) {
                scope.launch {
                    try {
                        val s = withContext(Dispatchers.IO) { nav.graph.api.track(t.surroundId) }
                        if (s != null) nav.player.play(listOf(s)) else Toasts.show("The surround version is gone", "warn")
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "That didn't work", "err")
                    }
                }
            })
        }
        if (me?.isAdmin != true) return emptyList()
        return listOf(MenuEntry("Get Dolby Atmos version", R.drawable.ic_download) {
            getAtmos(scope, { nav.graph.api.requestTrackSurround(t.id) })
        })
    }

    /** A surround song's menu entry to its stereo album, when it has one. */
    fun stereoEntry(nav: Nav, t: Track): MenuEntry? =
        if (t.stereoAlbumId.isNotEmpty()) MenuEntry("Go to the stereo version", R.drawable.ic_disc) { nav.open(Screen.Album(t.stereoAlbumId)) } else null
}

/**
 * The album page's chips between the versions (library.rs `album_versions`), drawn in the page's row of chips: a
 * Surround album's "Stereo version →"; a stereo album's surround versions and the Dolby Atmos version being fetched.
 */
@Composable
fun SurroundVersions(nav: Nav, album: Album, state: SurroundState?) {
    if (album.isSurround) {
        if (album.stereoAlbumId.isNotEmpty()) Badge("Stereo version →", onClick = { nav.open(Screen.Album(album.stereoAlbumId)) })
        return
    }
    val s = state ?: return
    for (h in s.have) {
        Badge(SurroundRules.versionLabel(h.quality) + " →", kind = "accent", onClick = { nav.open(Screen.Album(h.albumId)) })
    }
    s.request?.let { r -> Badge(SurroundRules.requestLabel(r), onClick = { nav.open(Screen.Requests) }) }
}
