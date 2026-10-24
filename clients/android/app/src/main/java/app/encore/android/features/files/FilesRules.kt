package app.encore.android.features.files

import app.encore.android.core.arr
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/**
 * The file manager's plain rules, with no Android and no network: paths, the server's name rule, the sort words, the
 * link a browsing went through, and the lines that say what happened. The web's `pages/files/logic.rs` on the phone
 * (the same rules and words), kept apart so the unit tests check them.
 */
object FilesRules {
    // ---- paths ----------------------------------------------------------------------------------------------------

    /** "a/b/c" → "a/b"; "a" → "". */
    fun parentOf(path: String): String = if (path.contains('/')) path.substringBeforeLast('/') else ""

    /** "Artist" + "Album" → "Artist/Album"; "" + "Artist" → "Artist". */
    fun join(path: String, name: String): String = if (path.isEmpty()) name else "$path/$name"

    /** "Library › Artist › Album" (the root's name and the path's parts). */
    fun place(rootName: String, path: String): String =
        (listOf(rootName) + path.split('/').filter { it.isNotEmpty() }).joinToString(" › ")

    /** "From Library › Artist": where a trashed item was (the folder it was in). */
    fun trashFrom(rootName: String, path: String): String = "From " + place(rootName, parentOf(path))

    /**
     * The first link a browsing went through, after opening [path] ([link]: the entry opened is a link to a folder).
     * At or below the link it stays; a link opened from outside it becomes the new one; anywhere else there is none.
     * Nothing at or below it is written (client shapes §2.4); the server says the same with `via_link`.
     */
    fun linkAfter(linkAt: String?, path: String, link: Boolean): String? = when {
        linkAt != null && (path == linkAt || path.startsWith("$linkAt/")) -> linkAt
        link -> path
        else -> null
    }

    // ---- names ----------------------------------------------------------------------------------------------------

    /**
     * Characters a name may not hold (the server's rule, encore-files names.rs): control characters (C0, DEL, C1),
     * the line and paragraph separators, and the characters that turn text around (bidi embeddings, overrides and
     * isolates).
     */
    fun hidden(c: Char): Boolean =
        Character.isISOControl(c) || c == ' ' || c == ' ' || c in '‪'..'‮' || c in '⁦'..'⁩'

    /** What is wrong with a new name, in the server's words (null: it can be sent; the server still checks it). */
    fun nameProblem(name: String): String? = when {
        name.isBlank() -> "A name can't be empty"
        name.contains('/') || name.contains('\\') -> "A name can't contain / or \\"
        name == "." || name == ".." -> "That name isn't allowed"
        name.toByteArray(Charsets.UTF_8).size > 255 -> "That name is too long"
        name.any { hidden(it) } -> "A name can't contain invisible or control characters"
        else -> null
    }

    /**
     * One part of an uploaded file's name as the server takes it: a slash or backslash becomes "_", the characters
     * the server refuses ([hidden]) go, and so do the spaces around it.
     */
    fun uploadPart(part: String): String = part.replace('/', '_').replace('\\', '_').filterNot { hidden(it) }.trim()

    // ---- search ---------------------------------------------------------------------------------------------------

    /** The words of a search, lowercased (none when nothing was typed). */
    fun searchWords(q: String): List<String> = q.split(Regex("\\s+")).filter { it.isNotEmpty() }.map { it.lowercase() }

    /**
     * Whether [name] holds every word of [q] (lowercased, anywhere, in any order): the open folder's filter. A blank
     * [q] lets everything through. The server's search also drops accents; the filter only lowercases.
     */
    fun matches(name: String, q: String): Boolean {
        val n = name.lowercase()
        return searchWords(q).all { n.contains(it) }
    }

    /**
     * Where a search result goes when tapped: (the folder to open, it is a link, the name to pick there). A folder (or
     * a link to one) opens itself; a file opens its folder with it picked.
     */
    fun hitTarget(hit: FileHit): Triple<String, Boolean, String?> =
        if (hit.entry.isDir) Triple(hit.path, hit.entry.kind == "link", null) else Triple(hit.parent, false, hit.entry.name)

    /**
     * The search results' heading: "3 found in Library", "200+ found in Library …" when the server stopped early,
     * "Nothing in Library is called that".
     */
    fun foundLine(n: Int, truncated: Boolean, place: String): String = when {
        n == 0 && !truncated -> "Nothing in $place is called that"
        n == 0 -> "Nothing found yet in $place: the search stopped early, try more words"
        truncated -> "$n+ found in $place (the nearest first; type more to narrow it)"
        n == 1 -> "1 found in $place"
        else -> "$n found in $place"
    }

    // ---- sorting --------------------------------------------------------------------------------------------------

    /** The sorts the server knows, with their names. */
    val SORTS = listOf("name" to "Name", "size" to "Size", "mtime" to "Date modified")

    fun sortLabel(key: String): String = SORTS.firstOrNull { it.first == key }?.second ?: "Name"

    /** The order of a sort in words: "A–Z" / "Z–A", "smallest first" / "largest first", "oldest first" / "newest first". */
    fun sortOrderText(key: String, dir: String): String = when (key) {
        "size" -> if (dir == "asc") "smallest first" else "largest first"
        "mtime" -> if (dir == "asc") "oldest first" else "newest first"
        else -> if (dir == "asc") "A–Z" else "Z–A"
    }

    /** "Sort: Name ↑" (↑ ascending, ↓ descending). */
    fun sortButton(key: String, dir: String): String = "Sort: " + sortLabel(key) + if (dir == "desc") " ↓" else " ↑"

    /** The sort after choosing [key]: the same key turns the order round; another starts names A–Z, sizes and dates largest and newest first. */
    fun nextSort(sort: String, dir: String, key: String): Pair<String, String> =
        if (key == sort) key to (if (dir == "asc") "desc" else "asc") else key to (if (key == "name") "asc" else "desc")

    // ---- what happened --------------------------------------------------------------------------------------------

    /**
     * `{failed: [{path | folder | id, reason}]}` as one line: "" when nothing failed, "<name>: <reason>" for one,
     * "N weren't done. <name>: <reason>" for more (a path is named by its last part).
     */
    fun failureLine(answer: JsonElement?): String {
        val failed = (answer as? JsonObject)?.arr("failed").objectsOf()
        val first = failed.firstOrNull() ?: return ""
        val what = listOf("path", "folder", "id").map { first.str(it) }.firstOrNull { it.isNotEmpty() }?.substringAfterLast('/') ?: ""
        val why = first.str("reason")
        val one = if (what.isEmpty()) why else "$what: $why"
        return if (failed.size == 1) one else "${failed.size} weren't done. $one"
    }

    /**
     * What one upload answer (§4.3) says about the one file sent as [sent]: "skipped" (with the server's reason),
     * "renamed" (to the name it got, relative to the upload's folder) or "uploaded".
     */
    fun uploadOutcome(answer: JsonObject?, sent: String): UploadOutcome {
        val skipped = answer?.arr("skipped").objectsOf().firstOrNull()
        val renamed = answer?.arr("renamed").objectsOf().firstOrNull()
        return when {
            skipped != null -> UploadOutcome("skipped", sent, skipped.str("reason").ifEmpty { "Skipped" })
            renamed != null -> UploadOutcome("renamed", renamed.str("to").ifEmpty { sent }, "")
            else -> UploadOutcome("uploaded", sent, "")
        }
    }

    /**
     * The end of an upload batch: what the toast says, and its kind (info when stopped, ok when all went, warn when
     * some didn't: the first one's name and reason are in [firstError]).
     */
    fun uploadEnd(ok: Int, count: Int, stopped: Boolean, firstError: String): Pair<String, String> = when {
        stopped -> "Upload stopped after $ok of $count" to "info"
        ok == count -> (if (ok == 1) "Uploaded 1 file" else "Uploaded $ok files") to "ok"
        else -> "Uploaded $ok of $count. $firstError".trim() to "warn"
    }

    /** "Import 1 album" / "Import 3 albums". */
    fun importButton(n: Int): String = if (n == 1) "Import 1 album" else "Import $n albums"
}
