package app.encore.android.features.requests

import app.encore.android.data.repo.CANCELLABLE_STATUSES
import app.encore.android.data.repo.LinkProbe
import app.encore.android.data.repo.LinkResult
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.RequestCandidate
import app.encore.android.data.repo.RequestError
import app.encore.android.data.repo.RequestOutput
import app.encore.android.data.repo.RequestRow
import app.encore.android.data.repo.RetryFailedResult
import app.encore.android.data.repo.SlskTransfer
import app.encore.android.data.repo.StorageHealth
import app.encore.android.data.repo.statusLabel

/** One line of a request's log as the details sheet shows it: the source in plain words (or "") and what happened. */
data class LogEntry(val source: String, val text: String)

/**
 * What the details sheet of a request shows: the "what" line, the whole status message (never cut), the facts the
 * row's status line squeezes in, and every log line the worker wrote, one per source and attempt. A waiting request
 * also has what it waits for ([status], "" otherwise); a failed file step its sentence ([error]) and its step,
 * operation, path and errno ([errorFacts]); a source's own output its head ("streamrip output", "<link> · failed ·
 * <date>") and text.
 */
data class RequestDetails(
    val title: String,
    val note: String,
    val message: String,
    val facts: List<String>,
    val log: List<LogEntry>,
    val status: String = "",
    val error: String = "",
    val errorFacts: List<String> = emptyList(),
    val outputTitle: String = "",
    val outputHead: String = "",
    val output: String = "",
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
        // a failed or waiting import whose files are kept gets Import again in Retry's place
        if (canImportAgain(r)) return false
        if (r.kind == "playlist") return true
        if (r.status != "failed" && r.status != "cancelled") return false
        return me?.can("fix") == true
    }

    fun canCancel(r: RequestRow): Boolean = r.status in CANCELLABLE_STATUSES

    /**
     * Import again (common.rs `can_import_again`): the downloaded files of a failed or waiting import are kept (or the
     * files a link's request could not import), so the row offers Import again where it offered Retry – nothing is
     * downloaded again. A Soulseek wait has no files.
     */
    fun canImportAgain(r: RequestRow): Boolean = r.hasFiles && (r.status == "failed" || r.status == "waiting" || r.status == "completed")

    /** What Cancel and Remove from list say first when a request keeps downloaded files (common.rs `KEPT_FILES_LOST`). */
    const val KEPT_FILES_LOST = "Its downloaded files have not been imported yet and can't be imported after this."

    /** What Import again does (the web's title on the button). */
    const val IMPORT_AGAIN_HELP = "Import the downloaded files again – nothing is downloaded again"

    /** "Remove from list" and the files entry: requests that are not moving any more. */
    fun canRemove(r: RequestRow): Boolean = !r.isActive

    /** The admin's files entry of a row's ⋯ menu; [kept]: the request keeps downloaded files for Import again. */
    fun filesLabel(status: String, kept: Boolean = false): String =
        if (status == "completed" || kept) "Delete downloaded files" else "Delete leftover files"

    /** Its confirmation. */
    fun filesConfirm(status: String, kept: Boolean = false): String = when {
        status == "completed" -> "Permanently delete the songs this request downloaded from the library?"
        kept -> "Delete the downloaded files kept for this request? They can't be imported afterwards."
        else -> "Delete the leftover/partial files of this download?"
    }

    // ---- waiting requests and the storage banner (R1) ------------------------------------------------------------

    /** What a waiting request waits for (`wait_reason`), in the web's words (common.rs `wait_words`). */
    fun waitWords(reason: String): String = when (reason) {
        "soulseek" -> "Soulseek isn't connected yet"
        "storage_import" -> "the downloaded files are kept and the import waits for the storage"
        "storage_download" -> "the download waits for the staging folder"
        else -> ""
    }

    /** The status in words, with what a waiting request waits for: "Waiting – the downloaded files are kept and …". */
    fun statusText(r: RequestRow): String {
        val label = statusLabel(r.status)
        val why = if (r.status == "waiting") waitWords(r.waitReason) else ""
        return if (why.isEmpty()) label else "$label – $why"
    }

    /** What members read while imports are paused (the server's own words for them). */
    const val PLAIN_BANNER = "Imports are paused because of a storage problem on the server. Your downloads are kept and will be added when it is fixed."

    /**
     * The storage banner (common.rs `storage_banner_html`): null while imports go on or from a server without the
     * check. Admins read the folder, the operation and the error (the server's sentence, with its clock), everyone
     * else the plain words.
     */
    fun storageBanner(s: StorageHealth?, admin: Boolean): String? {
        if (s == null || !s.paused) return null
        return when {
            s.text.isNotEmpty() -> s.text
            admin && s.path.isNotEmpty() -> "Imports are paused: ${s.path} is not writable – " + (if (s.op.isNotEmpty()) "${s.op}: " else "") + "${s.error}."
            else -> PLAIN_BANNER
        }
    }

    /** The toast after Check again and its kind. */
    fun checkToast(s: StorageHealth): Pair<String, String> = if (s.paused) "Imports are still paused" to "warn" else "Imports resumed" to "ok"

    // ---- the text Copy takes (a bug report's) ------------------------------------------------------------------

    /** What a request asked for, in one line: "Illuminate EP – Duumu", "Duumu – full discography", the link (common.rs `request_what`). */
    fun requestWhat(r: RequestRow): String {
        val title = when (r.kind) {
            "album" -> r.title.ifEmpty { r.album }
            "url" -> r.title.ifEmpty { r.url }
            "artist" -> ""
            else -> r.title
        }
        return when {
            title.isNotEmpty() && r.artist.isNotEmpty() -> "$title – ${r.artist}"
            title.isNotEmpty() -> title
            r.artist.isNotEmpty() && r.kind == "artist" -> "${r.artist} – full discography"
            else -> r.artist
        }
    }

    /** The sentence of a failed step: the server's whole text, else the system's words. */
    fun errorText(e: RequestError): String = e.text.ifEmpty { e.error }

    /** The failed step under its sentence: "Step: import", "Operation: mkdir", "Path: …", "Errno: 5"… (what is known). */
    fun errorFacts(e: RequestError): List<String> {
        val out = ArrayList<String>()
        fun add(k: String, v: String) { if (v.isNotBlank()) out.add("$k: $v") }
        add("Step", e.step)
        add("Operation", e.op)
        add("Path", e.path)
        add("Other path", e.to)
        add("Errno", e.errno)
        if (e.text.isNotEmpty()) add("System error", e.error)
        if (e.storage) add("Storage problem", "yes – not the source")
        return out
    }

    /** Where the kept files are: "Downloaded files kept in: …" (null when nothing is kept). */
    fun keptLine(r: RequestRow): String? =
        // (other users than admins get no server folder)
        if (r.hasFiles) "Downloaded files kept in: " + r.keptPath.ifEmpty { "the server" } else null

    /** "streamrip output" (the source that wrote it). */
    fun outputTitle(o: RequestOutput): String = o.source.ifEmpty { "streamrip" } + " output"

    /** "<link> · produced files · <date>" or "… · failed · …" (what is known). */
    fun outputHead(o: RequestOutput, date: (Double) -> String): String = listOf(
        o.url,
        if (o.ok) "produced files" else "failed",
        if (o.at > 0) date(o.at) else "",
    ).filter { it.isNotEmpty() }.joinToString(" · ")

    /**
     * A request in plain text, for Copy (common.rs `request_text`, the same lines as the web's): what a bug report
     * needs – title, artist, status and message, every attempt in the log with its source and whole error, the
     * failed step with its operation, path and errno, streamrip's output, the request id and the server's version.
     * [date] formats a unix time.
     */
    fun requestText(r: RequestRow, version: String, date: (Double) -> String): String {
        val out = arrayListOf("Encore request #${r.id}")
        fun add(k: String, v: String) { if (v.isNotBlank()) out.add("$k: $v") }
        add("Title", if (r.kind == "album") r.title.ifEmpty { r.album } else r.title)
        add("Artist", r.artist)
        if (r.kind == "track" && r.album != r.title) add("Album", r.album)
        add("Link", r.url)
        add("Kind", r.kind)
        add("Status", statusText(r))
        add("Message", r.message)
        if (r.status == "waiting" && r.nextTryAt > 0) add("Next try", date(r.nextTryAt))
        add("Attempts", r.attempts.toString())
        add("Source", r.source)
        if (r.picked) add("Picked by hand", "yes")
        r.candidate?.let { c -> candidateWhat(c)?.let { add("Downloading", if (c.username.isNotEmpty()) "from ${c.username}: $it".trimEnd(':', ' ') else it) } }
        r.error?.let { e ->
            add("Error", errorText(e))
            out.addAll(errorFacts(e))
        }
        keptLine(r)?.let { out.add(it) }
        val log = r.log.filter { it.isNotBlank() }
        if (log.isNotEmpty()) {
            out.add("")
            out.add("Log (every attempt, oldest first):")
            out.addAll(log)
        }
        val o = r.output
        if (o != null && o.text.isNotEmpty()) {
            out.add("")
            out.add("${outputTitle(o)} (${outputHead(o, date)}):")
            out.add(o.text.trimEnd())
        }
        out.add("")
        add("Requested by", r.username)
        if (r.createdAt > 0) add("Requested", date(r.createdAt))
        if (r.updatedAt > 0) add("Updated", date(r.updatedAt))
        add("Request id", r.id.toString())
        add("Server version", version.ifEmpty { "unknown" })
        return out.joinToString("\n")
    }

    /** A line of the server log as text (activity.rs `log_text`): "warning · <date>" and the message under it. */
    fun logText(level: String, message: String, at: String): String = "$level · $at\n$message"

    /** A failed Soulseek transfer as text: the user, the file and slskd's reason. */
    fun transferErrorText(f: SlskTransfer): String = listOf(f.username, f.filename, f.error).joinToString("\n")

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

    /** The candidate's folder and what it holds: "Illuminate EP (FLAC, 4 files)"; null when there is none. */
    private fun candidateWhat(c: RequestCandidate): String? {
        if (c.username.isEmpty() && c.directory.isEmpty()) return null
        val extra = listOfNotNull(
            c.format.uppercase().takeIf { it.isNotEmpty() },
            if (c.files > 0) "${c.files} file" + (if (c.files == 1) "" else "s") else null,
        ).joinToString(", ")
        return c.directory + if (extra.isNotEmpty()) " ($extra)" else ""
    }

    /** The candidate being fetched: "From alice: Illuminate EP (FLAC, 4 files)". */
    fun candidateLine(r: RequestRow): String {
        val c = r.candidate ?: return ""
        val what = candidateWhat(c) ?: return ""
        return if (c.username.isNotEmpty()) "From ${c.username}: $what".trimEnd(':', ' ') else what
    }

    /**
     * Everything the sheet shows for a request; `ago` is the formatted request time, [date] formats the next try and
     * the output's time.
     */
    fun details(r: RequestRow, admin: Boolean, ago: String, date: (Double) -> String = { "" }): RequestDetails {
        val facts = ArrayList<String>()
        if (r.attempts > 1) facts.add("Requested or retried ${r.attempts} times")
        if (r.source.isNotEmpty()) facts.add("via ${r.source}")
        if (r.picked) facts.add("Picked by hand")
        candidateLine(r).takeIf { it.isNotEmpty() }?.let { facts.add(it) }
        if (r.status == "waiting" && r.nextTryAt > 0) facts.add("Next try: " + date(r.nextTryAt))
        keptLine(r)?.let { facts.add(it) }
        if (admin && r.username.isNotEmpty()) facts.add("by ${r.username}")
        facts.add(ago)
        val o = r.output?.takeIf { it.text.isNotEmpty() }
        return RequestDetails(
            title = r.what,
            note = r.whatNote,
            message = r.message,
            facts = facts,
            log = logEntries(r),
            status = if (r.status == "waiting" && r.waitReason.isNotEmpty()) statusText(r) else "",
            error = r.error?.let { errorText(it) }.orEmpty(),
            errorFacts = r.error?.let { errorFacts(it) }.orEmpty(),
            outputTitle = o?.let { outputTitle(it) }.orEmpty(),
            outputHead = o?.let { outputHead(it, date) }.orEmpty(),
            output = o?.text?.trimEnd().orEmpty(),
        )
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
