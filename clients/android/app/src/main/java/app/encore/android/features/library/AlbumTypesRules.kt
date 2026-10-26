package app.encore.android.features.library

import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.int
import app.encore.android.core.obj
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.core.strings
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import java.net.URLEncoder

/**
 * Library types, Select / Delete… and who added it (0.36, docs/contracts/library-types.md §2.4 and §6): the pure parts
 * of the Library's albums list, the album page, the artist pages and the song ⋯ menu – the texts of who added a song
 * and the album's history line, the Type options, the albums list's address, the selection and its chunks, the texts
 * of the Delete… dialog and its toasts, the Change type choices and the One-offs shelf. The same words and rules as
 * web-rs/src/pages/library_types.rs and the desktop's library_types.rs.
 */
object AlbumTypesRules {
    /** Most albums one Select all picks (the server's `ids_only` cap). */
    const val SELECT_MAX = 5000
    /** Albums one Move to Trash call moves. */
    const val CHUNK = 100
    /** Songs of the One-offs shelf before "Show all N", and how many "Show all N" loads. */
    const val ONE_OFFS_FIRST = 10
    const val ONE_OFFS_ALL = 500
    /** How long the toast after a Move to Trash stays with its Open Trash button. */
    const val TOAST_MS = 8000L

    const val TYPE = "Type"
    const val ALL_TYPES = "All types"
    const val NO_ALBUMS_OF_TYPE = "No albums of this type"
    const val AT_MOST = "At most 5000 albums at a time"
    const val TRASH_BODY = "Their songs go to the Trash in Files, where you can restore them. The space is freed when the Trash is emptied."
    /** The Delete dialog's words while Keep deleted files in Trash is off (the default since 0.38). */
    const val DELETE_BODY = "Their songs are deleted for good. To be able to restore what you delete, turn on Keep deleted files in Trash in Settings → Library → Deleting files."
    const val OTHER_DISK = " The Trash is on another disk, so the files are copied there first – this can take a while."
    const val ONE_OFF_HELP = "A song or two from a compilation, remix album or soundtrack: shown on the artist's page under One-offs, not with the albums."
    const val ONE_OFFS = "One-offs"
    const val ONE_OFFS_LINE = "Songs from compilations, remix albums and soundtracks where you have only a few songs."
    const val CHANGE_TYPE = "Change type…"
    const val CHANGE_TYPE_TITLE = "Change type"
    const val TYPE_SAVED = "Type saved"
    const val MOVE_ALBUM = "Delete album…"
    const val MOVE_TO_TRASH = "Move to Trash"
    /** The dialog's button while deletes are for good. */
    const val DELETE_GO = "Delete"
    const val OPEN_TRASH = "Open Trash"
    const val SELECT = "Select"
    const val CLEAR_SELECTION = "Clear selection"
    const val DELETE = "Delete…"
    const val DONE = "Done"
    /** The first reason when a call moved fewer albums than it was given and named none. */
    const val GONE = "It is no longer in the library"

    /** The admin part of the album page's ⋯, in its order: Change type… after Edit details, Delete album… last. */
    fun albumAdminItems(admin: Boolean, hasType: Boolean): List<String> {
        if (!admin) return emptyList()
        val v = mutableListOf("Tag / match metadata", "Edit details (album, artist, songs)")
        if (hasType) v += CHANGE_TYPE
        v += listOf("Change artwork…", "Find artwork automatically", MOVE_ALBUM)
        return v
    }

    // ---- who added it (§2.4) -------------------------------------------------------------------------------------

    /** Is this how a request comes in (a request, a link from the Downloader, a download from the Soulseek page)? */
    fun requestLike(via: String): Boolean = via == "request" || via == "link" || via == "soulseek"

    /** Who added a song, in words ([day]: the phone's day of `requested_at`, when there is one). */
    fun addedText(via: String, by: String?, byDeleted: Boolean, day: String?): String {
        val who = by?.takeIf { it.isNotEmpty() }
        return when {
            requestLike(via) -> when {
                who != null && day != null -> "Requested by $who on $day"
                who != null -> "Requested by $who"
                byDeleted && day != null -> "Requested by a deleted user on $day"
                byDeleted -> "Requested by a deleted user"
                else -> "Requested by someone – record cleared"
            }
            via == "upload" -> when {
                who != null -> "Uploaded by $who"
                byDeleted -> "Uploaded by a deleted user"
                else -> ""
            }
            via == "import" -> when {
                who != null -> "Imported by $who"
                byDeleted -> "Imported by a deleted user"
                else -> ""
            }
            via == "scan" -> "Found in the music folder"
            else -> ""
        }
    }

    /** A JSON string, or null for a missing / null / empty one. */
    private fun JsonObject.textOrNull(key: String): String? {
        val v = this[key] as? JsonPrimitive ?: return null
        if (v is JsonNull) return null
        return v.content.takeIf { it.isNotEmpty() }
    }

    /** [addedText] of an answer of GET /api/library/tracks/{id}/added, or of an album's `history` group. */
    fun addedOf(o: JsonObject, day: (Double) -> String): String {
        val at = o.dbl("requested_at")
        return addedText(o.str("via"), o.textOrNull("by"), o.bool("by_deleted"), if (at > 0) day(at) else null)
    }

    /**
     * The album page's history line: at most three groups' texts, " · and N more", then the day it was added
     * (" · added DAY", or "Added DAY" when no group says anything).
     */
    fun historyLine(groups: List<String>, addedDay: String?): String {
        val texts = groups.filter { it.isNotEmpty() }
        var out = texts.take(3).joinToString(" · ")
        if (texts.size > 3) out += " · and ${texts.size - 3} more"
        return when {
            addedDay == null -> out
            out.isEmpty() -> "Added $addedDay"
            else -> "$out · added $addedDay"
        }
    }

    /**
     * The history line of a GET /api/library/albums/{id} answer: the server's `history` groups and `added_at` (0.36),
     * or – from an older server – its requesters joined here ("" when there is nothing to say).
     */
    fun albumHistory(o: JsonObject, day: (Double) -> String): String {
        val rows = o.arr("tracks").objectsOf()
        val requestedAt = rows.map { it.dbl("requested_at") }.filter { it > 0 }.minOrNull()
        val addedAt = rows.map { it.dbl("added_at") }.filter { it > 0 }.minOrNull()
        val history = o.arr("history")
        if (history != null) {
            val groups = history.objectsOf().map { addedOf(it, day) }
            val added = o.dbl("added_at").takeIf { it > 0 } ?: addedAt
            return historyLine(groups, added?.let(day))
        }
        val requesters = rows.map { it.str("requested_by") }.filter { it.isNotEmpty() }.distinct().joinToString(", ")
        return listOf(
            if (requesters.isNotEmpty()) "Requested by $requesters" + (if (requestedAt != null) " on ${day(requestedAt)}" else "") else "",
            if (addedAt != null) (if (requesters.isNotEmpty()) "added " else "Added ") + day(addedAt) else "",
        ).filter { it.isNotEmpty() }.joinToString(" · ")
    }

    /**
     * Who asked for a song, for its row's note: the adder of a request-like song (kept after Clear finished), or – from
     * a server before 0.36 – the request's user. ("", 0) when nobody asked.
     */
    fun requester(row: JsonObject): Pair<String, Double> {
        val via = row.str("added_via")
        if (via.isNotEmpty()) return if (requestLike(via)) row.str("added_by_name") to row.dbl("requested_at") else "" to 0.0
        return row.str("requested_by") to row.dbl("requested_at")
    }

    /** A song row's "requested by X on DAY" ("" when nobody asked). */
    fun requesterNote(row: JsonObject, day: (Double) -> String): String {
        val (who, at) = requester(row)
        if (who.isEmpty()) return ""
        return "requested by $who" + if (at > 0) " on ${day(at)}" else ""
    }

    // ---- the Type dropdown and the albums list (§6.1) -------------------------------------------------------------

    /** One type of GET /api/library/album-types. */
    data class TypeCount(val key: String, val label: String, val count: Int)

    fun typeCounts(o: JsonObject): List<TypeCount> =
        o.arr("types").objectsOf().mapNotNull { t -> t.str("key").takeIf { it.isNotEmpty() }?.let { TypeCount(it, t.str("label"), t.int("count")) } }

    /** The text of one entry of the Type dropdown: "Live (12)". */
    fun typeOptionText(label: String, count: Int): String = "$label ($count)"

    /** The Type dropdown's entries (key, text): All types first, then the types with albums (and the one picked). */
    fun typeOptions(types: List<TypeCount>, selected: String): List<Pair<String, String>> =
        listOf("" to ALL_TYPES) + types.filter { it.count > 0 || it.key == selected }.map { it.key to typeOptionText(it.label, it.count) }

    /** The chip's words: "Type" with All types, else the picked type's label. */
    fun typeChipText(types: List<TypeCount>, selected: String): String =
        if (selected.isEmpty()) TYPE else types.firstOrNull { it.key == selected }?.label ?: TYPE

    private fun enc(s: String): String = URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    /**
     * GET /api/library/albums for the phone's albums list: no `kind` (every album but the One-offs, a search finds
     * them too); a Type, when picked, decides alone.
     */
    fun albumsUrl(sort: String, dir: String, q: String, artist: String, type: String, offset: Int, limit: Int): String {
        val qs = StringBuilder("/api/library/albums?sort=").append(enc(sort)).append("&limit=").append(limit).append("&offset=").append(offset)
        if (dir.isNotEmpty()) qs.append("&dir=").append(enc(dir))
        if (q.isNotEmpty()) qs.append("&q=").append(enc(q))
        if (artist.isNotEmpty()) qs.append("&artist=").append(enc(artist))
        if (type.isNotEmpty()) qs.append("&type=").append(enc(type))
        return qs.toString()
    }

    /** Every id of the list as it is filtered (Select all): the same address with `ids_only=1`. */
    fun idsUrl(sort: String, dir: String, q: String, artist: String, type: String): String {
        val qs = StringBuilder("/api/library/albums?sort=").append(enc(sort))
        if (dir.isNotEmpty()) qs.append("&dir=").append(enc(dir))
        if (q.isNotEmpty()) qs.append("&q=").append(enc(q))
        if (artist.isNotEmpty()) qs.append("&artist=").append(enc(artist))
        if (type.isNotEmpty()) qs.append("&type=").append(enc(type))
        return qs.append("&ids_only=1").toString()
    }

    /** GET /api/library/album-types for an artist ("" = everyone's) and the album filter. */
    fun typesUrl(artist: String = "", filter: String = ""): String = "/api/library/album-types?artist=${enc(artist)}&filter=${enc(filter)}"

    /** `{total, ids, capped}` of an `ids_only=1` answer. */
    data class AlbumIds(val total: Int, val ids: List<String>, val capped: Boolean) {
        companion object {
            fun from(o: JsonObject): AlbumIds {
                val ids = o.arr("ids").strings().filter { it.isNotEmpty() }
                return AlbumIds(o.int("total").coerceAtLeast(ids.size), ids, o.bool("capped"))
            }
        }
    }

    // ---- the selection (§6.1) ------------------------------------------------------------------------------------

    /** The picked albums with [id] picked or unpicked (in the order picked, each once). */
    fun toggle(picked: List<String>, id: String): List<String> = if (id in picked) picked - id else picked + id

    /** Select all: these ids (at most [SELECT_MAX]), and whether the list had more. */
    fun selectAll(ids: List<String>): Pair<List<String>, Boolean> = ids.distinct().take(SELECT_MAX) to (ids.size > SELECT_MAX)

    /** The real runs of a Move to Trash: the selection in order, [CHUNK] albums a call. */
    fun chunks(ids: List<String>): List<List<String>> = ids.chunked(CHUNK)

    fun selectedText(n: Int): String = "$n selected"

    fun selectAllText(total: Int): String = "Select all $total"

    fun movingText(done: Int, n: Int): String = "Moving to the Trash… $done of $n"

    /** The toast after a Move to Trash: how many went of how many, and the first reason when some couldn't. */
    fun movedText(moved: Int, total: Int, firstReason: String): String {
        if (moved == total) return if (moved == 1) "Moved 1 album to the Trash" else "Moved $moved albums to the Trash"
        return "Moved $moved of $total albums to the Trash – ${total - moved} couldn't be moved: $firstReason"
    }

    /** The toast after a delete for good (Keep deleted files in Trash off). */
    fun deletedText(deleted: Int, total: Int, firstReason: String): String {
        if (deleted == total) return if (deleted == 1) "Deleted 1 album" else "Deleted $deleted albums"
        return "Deleted $deleted of $total albums – ${total - deleted} couldn't be deleted: $firstReason"
    }

    /** Does a dry run's (or a run's) answer say the Trash keeps what is deleted? (A server before 0.38 always did.) */
    fun keeps(d: JsonObject): Boolean {
        val k = d["keep"]
        return !(k is JsonPrimitive && k !is JsonNull && k.content == "false")
    }

    /** The dialog's button: Move to Trash, or Delete while deletes are for good. */
    fun goLabel(keep: Boolean): String = if (keep) MOVE_TO_TRASH else DELETE_GO

    /** The sums of the real runs: albums moved, and the first reason one couldn't be; [keep] false: deleted for good. */
    data class Moved(val moved: Int = 0, val firstReason: String = "", val keep: Boolean = true) {
        /** After one call's answer (`albums`, `failed[].reason`). */
        fun plus(answer: JsonObject): Moved = Moved(
            moved + answer.int("albums").coerceAtLeast(0),
            firstReason.ifEmpty { answer.arr("failed").objectsOf().firstOrNull()?.str("reason").orEmpty() },
            keep,
        )

        /** After a call that failed. */
        fun failed(message: String): Moved = copy(firstReason = firstReason.ifEmpty { message })

        /** The toast for [n] albums asked for. */
        fun text(n: Int): String {
            val reason = if (firstReason.isEmpty() && moved < n) GONE else firstReason
            return if (keep) movedText(moved, n, reason) else deletedText(moved, n, reason)
        }
    }

    /**
     * The Delete… dialog's text: deleted for good, or the Trash's words with its days and the other-disk sentence; what
     * stays on its own line.
     */
    fun confirmBody(keep: Boolean, days: Int, otherDisk: Boolean, skippedText: String): String {
        var body = when {
            !keep -> DELETE_BODY
            days > 0 -> "Their songs go to the Trash in Files, where you can restore them for $days days. Then they are deleted for good."
            else -> TRASH_BODY
        }
        if (otherDisk) body += OTHER_DISK
        if (skippedText.isNotEmpty()) body += "\n" + skippedText
        return body
    }

    /** The Delete… / Move album to Trash dialog from a dry run: its title, its words, and false when only Close. */
    data class TrashDialog(val title: String, val text: String, val canGo: Boolean, val keep: Boolean = true)

    fun trashDialog(dry: JsonObject): TrashDialog {
        val room = dry["room"]
        val keep = keeps(dry)
        if (room is JsonPrimitive && room !is JsonNull && room.content == "false") return TrashDialog(dry.str("text"), dry.str("room_text"), false, keep)
        return TrashDialog(dry.str("text"), confirmBody(keep, dry.int("days"), dry.bool("other_disk"), dry.str("skipped_text")), true, keep)
    }

    // ---- the album page (§6.2) -----------------------------------------------------------------------------------

    /** The Change type dialog's question. */
    fun whatIs(album: String): String = "What is “$album”?"

    /** The Change type dialog's automatic choice: "Automatic (Live · EP – From MusicBrainz)". */
    fun automaticLabel(label: String, why: String): String = "Automatic ($label – $why)"

    /** The Change type dialog's choices (value, label): the four formats, then Automatic (""). */
    fun typeChoices(t: JsonObject): List<Pair<String, String>> {
        val auto = t.obj("auto") ?: JsonObject(emptyMap())
        val why = auto.str("why_text").ifEmpty { t.str("why_text") }
        return listOf(
            "album" to "Album", "ep" to "EP", "single" to "Single", "one_off" to "One-off",
            "" to automaticLabel(auto.str("label"), why),
        )
    }

    /** The choice checked when the dialog opens: the owner's, else Automatic (""). */
    fun chosen(t: JsonObject): String = t.str("chosen").takeIf { it in listOf("album", "ep", "single", "one_off") } ?: ""

    // ---- the One-offs shelf (§6.3) -------------------------------------------------------------------------------

    /** "Show all N" of the One-offs shelf ("" when all are shown). */
    fun showAllText(total: Int, shown: Int): String = if (total > shown) "Show all $total" else ""

    /** A One-off's sub-line: "Now 99 · Various artists". */
    fun oneOffSub(album: String, releaseLabel: String): String = listOf(album, releaseLabel).filter { it.isNotEmpty() }.joinToString(" · ")
}
