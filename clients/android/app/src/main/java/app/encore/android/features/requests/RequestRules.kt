package app.encore.android.features.requests

import app.encore.android.data.repo.CANCELLABLE_STATUSES
import app.encore.android.data.repo.ClearWaitingResult
import app.encore.android.data.repo.DownloadAllNowResult
import app.encore.android.data.repo.DownloadNowResult
import app.encore.android.data.repo.LinkProbe
import app.encore.android.data.repo.LinkResult
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.RejectedDownload
import app.encore.android.data.repo.RequestCandidate
import app.encore.android.data.repo.RequestError
import app.encore.android.data.repo.RequestOutput
import app.encore.android.data.repo.RequestRow
import app.encore.android.data.repo.RequestUser
import app.encore.android.data.repo.RetryFailedResult
import app.encore.android.data.repo.SlskTransfer
import app.encore.android.data.repo.StorageHealth
import app.encore.android.data.repo.statusLabel

/**
 * Where a request's title goes in its details sheet (the owner on Oct 23: "from the download i should be able to click
 * on the artist song or album or play it if its downloaded"): the library album once it is there, else the
 * catalogue's album page for an album asked for by its release group, else a Discover search for it.
 */
sealed interface RequestTarget {
    data class LibraryAlbum(val id: String) : RequestTarget
    data class CatalogAlbum(val rgid: String) : RequestTarget
    data class Search(val query: String, val type: String) : RequestTarget
}

/** One line of a request's log as the details sheet shows it: the source in plain words (or "") and what happened. */
data class LogEntry(val source: String, val text: String)

/**
 * Keep anyway / Download anyway (R2, the owner on Oct 19: "i cant overide to have it download it anyways"): what a
 * request offers for a download the audio check turned away (common.rs `Anyway`).
 * - [action]: the route after `/api/requests/{id}/`: `keep-anyway` (the kept files are imported without the check) or
 *   `download-anyway` (the files are gone: the same source and link are asked for again without the check);
 * - [label] the button, [help] what it does (the web's title on it), [done] the toast once the server took it;
 * - [rejected]: which rejected download (`{rejected: id}`, 0: the server's newest), so the server acts on the one shown;
 * - [text]: why the check turned it away, in full ("The audio sounds like Diase – Sun, not Moonlight Shadow").
 */
data class Anyway(val action: String, val label: String, val help: String, val done: String, val rejected: Long, val text: String) {
    val keep: Boolean get() = action == "keep-anyway"
}

/** One download the audio check turned away, as the details sheet lists it: why, where from, the check's words, when, its files. */
data class RejectedLine(val text: String, val from: String, val check: String, val at: String, val files: String)

/**
 * What the details sheet of a request shows: the "what" line, the whole status message (never cut), the facts the
 * row's status line squeezes in, and every log line the worker wrote, one per source and attempt. A waiting request
 * also has what it waits for ([status], "" otherwise); a failed file step its sentence ([error]) and its step,
 * operation, path and errno ([errorFacts]); a source's own output its head ("streamrip output", "<link> · failed ·
 * <date>") and text; a download the audio check turned away, offered again, its reason and source ([anyway],
 * [anywayFrom]); and every download the check turned away ([rejected]).
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
    /** A download the audio check turned away and the row offers again: why, in full, and where it came from. */
    val anyway: String = "",
    val anywayFrom: String = "",
    /** Every download the audio check turned away, oldest first. */
    val rejected: List<RejectedLine> = emptyList(),
)

/**
 * The Requests page's rules, as the web has them (web-rs/src/pages/common.rs `request_row`, `bind_request_actions`,
 * `clear_menu`, `retry_failed`; requests.rs; download.rs). Pure, so the unit tests check the words and the gates.
 */
object RequestRules {
    /** The request's title as a link ([RequestTarget]); null for an artist (its name is the link), a link or a playlist. */
    fun titleTarget(r: RequestRow): RequestTarget? {
        if (r.kind != "album" && r.kind != "track") return null
        if (r.libraryAlbumId.isNotEmpty()) return RequestTarget.LibraryAlbum(r.libraryAlbumId)
        if (r.kind == "album" && r.mbid.isNotEmpty()) return RequestTarget.CatalogAlbum(r.mbid)
        val title = r.what
        if (title.isEmpty()) return null
        return RequestTarget.Search(listOf(r.artist, title).filter { it.isNotEmpty() }.joinToString(" "), r.kind)
    }

    /** The artist the details sheet links to ("" for a link or a playlist, which name none). */
    fun artistTarget(r: RequestRow): String = if (r.kind == "url" || r.kind == "playlist") "" else r.artist

    /** Play in the details sheet: a finished album or song request whose music is in the library. */
    fun canPlay(r: RequestRow): Boolean =
        r.status == "completed" && (r.libraryTrackId > 0L || (r.kind == "album" && r.libraryAlbumId.isNotEmpty()))

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

    /**
     * The status line after the kind badge: "3 attempts · via streamrip · <message> · by Caz · 8m ago". A hand pick the
     * audio check turned away has the reason as its message: it shows once, in full, on the row's own line
     * ([anywayLine]), not here as well.
     */
    fun statusLine(r: RequestRow, admin: Boolean, ago: String): String {
        val bits = ArrayList<String>()
        if (r.attempts > 1) bits.add("${r.attempts} attempts")
        if (r.source.isNotEmpty()) bits.add("via ${r.source}")
        if (r.message.isNotEmpty() && r.message.trim() != anywayLine(r)) bits.add(r.message)
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

    /**
     * An import that hasn't moved for 15 minutes (or whose task is gone: the list's `import_stalled`): the row offers
     * Import again, which stops it and imports the kept files at once (common.rs `stalled_import`).
     */
    fun stalledImport(r: RequestRow): Boolean = r.status == "importing" && r.importStalled

    /** Why a stalled import offers Import again (the web's and the desktop's tooltip; a line under the row here). */
    const val STALLED_IMPORT = "The import hasn't moved for 15 minutes"

    /** The admin's User list on the Requests page: its first entry (everyone's requests). */
    const val ALL_USERS = "All users"

    /** "Caz (612)": a person in the User list with their requests. */
    fun userLabel(name: String, count: Int): String = "$name ($count)"

    /**
     * The User list (requests.rs `user_options`): (user id, label) – 0 and "All users" first, then each person who has
     * requests with their count; the person [picked] (id to name) stays in it with 0 once their requests are gone, so
     * the list doesn't jump to everyone's.
     */
    fun userOptions(users: List<RequestUser>, picked: Pair<Long, String>?): List<Pair<Long, String>> {
        val out = mutableListOf(0L to ALL_USERS)
        for (u in users) if (u.id > 0L) out += u.id to userLabel(u.username, u.count)
        if (picked != null && picked.first > 0L && users.none { it.id == picked.first }) out += picked.first to userLabel(picked.second, 0)
        return out
    }

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
        "waiting_disk" -> "the downloads folder can't be written now"
        "plugin_install" -> "its source's plugin is still being installed"
        else -> ""
    }

    /** The status of a request waiting for a downloads folder (`wait_reason` "waiting_disk", common.rs `WAITING_DISK`). */
    const val WAITING_DISK = "Waiting for the downloads folder"

    /** A row's badge words (common.rs `request_badge`): its status, and for a request waiting for a downloads folder that it is. */
    fun badgeLabel(r: RequestRow): String =
        if (r.status == "waiting" && r.waitReason == "waiting_disk") WAITING_DISK else statusLabel(r.status)

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

    /**
     * The banner's sentence for the download folders (`downloads`, servers since 0.33): for admins, while a folder
     * downloads are saved in can't be written; null otherwise and from an older server.
     */
    fun downloadsBanner(s: StorageHealth?, admin: Boolean): String? =
        s?.downloads?.takeIf { admin && it.failing && it.text.isNotEmpty() }?.text

    /** The whole banner over a request list: imports paused, a downloads folder that fails, or both; null when neither. */
    fun banner(s: StorageHealth?, admin: Boolean): String? =
        listOfNotNull(storageBanner(s, admin), downloadsBanner(s, admin)).takeIf { it.isNotEmpty() }?.joinToString("\n")

    /** Imports are paused or a downloads folder fails: the banner asks again sooner ([pollMillis]). */
    fun failing(s: StorageHealth?): Boolean = s != null && (s.paused || s.downloads?.failing == true)

    /**
     * How long the banner waits before it asks again: 5 s while imports are paused (an import may resume them any
     * moment), 15 s while only a downloads folder fails (the server probes it once a minute; the web's notice asks
     * as often), else 30 s.
     */
    fun pollMillis(s: StorageHealth?): Long = when {
        s?.paused == true -> 5_000
        s?.downloads?.failing == true -> 15_000
        else -> 30_000
    }

    /**
     * The toast after Check again and its kind, from the answer and the state before it: "Imports resumed" only
     * when imports were paused; a downloads folder that can be written again says that (as the web's banner); a
     * check that still runs (a folder that doesn't answer) says so.
     */
    fun checkToast(s: StorageHealth, before: StorageHealth?): Pair<String, String> = when {
        s.paused -> "Imports are still paused" to "warn"
        s.downloads?.failing == true -> "A downloads folder still can't be written" to "warn"
        s.downloads?.checking == true -> "Still checking the downloads folders" to "warn"
        before?.paused == true -> "Imports resumed" to "ok"
        else -> "The downloads folders can be written" to "ok"
    }

    // ---- Download now (Oct 21: "for the que can i get a download now button") ---------------------------------------

    /**
     * Download now (common.rs `request_row`): only where the server offers it (`can_download_now`). It never breaks a
     * source's limit: the request goes first in its source's line, a wait is tried now, a Soulseek download queued at
     * a user switches to one with a free upload slot, or a torrent with no progress goes to the next source. An older
     * server sends no such field: no button.
     */
    fun canDownloadNow(r: RequestRow): Boolean = r.canDownloadNow == true

    /** The server has Download now (its rows carry `can_download_now`): admins get Download all now beside Retry failed. */
    fun hasDownloadNow(rows: List<RequestRow>): Boolean = rows.any { it.canDownloadNow != null }

    /**
     * What Download now does on this row, in the server's words ("Go first in line for Deezer"): the web's and the
     * desktop's tooltip, the details sheet's line under its buttons (the phone has no tooltips). "" when not offered.
     */
    fun downloadNowHelp(r: RequestRow): String = if (canDownloadNow(r)) r.downloadNow?.title?.trim().orEmpty() else ""

    /** What the toast says when the server answered a press without words. */
    const val STARTING_NOW = "Starting now"

    /**
     * The toast after Download now and its kind: the server's words ("Next in line for Deezer (1 download at a
     * time)"), "ok" when something changed or started, "warn" when nothing could be sooner ("Soulseek asked Encore to
     * wait – next attempt at 14:20"). Null when a press that changed nothing came back without words.
     */
    fun downloadNowToast(res: DownloadNowResult): Pair<String, String>? {
        val text = res.text.trim().ifEmpty { if (res.ok) STARTING_NOW else return null }
        return text to if (res.ok) "ok" else "warn"
    }

    /** Download all now with nothing to do (the server's words). */
    const val NOTHING_WAITING = "Nothing is waiting – every request is downloading or done"

    /** The toast after Download all now and its kind: the server's words; "warn" when nothing was waiting. */
    fun downloadAllNowToast(res: DownloadAllNowResult): Pair<String, String> =
        res.text.trim().ifEmpty { if (res.ok) STARTING_NOW else NOTHING_WAITING } to if (res.ok) "ok" else "warn"

    /**
     * The Download now lines of the details and of the copied text (common.rs `request_text`): the request's place in
     * its source's line, the Soulseek user's upload queue it waits in, a torrent or Usenet download with no progress,
     * and who asked to download it now. [date] formats a unix time.
     */
    fun downloadNowLines(r: RequestRow, date: (Double) -> String): List<String> {
        val out = ArrayList<String>()
        r.line?.takeIf { it.text.isNotBlank() }?.let { l ->
            out.add("In line: ${l.text.trim()}" + if (l.limitText.isNotBlank()) " (${l.limitText.trim()})" else "")
        }
        r.remoteQueue?.takeIf { it.text.isNotBlank() }?.let { q ->
            out.add("Soulseek queue: ${q.text.trim()}" + if (q.since > 0) " (since ${date(q.since)})" else "")
        }
        if (r.stalledSince > 0) out.add("No progress since: ${date(r.stalledSince)}")
        r.pressed?.takeIf { it.by.isNotBlank() || it.at > 0 }?.let { p ->
            out.add("Download now: asked" + (if (p.by.isNotBlank()) " by ${p.by}" else "") + (if (p.at > 0) " at ${date(p.at)}" else ""))
        }
        return out
    }

    // ---- a download the audio check turned away (R2) -------------------------------------------------------------

    /**
     * The offer of a request row (`anyway` of GET /api/requests, common.rs `anyway`), or null: an older server, no
     * offer, or an action this phone doesn't know. The server offers it only where it can be taken (a failed request,
     * or one still looking); [mayFix] says who sees the button. The text is the server's sentence, else the rejected
     * download's own.
     */
    fun anyway(r: RequestRow): Anyway? {
        val a = r.anyway ?: return null
        val (action, label, help, done) = when (a.action) {
            "keep" -> listOf("keep-anyway", "Keep anyway", "Import the download the audio check turned away, without the check", "It will be imported without the audio check")
            "download" -> listOf("download-anyway", "Download anyway", "Download it again from the same place, without the audio check", "Downloading it again without the audio check")
            else -> return null
        }
        val text = a.text.ifBlank {
            r.rejected.firstOrNull { it.id == a.id }?.let { e -> e.text.ifBlank { e.reason } }.orEmpty()
        }
        return Anyway(action, label, help, done, a.id, text)
    }

    /**
     * Keep anyway and Download anyway are taken as Retry is: by admins, and by the request's owner with the `fix`
     * permission (members see only their own requests). Others would only get "Ask an admin to keep it anyway", so
     * they see the reason and no button.
     */
    fun mayFix(me: Me?): Boolean = me?.can("fix") == true

    /** The row's button: an offer, for someone who may take it. */
    fun canTakeAnyway(r: RequestRow, me: Me?): Boolean = anyway(r) != null && mayFix(me)

    /** The reason under the row's status line, in full (it wraps, never cut): "" when nothing is offered. */
    fun anywayLine(r: RequestRow): String = anyway(r)?.text?.trim().orEmpty()

    /** "Audio check: off for this request" while it runs (or ran and failed) without the check. */
    const val SKIP_CHECK = "Audio check: off for this request"

    /**
     * Where a rejected download came from, in one line (common.rs `rejected_from`): "Deezer · via streamrip ·
     * https://www.deezer.com/track/1 · 01. Moonlight Shadow.flac"; an album names its first file "and 9 more".
     */
    fun rejectedFrom(e: RejectedDownload): String {
        val files = if (e.count > 1 && e.file.isNotBlank()) "${e.file} and ${e.count - 1} more" else e.file
        return listOf(e.via, e.detail, e.url, files).filter { it.isNotBlank() }.joinToString(" · ")
    }

    /** Its files: "kept in <folder>" (admins), "kept on the server" (everyone else), or "gone". */
    fun rejectedFiles(e: RejectedDownload): String = when {
        !e.hasFile -> "gone"
        e.path.isNotBlank() -> "kept in ${e.path}"
        else -> "kept on the server"
    }

    /** Why it was turned away, in full; the check's own words when the server sent no sentence. */
    fun rejectedText(e: RejectedDownload): String = e.text.ifBlank { e.reason }.ifBlank { "The audio sounds like another song" }

    /** Every download the audio check turned away, oldest first, as the details sheet lists them; [date] formats `at`. */
    fun rejectedLines(r: RequestRow, date: (Double) -> String): List<RejectedLine> = r.rejected.map { e ->
        RejectedLine(
            text = rejectedText(e),
            from = rejectedFrom(e),
            check = if (e.text.isNotBlank() && e.reason.isNotBlank()) e.reason else "",
            at = if (e.at > 0) date(e.at) else "",
            files = rejectedFiles(e),
        )
    }

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
        out.addAll(downloadNowLines(r, date))
        add("Attempts", r.attempts.toString())
        add("Source", r.source)
        if (r.picked) add("Picked by hand", "yes")
        r.candidate?.let { c -> candidateWhat(c)?.let { add("Downloading", if (c.username.isNotEmpty()) "from ${c.username}: $it".trimEnd(':', ' ') else it) } }
        r.error?.let { e ->
            add("Error", errorText(e))
            out.addAll(errorFacts(e))
        }
        keptLine(r)?.let { out.add(it) }
        if (r.skipCheck) out.add(SKIP_CHECK)
        // the downloads the audio check turned away (R2), oldest first: why in full, where from, and whether the files are kept
        val rejected = rejectedLines(r, date)
        if (rejected.isNotEmpty()) {
            out.add("")
            out.add("Turned away by the audio check (oldest first):")
        }
        for (e in rejected) {
            out.add(e.text)
            fun sub(k: String, v: String) { if (v.isNotBlank()) out.add("  $k: $v") }
            sub("From", e.from)
            sub("Check", e.check)
            sub("When", e.at)
            sub("Files", e.files)
        }
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
        facts.addAll(downloadNowLines(r, date))
        keptLine(r)?.let { facts.add(it) }
        if (r.skipCheck) facts.add(SKIP_CHECK)
        if (admin && r.username.isNotEmpty()) facts.add("by ${r.username}")
        facts.add(ago)
        val o = r.output?.takeIf { it.text.isNotEmpty() }
        val offer = anyway(r)
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
            anyway = offer?.text?.trim().orEmpty(),
            anywayFrom = offer?.let { a -> r.rejected.firstOrNull { it.id == a.rejected }?.let { rejectedFrom(it) } }.orEmpty(),
            rejected = rejectedLines(r, date),
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

    /** Clear waiting's question for [count] requests (the server's dry run counted them), as the web asks it. */
    fun clearWaitingQuestion(count: Int): String =
        "Cancel $count waiting request${if (count == 1) "" else "s"}? Downloads and imports already running keep going."

    /** Clear waiting's toast: the server's sentence, or the same words when an answer has none. */
    fun clearWaitingToast(r: ClearWaitingResult): String = r.text.trim().ifEmpty {
        if (r.count <= 0) "Nothing is waiting" else "Cancelled ${r.count} waiting request${if (r.count == 1) "" else "s"}"
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
        listOf("Errored", "Rejected", "TimedOut", "Failed", "Aborted").any { state.contains(it) } -> "err"
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
