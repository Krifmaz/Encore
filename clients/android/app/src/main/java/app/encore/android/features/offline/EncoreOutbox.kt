package app.encore.android.features.offline

import dev.krifmaz.kit.compose.core.arr
import dev.krifmaz.kit.compose.core.bool
import dev.krifmaz.kit.compose.core.dbl
import dev.krifmaz.kit.compose.core.long
import dev.krifmaz.kit.compose.core.objectsOf
import dev.krifmaz.kit.compose.core.parseJsonObject
import dev.krifmaz.kit.compose.core.str
import dev.krifmaz.kit.compose.offline.OutAction
import dev.krifmaz.kit.compose.offline.OutMerge
import dev.krifmaz.kit.compose.offline.Outbox
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * Encore's changes in the kit's offline outbox ([Outbox]): the plays and the likes made while the server could not be
 * reached. The kit keeps, caps and counts them; this says what a play and a like are, how each merges, the body that
 * sends plays, the words for what waits, and reads the outbox file Encore wrote before the kit's (plays and likes in
 * two lists, [fromLegacy]) so nothing waiting on a phone is lost by an update.
 */
object EncoreOutbox {
    /** The [OutAction.kind] of a play. */
    const val PLAY = "play"

    /** The [OutAction.kind] of a like or an unlike. */
    const val LIKE = "like"

    /** The server takes at most this many plays per call (`POST /api/library/plays`). */
    const val PLAYS_PER_CALL = 500

    /** At most this many plays wait: the oldest go first. */
    const val MAX_PLAYS = 5000

    /** The words for each kind, for [countsText] and [backOnlineText]. */
    private val NOUNS: Map<String, Pair<String, String>> = mapOf(PLAY to ("play" to "plays"), LIKE to ("like" to "likes"))

    /**
     * A play the server did not get yet.
     *
     * @property trackId the song.
     * @property seconds how long it played.
     * @property playedAt when it played (Unix seconds).
     */
    data class Play(val trackId: Long, val seconds: Double, val playedAt: Long)

    /**
     * A like or an unlike the server did not get yet.
     *
     * @property trackId the song.
     * @property liked true for a like, false for an unlike.
     * @property at when the heart was pressed (Unix seconds).
     */
    data class Like(val trackId: Long, val liked: Boolean, val at: Long)

    /** A play as an outbox change: one per song and second. */
    private fun actionOf(p: Play): OutAction = OutAction(PLAY, p.trackId.toString(), buildJsonObject { put("seconds", p.seconds) }, p.playedAt)

    /** A like as an outbox change: the data holds only `liked`, so the same tap again changes nothing. */
    private fun actionOf(l: Like): OutAction = OutAction(LIKE, l.trackId.toString(), buildJsonObject { put("liked", l.liked) }, l.at)

    /** A play, once (the same song at the same second is the same play); at most [MAX_PLAYS] wait. */
    fun addPlay(o: Outbox, p: Play): Outbox = if (p.trackId <= 0L) o else o.add(actionOf(p), OutMerge.Each, MAX_PLAYS)

    /** A like or an unlike replaces the one still waiting for the same song: the last tap is what the server gets. */
    fun addLike(o: Outbox, l: Like): Outbox = if (l.trackId <= 0L) o else o.add(actionOf(l), OutMerge.Latest, Int.MAX_VALUE)

    /** The plays waiting, oldest first, each with the change it came from (for [Outbox.forget]). */
    fun plays(o: Outbox): List<Pair<OutAction, Play>> = o.of(PLAY).mapNotNull { a ->
        a.key.toLongOrNull()?.takeIf { it > 0L }?.let { a to Play(it, a.data.dbl("seconds"), a.at) }
    }

    /** The likes and unlikes waiting, in the order they were made, each with the change it came from. */
    fun likes(o: Outbox): List<Pair<OutAction, Like>> = o.of(LIKE).mapNotNull { a ->
        a.key.toLongOrNull()?.takeIf { it > 0L }?.let { a to Like(it, a.data.bool("liked"), a.at) }
    }

    /** The body of `POST /api/library/plays` for these plays (the list form, 0.28): seconds to one decimal. */
    fun playsBody(plays: List<Play>): JsonObject = buildJsonObject {
        put(
            "plays",
            buildJsonArray {
                for (p in plays) {
                    add(
                        buildJsonObject {
                            put("track_id", p.trackId)
                            put("seconds", Math.round(p.seconds.coerceAtLeast(0.0) * 10.0) / 10.0)
                            put("played_at", p.playedAt)
                        },
                    )
                }
            },
        )
    }

    /** "3 plays and 1 like", "1 play", "2 likes" ("" for none). */
    fun countsText(plays: Int, likes: Int): String = Outbox.countsText(linkedMapOf(PLAY to plays, LIKE to likes), NOUNS)

    /** The toast once the server answers again: "Back online – 3 plays sent" ("Back online" when nothing went). */
    fun backOnlineText(plays: Int, likes: Int): String = Outbox.backOnlineText(linkedMapOf(PLAY to plays, LIKE to likes), NOUNS)

    /**
     * The outbox document for the text of Encore's own outbox file from before the kit's (`{owner, plays: [{track_id,
     * seconds, played_at}], likes: [{track_id, liked, at}]}`), with the same rules as [addPlay] and [addLike]; null
     * when the text is not that (already the kit's, empty or unreadable).
     */
    fun fromLegacy(text: String?): Outbox? {
        val o = parseJsonObject(text) ?: return null
        if ("actions" in o || ("plays" !in o && "likes" !in o)) return null
        var out = Outbox(owner = o.str("owner"))
        for (p in o.arr("plays").objectsOf()) out = addPlay(out, Play(p.long("track_id"), p.dbl("seconds"), p.long("played_at")))
        for (l in o.arr("likes").objectsOf()) out = addLike(out, Like(l.long("track_id"), l.bool("liked"), l.long("at")))
        return out
    }
}
