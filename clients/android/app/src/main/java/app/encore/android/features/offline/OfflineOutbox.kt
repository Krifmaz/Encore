package app.encore.android.features.offline

import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.long
import app.encore.android.core.objectsOf
import app.encore.android.core.parseJsonObject
import app.encore.android.core.str
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.File

/** A play the server did not get yet: how long it played and when (Unix seconds). */
data class OutPlay(val trackId: Long, val seconds: Double, val playedAt: Long)

/** A like (`liked`) or an unlike the server did not get yet, and when the heart was pressed. */
data class OutLike(val trackId: Long, val liked: Boolean, val at: Long)

/**
 * Plays and likes made while the server could not be reached (offline-manifest.md "The outbox", the phone's copy
 * of the web's rules). It belongs to the account that made them: [owner] is `<server>|<username>`, a change by
 * another account drops it first, and it is only ever sent under that account's session.
 */
data class Outbox(val owner: String = "", val plays: List<OutPlay> = emptyList(), val likes: List<OutLike> = emptyList()) {
    val size: Int get() = plays.size + likes.size
    val isEmpty: Boolean get() = size == 0

    /** A change by `owner` is coming: another account's outbox is dropped first. */
    fun own(owner: String): Outbox = if (this.owner == owner) this else Outbox(owner)

    fun belongsTo(owner: String): Boolean = owner.isNotEmpty() && this.owner == owner

    /** A play, once (the same song at the same second is the same play); at most [MAX_PLAYS], the oldest go first. */
    fun addPlay(p: OutPlay): Outbox {
        if (p.trackId <= 0L || plays.any { it.trackId == p.trackId && it.playedAt == p.playedAt }) return this
        val list = plays + p
        return copy(plays = if (list.size > MAX_PLAYS) list.drop(list.size - MAX_PLAYS) else list)
    }

    /** A like or an unlike replaces the one still waiting for the same song: the last tap is what the server gets. */
    fun addLike(l: OutLike): Outbox {
        if (l.trackId <= 0L) return this
        val i = likes.indexOfFirst { it.trackId == l.trackId }
        return when {
            i < 0 -> copy(likes = likes + l)
            likes[i].liked == l.liked -> this
            else -> copy(likes = likes.toMutableList().also { it[i] = l })
        }
    }

    /** What went: the plays sent, and the likes sent (a like that changed meanwhile stays). */
    fun forget(sentPlays: List<OutPlay>, sentLikes: List<OutLike>): Outbox {
        val sent = sentPlays.map { it.trackId to it.playedAt }.toHashSet()
        return copy(
            plays = plays.filter { (it.trackId to it.playedAt) !in sent },
            likes = likes.filter { l -> sentLikes.none { it.trackId == l.trackId && it.liked == l.liked } },
        )
    }

    fun toJson(): JsonObject = buildJsonObject {
        put("owner", owner)
        put("plays", buildJsonArray {
            for (p in plays) add(buildJsonObject { put("track_id", p.trackId); put("seconds", p.seconds); put("played_at", p.playedAt) })
        })
        put("likes", buildJsonArray {
            for (l in likes) add(buildJsonObject { put("track_id", l.trackId); put("liked", l.liked); put("at", l.at) })
        })
    }

    companion object {
        /** The server takes at most this many plays per call. */
        const val PLAYS_PER_CALL = 500
        /** At most this many plays wait. */
        const val MAX_PLAYS = 5000

        /** The saved document; an unreadable one is empty. */
        fun parse(text: String?): Outbox {
            val o = parseJsonObject(text) ?: return Outbox()
            return Outbox(
                owner = o.str("owner"),
                plays = o.arr("plays").objectsOf().map { OutPlay(it.long("track_id"), it.dbl("seconds"), it.long("played_at")) }.filter { it.trackId > 0L },
                likes = o.arr("likes").objectsOf().map { OutLike(it.long("track_id"), it.bool("liked"), it.long("at")) }.filter { it.trackId > 0L },
            )
        }

        /** The body of `POST /api/library/plays` for these plays (the list form, 0.28). */
        fun playsBody(plays: List<OutPlay>): JsonObject = buildJsonObject {
            put("plays", buildJsonArray {
                for (p in plays) add(buildJsonObject {
                    put("track_id", p.trackId)
                    put("seconds", Math.round(p.seconds.coerceAtLeast(0.0) * 10.0) / 10.0)
                    put("played_at", p.playedAt)
                })
            })
        }

        /** "3 plays and 1 like", "1 play", "2 likes" ("" for none). */
        fun countsText(plays: Int, likes: Int): String {
            fun one(n: Int, word: String) = "$n $word" + if (n == 1) "" else "s"
            return when {
                plays == 0 && likes == 0 -> ""
                likes == 0 -> one(plays, "play")
                plays == 0 -> one(likes, "like")
                else -> one(plays, "play") + " and " + one(likes, "like")
            }
        }

        /** The toast once the server answers again: "Back online – 3 plays sent". */
        fun backOnlineText(plays: Int, likes: Int): String {
            val sent = countsText(plays, likes)
            return if (sent.isEmpty()) "Back online" else "Back online – $sent sent"
        }
    }
}

/** The outbox kept in a file of the app's own storage; every change is written at once. Thread-safe. */
class OutboxFile(private val file: File) {
    private val lock = Any()
    private var doc: Outbox? = null
    private val _changes = MutableStateFlow(0)

    /** Moves on every change, so a screen showing what waits (the Downloaded list's note) reads it again. */
    val changes: StateFlow<Int> = _changes.asStateFlow()

    private fun loaded(): Outbox {
        doc?.let { return it }
        val d = try { if (file.isFile) Outbox.parse(file.readText()) else Outbox() } catch (_: Exception) { Outbox() }
        doc = d
        return d
    }

    fun read(): Outbox = synchronized(lock) { loaded() }

    /** Applies `change` and writes the result (kept in memory when the write fails, e.g. a full phone). */
    fun update(change: (Outbox) -> Outbox): Outbox = synchronized(lock) {
        val before = loaded()
        val after = change(before)
        if (after != before) {
            doc = after
            try {
                file.parentFile?.mkdirs()
                val tmp = File(file.parentFile, file.name + ".tmp")
                tmp.writeText(after.toJson().toString())
                if (!tmp.renameTo(file)) { file.delete(); tmp.renameTo(file) }
            } catch (_: Exception) { /* kept in memory until the next write */ }
            _changes.value = _changes.value + 1
        }
        after
    }
}
