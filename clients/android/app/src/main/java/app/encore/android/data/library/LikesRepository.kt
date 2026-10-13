package app.encore.android.data.library

import app.encore.android.data.api.EncoreApi
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The ids of the user's liked songs; like/unlike update the set at once and then tell the server. */
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
                _liked.value = withContext(Dispatchers.IO) { api.likes() }
            } catch (_: Exception) {
                lastRefresh = 0L
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
            try {
                withContext(Dispatchers.IO) { if (was) api.unlike(id) else api.like(id) }
            } catch (_: Exception) {
                // put it back the way the server has it
                _liked.value = if (was) _liked.value + id else _liked.value - id
            }
        }
    }
}
