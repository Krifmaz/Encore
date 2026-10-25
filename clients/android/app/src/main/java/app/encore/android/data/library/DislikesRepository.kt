package app.encore.android.data.library

import app.encore.android.data.api.EncoreApi
import app.encore.android.features.offline.OfflineMode
import app.encore.android.ui.components.Toasts
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The ids of the songs this account disliked (the player's thumbs-down, the server's /api/taste/dislikes): a disliked
 * song is never queued by a shuffle, a mix or a radio again and leaves Liked Songs. [toggle] updates the set at once
 * and then tells the server; a dislike needs the server (there is no outbox for it), so offline it says so instead.
 */
class DislikesRepository(private val api: EncoreApi, private val likes: LikesRepository, private val scope: CoroutineScope) {
    private val _disliked = MutableStateFlow<Set<Long>>(emptySet())

    /** The disliked song ids as last known. */
    val disliked: StateFlow<Set<Long>> = _disliked.asStateFlow()

    /** when the list was last fetched (ms), for [refresh]'s once-a-minute budget */
    private var lastRefresh = 0L

    init {
        // a like takes a dislike back on the server (the two are opposites): the thumbs follow here
        scope.launch {
            likes.liked.collect { liked ->
                if (_disliked.value.any { it in liked }) _disliked.value = _disliked.value - liked
            }
        }
    }

    /** Fetch the list again: at most once a minute unless forced. */
    fun refresh(force: Boolean = false) {
        val now = System.currentTimeMillis()
        if (!force && now - lastRefresh < 60_000L) return
        if (OfflineMode.active.value) return
        lastRefresh = now
        scope.launch {
            try {
                _disliked.value = withContext(Dispatchers.IO) { api.dislikes() }
            } catch (_: Exception) {
                lastRefresh = 0L
            }
        }
    }

    /** Signed out: forget the list. */
    fun clear() {
        _disliked.value = emptySet()
        lastRefresh = 0L
    }

    /** Whether the song is disliked. */
    fun isDisliked(id: Long): Boolean = id in _disliked.value

    /**
     * Dislike the song, or take the dislike back. A dislike also takes the song out of Liked Songs (the server does
     * it; the hearts follow here). [onDisliked] runs on the main thread once the server took a new dislike (the
     * player moves on from the song).
     */
    fun toggle(id: Long, onDisliked: () -> Unit = {}) {
        if (OfflineMode.active.value) { Toasts.show("Dislikes need the server – try again once it's back", "err"); return }
        val was = id in _disliked.value
        _disliked.value = if (was) _disliked.value - id else _disliked.value + id
        scope.launch {
            try {
                withContext(Dispatchers.IO) { if (was) api.undislike(id) else api.dislike(id) }
                if (was) {
                    Toasts.show("Dislike taken back", "ok")
                } else {
                    likes.forget(id)
                    Toasts.show("Got it – you won't hear this song in shuffles and mixes again", "ok")
                    onDisliked()
                }
            } catch (e: Exception) {
                // put it back the way the server has it
                _disliked.value = if (was) _disliked.value + id else _disliked.value - id
                Toasts.show(e.message ?: "Couldn't save the dislike", "err")
            }
        }
    }

    /** "Don't play <artist>": none of the artist's songs in shuffles and mixes again. */
    fun dislikeArtist(name: String) {
        if (name.isBlank()) return
        scope.launch {
            try {
                withContext(Dispatchers.IO) { api.dislikeArtist(name) }
                Toasts.show("Got it – no more $name in shuffles and mixes", "ok")
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't save that", "err")
            }
        }
    }
}
