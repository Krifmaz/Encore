package app.encore.android

import android.app.Activity
import android.app.ActivityManager
import android.app.Application
import android.app.ApplicationExitInfo
import android.content.ComponentCallbacks2
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import android.util.Log
import app.encore.android.features.bugs.CrashLogReport
import app.encore.android.ui.components.Toasts
import androidx.annotation.RequiresApi
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import org.json.JSONObject
import java.io.File
import java.io.PrintWriter
import java.io.StringWriter
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

/**
 * Crash capture (the owner on GrapheneOS: "the app randomly crashes and/or closes and then the music stops").
 *
 * - An uncaught exception on any thread writes a report (stack, app version, device, OS and the last 200 lines
 *   of [log]) to filesDir/crashes/<ts>.json before the previous handler (the system's) ends the process.
 * - On the next start, crashes the system saw but the app couldn't catch (native crashes with their tombstone,
 *   ANRs with their trace, and every close while in use or playing: for memory, by a signal, for an update of the
 *   app or of a part it uses, by the user or by Android for another reason) are read from ApplicationExitInfo
 *   (Android 11+) into the same folder.
 * - [exportText] writes the phone's own crash log as a .txt to share (This device → App crashes), without the
 *   server: Android's exit records, the reports kept here and the recent log.
 * - Every report is sent to POST /api/diagnostics/client-crash (Diagnostics → App crashes for admins) and deleted
 *   once the server has it; without a connection they wait for the next start.
 */
object CrashLog {
    private const val TAG = "Encore"
    private const val MAX_LINES = 200
    /** reports waiting for the server; older ones go when there are more */
    private const val MAX_FILES = 20
    /** a trace or tombstone is cut to this many characters */
    private const val MAX_TRACE = 200_000
    /** reports already sent, kept for the phone's own crash log */
    private const val MAX_SENT = 10
    /** on the first start with this code, system exit records this far back are still sent */
    private const val FIRST_LOOK_BACK_MS = 14L * 24 * 3600 * 1000

    private const val PREFS = "encore_crash"
    private const val K_EXIT_SEEN = "exit_seen"
    private const val K_JAVA_CRASH_AT = "java_crash_at"
    /** the newest report the "send the crash log" toast was shown for (its file's time) */
    private const val K_OFFERED_AT = "offered_at"

    private val ring = ArrayDeque<String>(MAX_LINES)
    @Volatile private var dir: File? = null
    @Volatile private var installed = false

    /** One line into the in-app ring log (kept in memory, the last 200 lines go into a crash report) and logcat. */
    fun log(msg: String) {
        Log.i(TAG, msg)
        val line = "${stamp(System.currentTimeMillis())} [${Thread.currentThread().name}] $msg"
        synchronized(ring) {
            if (ring.size >= MAX_LINES) ring.removeFirst()
            ring.addLast(line)
        }
    }

    private fun recent(): String = synchronized(ring) { ring.joinToString("\n") }

    /** Installs the handler (chained to the previous one) and the lifecycle notes. Call first thing in Application.onCreate. */
    fun install(app: Application) {
        if (installed) return
        installed = true
        dir = File(app.filesDir, "crashes")
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            try {
                write(app, thread, error)
            } catch (_: Throwable) {
                // never let the report hide the crash
            }
            if (previous != null) previous.uncaughtException(thread, error) else {
                android.os.Process.killProcess(android.os.Process.myPid())
                kotlin.system.exitProcess(10)
            }
        }
        log("app start ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE}), pid ${android.os.Process.myPid()}")
        app.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = log("${activity.javaClass.simpleName} created")
            override fun onActivityStarted(activity: Activity) = log("${activity.javaClass.simpleName} started")
            override fun onActivityResumed(activity: Activity) {}
            override fun onActivityPaused(activity: Activity) {}
            override fun onActivityStopped(activity: Activity) = log("${activity.javaClass.simpleName} stopped")
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
            override fun onActivityDestroyed(activity: Activity) = log("${activity.javaClass.simpleName} destroyed")
        })
        app.registerComponentCallbacks(object : ComponentCallbacks2 {
            override fun onTrimMemory(level: Int) = log("trim memory $level")
            override fun onConfigurationChanged(newConfig: Configuration) {}
            @Deprecated("Deprecated in Java")
            override fun onLowMemory() = log("low memory")
        })
    }

    /** How many reports wait for the server (Settings → This device). */
    fun pendingCount(): Int = dir?.listFiles { f -> f.name.endsWith(".json") }?.size ?: 0

    /** Collects the system's exit records and sends every waiting report, in the background. */
    fun uploadPending(app: Application, graph: AppGraph) {
        graph.scope.launch(Dispatchers.IO) {
            try {
                collectExits(app)
                graph.store.awaitLoaded()
                if (graph.store.current == null) return@launch
                offerToSend(app, graph)
                send(graph)
            } catch (e: Exception) {
                Log.w(TAG, "crash reports not sent", e)
            }
        }
    }

    /**
     * After a crash or a close while playing (not one the user asked for): a toast with Send, which sends the crash
     * log to the admin's Bug reports ([CrashLogReport]). Once per new report.
     */
    private fun offerToSend(app: Application, graph: AppGraph) {
        val d = dir ?: return
        val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val offered = prefs.getLong(K_OFFERED_AT, 0L)
        val fresh = d.listFiles { f -> f.name.endsWith(".json") && f.lastModified() > offered }.orEmpty()
        val worth = fresh.filter { f -> try { JSONObject(f.readText()).optString("kind") != "stopped" } catch (_: Exception) { false } }
        if (fresh.isEmpty()) return
        prefs.edit().putLong(K_OFFERED_AT, fresh.maxOf { it.lastModified() }).apply()
        if (worth.isEmpty()) return
        Toasts.show("Encore closed unexpectedly. Send the crash log to the admin?", "err", action = "Send") {
            graph.scope.launch(Dispatchers.IO) {
                try {
                    CrashLogReport.send(graph)
                    Toasts.show("Crash log sent to the admin")
                } catch (e: Exception) {
                    Toasts.show("The crash log wasn't sent: ${e.message ?: e.javaClass.simpleName}", "err")
                }
            }
        }
    }

    // ---- writing ----------------------------------------------------------------------------------------------

    private fun write(context: Context, thread: Thread, error: Throwable) {
        val now = System.currentTimeMillis()
        val sw = StringWriter()
        error.printStackTrace(PrintWriter(sw))
        val text = buildString {
            append(header(now))
            append("Thread: ").append(thread.name).append('\n')
            append('\n').append(sw.toString().trimEnd()).append('\n')
            append("\nRecent log (last ").append(MAX_LINES).append(" lines):\n").append(recent()).append('\n')
        }
        save(now, "crash", error.toString().lineSequence().firstOrNull() ?: error.javaClass.name, text, "crash-$now")
        // the system's own record of this crash (REASON_CRASH) is then skipped as a duplicate
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putLong(K_JAVA_CRASH_AT, now).commit()
    }

    private fun header(atMs: Long): String = buildString {
        append("Encore for Android ").append(BuildConfig.VERSION_NAME).append(" (").append(BuildConfig.VERSION_CODE).append(")\n")
        append("Device: ").append(Build.MANUFACTURER).append(' ').append(Build.MODEL).append(" (").append(Build.DEVICE).append(")\n")
        append("OS: ").append(os()).append('\n')
        append("Build: ").append(Build.FINGERPRINT).append('\n')
        append("Time: ").append(stamp(atMs)).append('\n')
    }

    private fun save(atMs: Long, kind: String, summary: String, text: String, key: String) {
        val d = dir ?: return
        d.mkdirs()
        val o = JSONObject()
            .put("kind", kind)
            .put("time", atMs / 1000.0)
            .put("summary", summary.take(300))
            .put("text", text)
            .put("key", key)
        // written whole, then renamed, so a half-written file is never sent
        val tmp = File(d, "$key.tmp")
        tmp.writeText(o.toString())
        tmp.renameTo(File(d, "$key.json"))
        val files = d.listFiles { f -> f.name.endsWith(".json") }?.sortedBy { it.lastModified() } ?: return
        if (files.size > MAX_FILES) files.take(files.size - MAX_FILES).forEach { it.delete() }
    }

    // ---- what the system saw (Android 11+) ---------------------------------------------------------------------

    private fun collectExits(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return
        try { collectExits30(context) } catch (e: Exception) { Log.w(TAG, "exit records unreadable", e) }
    }

    @RequiresApi(Build.VERSION_CODES.R)
    private fun collectExits30(context: Context) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val am = context.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager ?: return
        val now = System.currentTimeMillis()
        val seen = prefs.getLong(K_EXIT_SEEN, 0L).let { if (it == 0L) now - FIRST_LOOK_BACK_MS else it }
        val javaCrashAt = prefs.getLong(K_JAVA_CRASH_AT, 0L)
        val exits = am.getHistoricalProcessExitReasons(context.packageName, 0, 16)
        var newest = seen
        for (e in exits) {
            if (e.timestamp <= seen) continue
            newest = maxOf(newest, e.timestamp)
            val kind = when (e.reason) {
                ApplicationExitInfo.REASON_CRASH_NATIVE -> "native"
                ApplicationExitInfo.REASON_ANR -> "anr"
                // our own handler wrote this one already (unless it couldn't)
                ApplicationExitInfo.REASON_CRASH -> if (kotlin.math.abs(e.timestamp - javaCrashAt) < 15_000L) null else "crash"
                // a kill matters when the app was in use or playing (cached apps are closed for memory all the time)
                ApplicationExitInfo.REASON_LOW_MEMORY -> if (e.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_PERCEPTIBLE) "low_memory" else null
                ApplicationExitInfo.REASON_SIGNALED -> if (e.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_PERCEPTIBLE) "signaled" else null
                // the app quitting by itself is not a crash; the rest only counts while in use or playing
                ApplicationExitInfo.REASON_EXIT_SELF -> null
                else -> if (e.importance <= ActivityManager.RunningAppProcessInfo.IMPORTANCE_PERCEPTIBLE) inUseKind(e.reason) else null
            } ?: continue
            val summary = when (kind) {
                "native" -> "Native crash (signal ${e.status})" + (e.description?.let { ": $it" } ?: "")
                "anr" -> "App not responding" + (e.description?.let { ": $it" } ?: "")
                "low_memory" -> "Closed by the system for memory while in use"
                "signaled" -> "Killed by signal ${e.status} while in use" + (e.description?.let { ": $it" } ?: "")
                "crash" -> "Crash" + (e.description?.let { ": $it" } ?: "")
                else -> "Closed while in use: ${reasonName(e.reason)}" + (e.description?.let { ": $it" } ?: "")
            }
            val text = buildString {
                append(header(e.timestamp))
                append("Process: ").append(e.processName).append(" (pid ").append(e.pid).append(")\n")
                append("Reason: ").append(kind).append(", ").append(reasonName(e.reason)).append(" (").append(e.reason).append("), status ").append(e.status)
                append(", importance ").append(e.importance).append('\n')
                append("Memory: pss ").append(e.pss).append(" kB, rss ").append(e.rss).append(" kB\n")
                e.description?.let { append("Description: ").append(it).append('\n') }
                val trace = trace(e)
                if (trace.isNotEmpty()) append('\n').append(if (kind == "native") "Tombstone (text parts):\n" else "Trace:\n").append(trace).append('\n')
            }
            save(e.timestamp, kind, summary, text, "exit-${e.timestamp}-${e.pid}")
        }
        if (newest != seen || prefs.getLong(K_EXIT_SEEN, 0L) == 0L) prefs.edit().putLong(K_EXIT_SEEN, maxOf(newest, seen)).apply()
    }

    /**
     * The kind of a close while in use that is neither a crash nor a kill for memory or by a signal: an update of the
     * app or of a part it uses (WebView, a library app), the user or Android stopping it, or another reason.
     */
    @RequiresApi(Build.VERSION_CODES.R)
    private fun inUseKind(reason: Int): String = when (reason) {
        REASON_PACKAGE_UPDATED, ApplicationExitInfo.REASON_DEPENDENCY_DIED, REASON_PACKAGE_STATE_CHANGE -> "updated"
        ApplicationExitInfo.REASON_USER_REQUESTED, ApplicationExitInfo.REASON_USER_STOPPED -> "stopped"
        else -> "closed"
    }

    /** ApplicationExitInfo.REASON_PACKAGE_STATE_CHANGE and REASON_PACKAGE_UPDATED (Android 14), by their values. */
    private const val REASON_PACKAGE_STATE_CHANGE = 15
    private const val REASON_PACKAGE_UPDATED = 16

    /** Android's name of an exit reason, for the reports and the crash log. */
    private fun reasonName(reason: Int): String = when (reason) {
        1 -> "exit self"
        2 -> "signaled"
        3 -> "low memory"
        4 -> "crash"
        5 -> "native crash"
        6 -> "not responding"
        7 -> "initialization failure"
        8 -> "permission change"
        9 -> "excessive resource usage"
        10 -> "user requested"
        11 -> "user stopped"
        12 -> "dependency died (a part it uses was updated or stopped)"
        13 -> "other"
        14 -> "freezer"
        15 -> "package state change"
        16 -> "package updated"
        else -> "unknown ($reason)"
    }

    // ---- the phone's own crash log -------------------------------------------------------------------------------

    /**
     * The crash log as a .txt under cacheDir/shared/crash-log (shared through the FileProvider): the device, Android's
     * last exit records (every reason, in use or not), the reports waiting and the last ones sent, and the recent log.
     */
    fun exportText(context: Context): File {
        val now = System.currentTimeMillis()
        val text = buildString {
            append("Encore crash log\n").append(header(now))
            append("\n== Why Android closed the app (newest first) ==\n")
            append(exitHistory(context).ifEmpty { "(Android 11 or newer keeps these; none here)\n" })
            val d = dir
            val waiting = d?.listFiles { f -> f.name.endsWith(".json") }?.sortedByDescending { it.lastModified() }.orEmpty()
            val sent = d?.let { File(it, "sent").listFiles { f -> f.name.endsWith(".json") } }?.sortedByDescending { it.lastModified() }.orEmpty()
            for ((title, files) in listOf("Reports waiting to be sent" to waiting, "Reports already sent (newest ${MAX_SENT})" to sent)) {
                append("\n== ").append(title).append(" ==\n")
                if (files.isEmpty()) append("(none)\n")
                for (f in files) {
                    val o = try { JSONObject(f.readText()) } catch (_: Exception) { continue }
                    append("\n--- ").append(o.optString("summary")).append(" ---\n").append(o.optString("text").trimEnd()).append('\n')
                }
            }
            append("\n== Recent log (last ").append(MAX_LINES).append(" lines) ==\n").append(recent()).append('\n')
        }
        val out = File(context.cacheDir, "shared/crash-log")
        out.mkdirs()
        out.listFiles()?.forEach { if (it.isFile) it.delete() }
        val day = SimpleDateFormat("yyyy-MM-dd-HHmm", Locale.US).format(Date(now))
        return File(out, "encore-crash-log-$day.txt").also { it.writeText(text) }
    }

    /** The newest report's first line (kept here, sent or not), else Android's newest exit reason; "" for none. */
    fun latestSummary(context: Context): String {
        val d = dir
        val files = (d?.listFiles { f -> f.name.endsWith(".json") }.orEmpty() +
            (d?.let { File(it, "sent").listFiles { f -> f.name.endsWith(".json") } }.orEmpty()))
        val newest = files.maxByOrNull { it.lastModified() }
        newest?.let { f -> try { JSONObject(f.readText()).optString("summary") } catch (_: Exception) { null } }?.takeIf { it.isNotBlank() }?.let { return it }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return ""
        return try {
            val am = context.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager ?: return ""
            am.getHistoricalProcessExitReasons(context.packageName, 0, 1).firstOrNull()?.let { "Last close: " + reasonName(it.reason) } ?: ""
        } catch (_: Exception) { "" }
    }

    /** The device line of a crash log sent as a bug report. */
    fun deviceLine(): String = "${Build.MANUFACTURER} ${Build.MODEL} · ${os()}"

    /** Android's exit records as text (the crash log's first section; a sent crash log's description). */
    fun exitHistory(context: Context): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return ""
        return try { exitHistory30(context) } catch (e: Exception) { "(unreadable: $e)\n" }
    }

    @RequiresApi(Build.VERSION_CODES.R)
    private fun exitHistory30(context: Context): String {
        val am = context.getSystemService(Context.ACTIVITY_SERVICE) as? ActivityManager ?: return ""
        return buildString {
            for (e in am.getHistoricalProcessExitReasons(context.packageName, 0, 16)) {
                append(stamp(e.timestamp)).append("  ").append(reasonName(e.reason))
                append(", importance ").append(e.importance).append(", status ").append(e.status)
                append(", pss ").append(e.pss).append(" kB")
                e.description?.let { append("\n    ").append(it) }
                append('\n')
            }
        }
    }

    /**
     * The tombstone / ANR trace. Native tombstones are protobuf on Android 12+: the readable parts (signal, abort
     * message, frames with their function names) are kept, the binary is dropped.
     */
    @RequiresApi(Build.VERSION_CODES.R)
    private fun trace(e: ApplicationExitInfo): String {
        val bytes = try { e.traceInputStream?.use { it.readBytes() } } catch (_: Exception) { null } ?: return ""
        if (bytes.isEmpty()) return ""
        val printable = bytes.count { b -> val c = b.toInt() and 0xff; c == 9 || c == 10 || c == 13 || c in 32..126 || c >= 0x80 }
        val text = if (printable >= bytes.size * 95 / 100) String(bytes, Charsets.UTF_8) else strings(bytes)
        return if (text.length > MAX_TRACE) text.take(MAX_TRACE) + "\n… (cut)" else text
    }

    /** Runs of 4+ printable ASCII characters, one per line (like `strings`). */
    private fun strings(bytes: ByteArray): String {
        val out = StringBuilder()
        val run = StringBuilder()
        fun flush() {
            if (run.length >= 4) out.append(run).append('\n')
            run.setLength(0)
        }
        for (b in bytes) {
            val c = b.toInt() and 0xff
            if (c in 32..126) run.append(c.toChar()) else flush()
            if (out.length > MAX_TRACE) break
        }
        flush()
        return out.toString()
    }

    // ---- sending ----------------------------------------------------------------------------------------------

    private fun send(graph: AppGraph) {
        val d = dir ?: return
        val files = d.listFiles { f -> f.name.endsWith(".json") }?.sortedBy { it.lastModified() } ?: return
        if (files.isEmpty()) return
        val deviceId = graph.store.deviceId
        for (f in files) {
            val stored = try { Json.parseToJsonElement(f.readText()).jsonObject } catch (_: Exception) { f.delete(); continue }
            val body = JsonObject(stored + mapOf(
                "device" to JsonPrimitive("${Build.MANUFACTURER} ${Build.MODEL}"),
                "device_id" to JsonPrimitive(deviceId),
                "app_version" to JsonPrimitive("${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})"),
                "os" to JsonPrimitive(os()),
            ))
            try {
                graph.api.post("/api/diagnostics/client-crash", body)
                keepSent(d, f)
            } catch (e: app.encore.android.data.api.ApiException) {
                // a report the server refuses for good is dropped; an older server without the route (404/405),
                // a sign-in problem or a busy server: it waits (at most MAX_FILES are kept)
                if (e.code == 400 || e.code == 413 || e.code == 422) f.delete() else return
            } catch (_: Exception) {
                return   // offline: the rest waits for the next start
            }
        }
    }

    /** A sent report moves to crashes/sent (the newest [MAX_SENT] stay) for the phone's own crash log. */
    private fun keepSent(d: File, f: File) {
        val sent = File(d, "sent")
        sent.mkdirs()
        if (!f.renameTo(File(sent, f.name))) f.delete()
        val files = sent.listFiles { x -> x.name.endsWith(".json") }?.sortedBy { it.lastModified() } ?: return
        if (files.size > MAX_SENT) files.take(files.size - MAX_SENT).forEach { it.delete() }
    }

    private fun os(): String = "Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT}) · ${Build.DISPLAY}"

    private fun stamp(ms: Long): String =
        SimpleDateFormat("yyyy-MM-dd HH:mm:ss.SSS Z", Locale.US).apply { timeZone = TimeZone.getDefault() }.format(Date(ms))
}
