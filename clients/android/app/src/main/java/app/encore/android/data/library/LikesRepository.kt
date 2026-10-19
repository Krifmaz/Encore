package app.encore.android.data.library

import app.encore.android.data.api.EncoreApi
import app.encore.android.features.offline.OfflineMode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The ids of the user's liked songs; like/unlike update the set at once and then tell the server. While the server
 * can't be reached, a like waits in the outbox (features/offline) and the hearts use the list this phone kept.
 */
class LikesRepository(private val api: EncoreApi, private val scope: CoroutineScope) {
    private val _liked = MutableStateFlow<Set<Long>>(emptySet())
    val liked: StateFlow<Set<Long>> = _liked.asStateFlow()
    private var lastRefresh = 0L

    /** At most once a minute unless forced (PLAN §5 freshness budget). */
    fun refresh(force: Boolean = false) {
        val now = System.currentTimeMillis()
        if (!force && now - lastRefresh < 60_000L) return
        lastRefresh = now
        scope.launch {
            try {
                // the likes still waiting in the outbox count already; the list is kept for the hearts offline
                _liked.value = withContext(Dispatchers.IO) { OfflineMode.withPending(api.likes()).also { OfflineMode.keepLikes(it) } }
            } catch (_: Exception) {
                lastRefresh = 0L
                if (_liked.value.isEmpty()) _liked.value = withContext(Dispatchers.IO) { OfflineMode.withPending(OfflineMode.keptLikes()) }
            }
        }
    }

    fun clear() {
        _liked.value = emptySet()
        lastRefresh = 0L
    }

    fun isLiked(id: Long): Boolean = id in _liked.value

    fun toggle(id: Long) {
        val was = id in _liked.value
        _liked.value = if (was) _liked.value - id else _liked.value + id
        scope.launch {
            // offline, or an earlier tap on this song still waits: through the outbox, so the server gets them in order
            if (OfflineMode.active.value || withContext(Dispatchers.IO) { OfflineMode.hasLike(id) }) {
                withContext(Dispatchers.IO) { OfflineMode.queueLike(id, !was, _liked.value) }
                // online with an earlier tap waiting: send them now, in order (nothing while offline)
                OfflineMode.syncSoon()
                return@launch
            }
            try {
                withContext(Dispatchers.IO) { if (was) api.unlike(id) else api.like(id) }
                // the server answers again: plays and likes from a short gap go now
                OfflineMode.syncSoon()
            } catch (e: Exception) {
                if (OfflineMode.unreachable(e)) {
                    withContext(Dispatchers.IO) { OfflineMode.queueLike(id, !was, _liked.value) }
                } else {
                    // put it back the way the server has it
                    _liked.value = if (was) _liked.value + id else _liked.value - id
                }
            }
        }
    }
}
