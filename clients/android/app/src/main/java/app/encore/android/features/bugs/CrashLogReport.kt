package app.encore.android.features.bugs

import android.net.Uri
import app.encore.android.AppGraph
import app.encore.android.BuildConfig
import app.encore.android.CrashLog

/**
 * "Send crash log" (This device → App crashes): the phone's crash log ([CrashLog.exportText]) as a bug report of the
 * kind `crash`, so it lands in the admin's Bug reports under Crash logs (and in its .txt export, written in whole).
 * The description is Android's exit records; the whole log is the attached .txt. Blocking: call it off the main thread.
 * Gives the new report's id.
 */
object CrashLogReport {
    /** The server takes text files up to 8 MB; the log is cut well before that (by characters: it stays UTF-8). */
    private const val MAX_CHARS = 3_000_000
    /** The server's limit for a description is 20,000 characters. */
    private const val MAX_BODY = 18_000

    fun send(graph: AppGraph, note: String = ""): Long {
        val app = graph.app
        val file = CrashLog.exportText(app)
        val bytes = file.readText().let { if (it.length > MAX_CHARS) it.take(MAX_CHARS) + "\n… (cut)" else it }.toByteArray()
        val summary = CrashLog.latestSummary(app).ifBlank { "sent from the app" }
        val body = buildString {
            if (note.isNotBlank()) append(note.trim()).append("\n\n")
            append("Why Android closed the app (newest first):\n")
            append(CrashLog.exitHistory(app).ifBlank { "(none recorded)\n" })
            append("\nThe whole log is in the attached file.")
        }.take(MAX_BODY)
        return graph.bugReports.create(
            kind = "crash",
            title = BugRules.crashTitle(summary),
            body = body,
            page = "This device",
            appVersion = "${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})",
            device = CrashLog.deviceLine(),
            files = listOf(Attachment(Uri.fromFile(file), file.name, "text/plain", bytes.size.toLong(), bytes)),
            resolver = app.contentResolver,
        )
    }
}
