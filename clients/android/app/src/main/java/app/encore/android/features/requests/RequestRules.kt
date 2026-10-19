package app.encore.android.features.requests

import app.encore.android.data.repo.CANCELLABLE_STATUSES
import app.encore.android.data.repo.LinkProbe
import app.encore.android.data.repo.LinkResult
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.RequestRow
import app.encore.android.data.repo.RetryFailedResult

/** One line of a request's log as the details sheet shows it: the source in plain words (or "") and what happened. */
data class LogEntry(val source: String, val text: String)

/**
 * What the details sheet of a request shows: the "what" line, the whole status message (never cut), the facts the
 * row's status line squeezes in, and every log line the worker wrote, one per source and attempt.
 */
data class RequestDetails(
    val title: String,
    val note: String,
    val message: String,
    val facts: List<String>,
    val log: List<LogEntry>,
)

/**
 * The Requests page's rules, as the web has them (web-rs/src/pages/common.rs `request_row`, `bind_request_actions`,
 * `clear_menu`, `retry_failed`; requests.rs; download.rs). Pure, so the unit tests check the words and the gates.
 */
object RequestRules {
    /**
     * The worker's source ids (log lines start with them) in plain words; anything else is shown as it is. `ytdlp`
     * fetches YouTube Music searches and every direct link (YouTube, SoundCloud, Bandcamp…), so it keeps the name
     * Settings gives it.
     */
    private val SOURCE_NAMES = mapOf(
        "slskd" to "Soulseek",
        "slskd_lossless" to "Soulseek",
        "torznab" to "Torrents & Usenet",
        "ytdlp" to "yt-dlp",
        "ytmusic" to "YouTube Music",
        "streamrip" to "streamrip",
        "qobuz" to "Qobuz",
        "tidal" to "Tidal",
        "deezer" to "Deezer",
        "link" to "",
        "start" to "",
    )

    fun sourceName(id: String): String = SOURCE_NAMES[id] ?: id

    // ---- the row ---------------------------------------------------------------------------------------------

    /** The status line after the kind badge: "3 attempts · via streamrip · <message> · by Caz · 8m ago". */
    fun statusLine(r: RequestRow, admin: Boolean, ago: String): String {
        val bits = ArrayList<String>()
        if (r.attempts > 1) bits.add("${r.attempts} attempts")
        if (r.source.isNotEmpty()) bits.add("via ${r.source}")
        if (r.message.isNotEmpty()) bits.add(r.message)
        if (admin && r.username.isNotEmpty()) bits.add("by ${r.username}")
        bits.add(ago)
        return bits.joinToString(" · ")
    }

    /** Approve / Deny: admins, on requests that wait for approval. */
    fun canApprove(r: RequestRow, admin: Boolean): Boolean = admin && r.status == "pending"

    /** Pick: albums and songs that are not in the library yet, for people allowed to pick (the phone hides it otherwise). */
    fun canPick(r: RequestRow, me: Me?): Boolean = (r.kind == "album" || r.kind == "track") && r.status != "completed" && me?.can("pick") == true

    /**
     * Retry (requests.rs `retry_request`): a playlist (its failed songs), or a failed or cancelled request. Members need
     * the `fix` permission; without it the server answers "Ask an admin to retry". (A cancelled request also needs
     * auto-approve, the member's own or the server-wide "Auto-approve all requests" that members can't see, so that
     * one is left to the server.)
     */
    fun canRetry(r: RequestRow, me: Me?): Boolean {
        if (r.kind == "playlist") return true
        if (r.status != "failed" && r.status != "cancelled") return false
        return me?.can("fix") == true
    }

    fun canCancel(r: RequestRow): Boolean = r.status in CANCELLABLE_STATUSES

    /** "Remove from list" and the files entry: requests that are not moving any more. */
    fun canRemove(r: RequestRow): Boolean = !r.isActive

    /** The admin's files entry of a row's ⋯ menu. */
    fun filesLabel(status: String): String = if (status == "completed") "Delete downloaded files" else "Delete leftover files"

    /** Its confirmation. */
    fun filesConfirm(status: String): String =
        if (status == "completed") "Permanently delete the songs this request downloaded from the library?" else "Delete the leftover/partial files of this download?"

    // ---- the details sheet -------------------------------------------------------------------------------------

    private val LOG_LINE = Regex("^([a-z][a-z_]*): (.*)$", RegexOption.DOT_MATCHES_ALL)

    /** "torznab: no matching release" → (Torrents & Usenet, no matching release); other lines stay whole. */
    fun logEntry(line: String): LogEntry {
        val m = LOG_LINE.matchEntire(line.trim()) ?: return LogEntry("", line.trim())
        val id = m.groupValues[1]
        return LogEntry(sourceName(id), m.groupValues[2].trim())
    }

    /** The worker's failure message: "All sources exhausted (a; b; c)", the last three sources' reasons. */
    private val EXHAUSTED = Regex("^All sources exhausted \\((.*)\\)$", RegexOption.DOT_MATCHES_ALL)

    /** "All sources exhausted (a; b; c)" → [a, b, c]; anything else → nothing. */
    fun exhaustedParts(message: String): List<String> {
        val m = EXHAUSTED.matchEntire(message.trim()) ?: return emptyList()
        return m.groupValues[1].split("; ").map { it.trim() }.filter { it.isNotEmpty() }
    }

    /** The log as the sheet lists it: the server's lines, or, from a server that sends none, the parts of the message. */
    fun logEntries(r: RequestRow): List<LogEntry> {
        val lines = r.log.filter { it.isNotBlank() }
        return (lines.ifEmpty { exhaustedParts(r.message) }).map { logEntry(it) }
    }

    /** The candidate being fetched: "From alice: Illuminate EP (FLAC, 4 files)". */
    fun candidateLine(r: RequestRow): String {
        val c = r.candidate ?: return ""
        if (c.username.isEmpty() && c.directory.isEmpty()) return ""
        val extra = listOfNotNull(
            c.format.uppercase().takeIf { it.isNotEmpty() },
            if (c.files > 0) "${c.files} file" + (if (c.files == 1) "" else "s") else null,
        ).joinToString(", ")
        val what = c.directory + if (extra.isNotEmpty()) " ($extra)" else ""
        return if (c.username.isNotEmpty()) "From ${c.username}: $what".trimEnd(':', ' ') else what
    }

    /** Everything the sheet shows for a request; `ago` is the formatted request time. */
    fun details(r: RequestRow, admin: Boolean, ago: String): RequestDetails {
        val facts = ArrayList<String>()
        if (r.attempts > 1) facts.add("Requested or retried ${r.attempts} times")
        if (r.source.isNotEmpty()) facts.add("via ${r.source}")
        if (r.picked) facts.add("Picked by hand")
        candidateLine(r).takeIf { it.isNotEmpty() }?.let { facts.add(it) }
        if (admin && r.username.isNotEmpty()) facts.add("by ${r.username}")
        facts.add(ago)
        return RequestDetails(r.what, r.whatNote, r.message, facts, logEntries(r))
    }

    // ---- bulk actions ----------------------------------------------------------------------------------------

    /** The toast after Retry failed (common.rs `retry_failed`) and its kind. */
    fun retryFailedToast(r: RetryFailedResult): Pair<String, String> {
        val msg = when {
            r.retried == 0 && r.skipped == 0 -> "No failed requests to retry"
            r.skipped == 0 -> "Retrying ${r.retried} request" + if (r.retried == 1) "" else "s"
            else -> "Retrying ${r.retried} · ${r.skipped} couldn't be retried" + if (r.why.isNotEmpty()) " (${r.why})" else ""
        }
        return msg to if (r.skipped > 0) "warn" else "ok"
    }

    /** The toast after a Clear (common.rs `clear_menu`). */
    fun clearToast(removed: Int, deleted: Int, files: Boolean): String = "$removed cleared" + if (files) " · $deleted file(s) deleted" else ""

    // ---- the link box (the web's Downloader, download.rs) -----------------------------------------------------

    /** Links that only describe songs (no downloadable audio): matched song by song, never "Exactly this link". */
    private val METADATA_ONLY = Regex(
        "open\\.spotify\\.com|spotify\\.link|^spotify:|music\\.apple\\.com|music\\.amazon\\.|amazon\\.[a-z.]+/music/player/|amzn\\.to/|listenbrainz\\.org",
        RegexOption.IGNORE_CASE,
    )
    private val STREAMRIP = Regex("qobuz\\.com|tidal\\.com|deezer\\.com|deezer\\.page\\.link", RegexOption.IGNORE_CASE)

    fun metadataOnly(url: String): Boolean = METADATA_ONLY.containsMatchIn(url.trim())

    /** Preview of these links opens "Import as playlist" instead (yt-dlp can't read them). */
    fun previewImports(url: String): Boolean = metadataOnly(url) || STREAMRIP.containsMatchIn(url.trim())

    /** "If a song is on an album:" choices, as the web's tabs: (album_mode, label). */
    val ALBUM_MODES = listOf("song" to "Just the song", "album" to "Get the whole album", "" to "Admin default")

    /** The toast after Download (download.rs): "Downloading", or "3 song(s) requested, 1 already requested – whole albums will be fetched". */
    fun linkToast(r: LinkResult, albumMode: String): String {
        if (r.mode == "direct") return "Downloading"
        return "${r.requested} song(s) requested" +
            (if (r.alreadyRequested > 0) ", ${r.alreadyRequested} already requested" else "") +
            (if (albumMode == "album") " – whole albums will be fetched" else "")
    }

    /** The line under a Preview's title: "Youtube · Some Channel · 3:41" or "… · 12 items". */
    fun probeLine(p: LinkProbe): String = listOf(
        p.extractor,
        p.uploader,
        if (p.isPlaylist) "${p.count} items" else if (p.duration > 0) duration(p.duration) else "",
    ).filter { it.isNotEmpty() }.joinToString(" · ")

    /** A Soulseek transfer's state badge kind on Activity (activity.rs `slskd_row`). */
    fun transferKind(state: String): String = when {
        state.contains("Succeeded") -> "ok"
        listOf("Errored", "Rejected", "TimedOut", "Cancelled").any { state.contains(it) } -> "err"
        else -> "info"
    }

    /** The web's `fmt.dur`: m:ss, or h:mm:ss from an hour. */
    fun duration(seconds: Double): String {
        val s = Math.round(seconds.coerceAtLeast(0.0))
        val h = s / 3600
        val m = (s % 3600) / 60
        val sec = s % 60
        return if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%d:%02d".format(m, sec)
    }
}
