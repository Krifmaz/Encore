package app.encore.android.features.bugs

import app.encore.android.data.api.ApiException
import java.io.EOFException
import java.io.InterruptedIOException
import java.net.SocketException
import java.net.URLEncoder
import java.net.UnknownHostException
import javax.net.ssl.SSLException

/**
 * What bug reports need without Android: the kinds and statuses in the web's words and badge colours, the list's and
 * the export's query, the lines the list and a report show, and whether a picked picture goes to the server as it is.
 * The server is server/src/routes/bugreports.rs (or its 0.29 crate), the web page web-rs/src/pages/bugreports.rs.
 */
object BugRules {
    /** Files per report or per reply (the server's MAX_FILES). */
    const val MAX_FILES = 6
    /** Bytes per file (the server's MAX_FILE). */
    const val MAX_FILE_BYTES = 8L * 1024 * 1024
    const val MAX_TITLE = 200
    const val MAX_BODY = 20_000
    /** The server refuses an image wider or taller than this (MAX_SIDE) … */
    const val MAX_SIDE = 8192
    /** … or one that would decode to more than this (MAX_PIXEL_BYTES; counted here as 4 bytes a pixel). */
    const val MAX_PIXEL_BYTES = 128L * 1024 * 1024

    /** The three kinds, as the server stores them and as people read them. */
    val KINDS: List<Pair<String, String>> = listOf("bug" to "Bug", "not_bug" to "Not a bug", "howto" to "How do I…")

    /** The five statuses: key, label, badge colour (the web's badge classes). */
    val STATUSES: List<Triple<String, String, String>> = listOf(
        Triple("new", "New", "warn"),
        Triple("looking", "Looking into it", "info"),
        Triple("fixed", "Fixed", "ok"),
        Triple("wontfix", "Won't fix", ""),
        Triple("answered", "Answered", "accent"),
    )

    /** The web's words when a call never reached the server (its `post_form`). */
    const val NOT_CONNECTED = "Not connected to the server"

    /**
     * The words for a failed call: the server's (`detail`, an [ApiException]) or the phone's own, except that a call
     * that never reached the server or got no answer (no network, the name or the connection failing, a time-out)
     * says [NOT_CONNECTED] as the web does, not the network library's text.
     */
    fun errorWords(e: Throwable): String = if (unreachable(e)) NOT_CONNECTED else e.message ?: "Request failed"

    private fun unreachable(e: Throwable): Boolean {
        var c: Throwable? = e
        var depth = 0
        while (c != null && depth < 5) {
            if (c is ApiException) return false
            if (c is UnknownHostException || c is SocketException || c is InterruptedIOException || c is SSLException || c is EOFException) return true
            c = c.cause
            depth++
        }
        return false
    }

    /** The list's tabs: All, then the statuses; the tab says "Looking" (the badge keeps the long label) so six fit. */
    val TABS: List<Pair<String, String>> =
        listOf("all" to "All") + STATUSES.map { (k, l, _) -> k to if (k == "looking") "Looking" else l }

    fun kindLabel(kind: String): String = KINDS.firstOrNull { it.first == kind }?.second ?: "Bug"

    fun kindBadge(kind: String): String = when (kind) {
        "bug" -> "err"
        "howto" -> "info"
        else -> ""
    }

    fun statusLabel(status: String): String = STATUSES.firstOrNull { it.first == status }?.second ?: "New"

    fun statusBadge(status: String): String = STATUSES.firstOrNull { it.first == status }?.third ?: ""

    // ---- the list's filter ------------------------------------------------------------------------------------

    /** What the list shows: a status tab ("all" or a status), a kind ("" for every kind), the search, and Mine (admins). */
    data class Filter(val status: String = "all", val kind: String = "", val q: String = "", val mine: Boolean = false) {
        /** Something narrows the list (the empty list then says "Nothing matches these filters."). */
        val active: Boolean get() = (status != "all" && status.isNotEmpty()) || kind.isNotEmpty() || q.isNotBlank()
    }

    /** A kept filter made valid: an unknown status is "all", an unknown kind none, Mine only for admins. */
    fun normalize(f: Filter, admin: Boolean): Filter = Filter(
        status = if (STATUSES.any { it.first == f.status }) f.status else "all",
        kind = if (KINDS.any { it.first == f.kind }) f.kind else "",
        q = f.q,
        mine = admin && f.mine,
    )

    /** `status=` and `kind=`, only what is set: what the export takes from the list (the web's `params`). */
    fun params(f: Filter): List<String> {
        val p = ArrayList<String>()
        if (f.status != "all" && f.status.isNotEmpty()) p.add("status=" + enc(f.status))
        if (f.kind.isNotEmpty()) p.add("kind=" + enc(f.kind))
        return p
    }

    /** The query of GET /api/bug-reports: the filters, the search, and `mine=1` for an admin who asked for Mine. */
    fun listQuery(f: Filter, admin: Boolean): String {
        val p = ArrayList(params(f))
        val q = f.q.trim()
        if (q.isNotEmpty()) p.add("q=" + enc(q))
        if (admin && f.mine) p.add("mine=1")
        return if (p.isEmpty()) "" else "?" + p.joinToString("&")
    }

    /** The query of GET /api/bug-reports/export for the list: its status and kind, as the web's Export link. */
    fun exportQuery(f: Filter): String {
        val p = params(f)
        return if (p.isEmpty()) "" else "?" + p.joinToString("&")
    }

    /** The query of GET /api/bug-reports/export for one report ("Export this report"). */
    fun exportOneQuery(id: Long): String = "?ids=$id"

    /** The zip's name on the phone (built here, never taken from the server). */
    fun exportName(day: String, id: Long? = null): String =
        if (id != null) "encore-bug-report-$id-$day.zip" else "encore-bug-reports-$day.zip"

    // ---- the words the list and a report show ------------------------------------------------------------------

    /** "No replies", "1 reply", "3 replies", plus " · 1 file" / " · 2 files" when the report has files. */
    fun countWords(messages: Int, files: Int): String {
        val replies = when (messages) {
            0 -> "No replies"
            1 -> "1 reply"
            else -> "$messages replies"
        }
        return if (files > 0) "$replies · $files file${if (files == 1) "" else "s"}" else replies
    }

    /** "by ann · 18 Oct 2026, 14:20 · page Home · app 0.30.0 · EncoreAndroid/…": the line under a report's title. */
    fun metaLine(username: String, date: String, page: String, appVersion: String, device: String): String {
        val s = StringBuilder("by ").append(username).append(" · ").append(date)
        if (page.isNotEmpty()) s.append(" · page ").append(page)
        if (appVersion.isNotEmpty()) s.append(" · app ").append(appVersion)
        if (device.isNotEmpty()) s.append(" · ").append(device)
        return s.toString()
    }

    /** The line under the New report form: what goes along with the report. */
    fun sentAlong(page: String, version: String): String =
        "Sent along: " + (if (page.isEmpty()) "" else "the page you came from ($page), ") +
            "the app version (${version.ifEmpty { "unknown" }}) and this phone."

    // ---- pictures ---------------------------------------------------------------------------------------------

    /** The format a file's first bytes say, of those the server takes ("png", "jpg", "webp", "gif"); null otherwise. */
    fun sniffImage(head: ByteArray): String? {
        fun at(i: Int): Int = if (i < head.size) head[i].toInt() and 0xFF else -1
        fun ascii(from: Int, text: String): Boolean = text.indices.all { at(from + it) == text[it].code }
        return when {
            at(0) == 0x89 && ascii(1, "PNG") && at(4) == 0x0D && at(5) == 0x0A && at(6) == 0x1A && at(7) == 0x0A -> "png"
            at(0) == 0xFF && at(1) == 0xD8 && at(2) == 0xFF -> "jpg"
            ascii(0, "GIF87a") || ascii(0, "GIF89a") -> "gif"
            ascii(0, "RIFF") && ascii(8, "WEBP") -> "webp"
            else -> null
        }
    }

    fun mimeOf(ext: String): String = when (ext) {
        "png" -> "image/png"
        "gif" -> "image/gif"
        "webp" -> "image/webp"
        else -> "image/jpeg"
    }

    /**
     * Whether the server takes a picture as the phone has it: a format it reads, at most 8 MB, at most 8192 pixels a
     * side and at most 128 MiB decoded. Anything else is sent as a smaller JPEG (a HEIC photo, a 50 MP camera shot).
     * A size or a side that isn't known (0 or less) doesn't count as small.
     */
    fun sendsAsIs(format: String?, size: Long, width: Int, height: Int): Boolean =
        format != null && size in 1..MAX_FILE_BYTES && width in 1..MAX_SIDE && height in 1..MAX_SIDE &&
            width.toLong() * height.toLong() * 4L <= MAX_PIXEL_BYTES

    /**
     * The longest side, at most [maxSide], of a picture that is made smaller: the size (width, height) to scale
     * [width] × [height] to, keeping the shape; the same size when it already fits.
     */
    fun fitInside(width: Int, height: Int, maxSide: Int): Pair<Int, Int> {
        val longest = maxOf(width, height)
        if (width <= 0 || height <= 0 || longest <= maxSide) return width to height
        val scale = maxSide.toDouble() / longest
        return maxOf(1, Math.round(width * scale).toInt()) to maxOf(1, Math.round(height * scale).toInt())
    }

    /** The name a picture is sent under: the picked name's own part with the extension of what is sent. */
    fun fileName(name: String, ext: String): String {
        val base = name.substringAfterLast('/').substringAfterLast('\\').trim()
        val stem = base.substringBeforeLast('.', base).trim().trimStart('.')
        return (stem.ifEmpty { "image" }).take(100) + "." + ext
    }

    fun enc(s: String): String = URLEncoder.encode(s, "UTF-8").replace("+", "%20")
}
