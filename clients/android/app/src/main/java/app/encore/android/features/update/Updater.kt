package app.encore.android.features.update

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.net.Uri
import android.provider.Settings
import android.util.Log
import app.encore.android.BuildConfig
import app.encore.android.CrashLog
import app.encore.android.core.parseJsonObject
import app.encore.android.data.api.USER_AGENT
import app.encore.android.features.update.UpdateRules.Check
import app.encore.android.features.update.UpdateRules.Phase
import app.encore.android.features.update.UpdateRules.Release
import app.encore.android.ui.components.Toasts
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.IOException
import java.security.MessageDigest
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The phone's own update (Settings → This device → Updates; the car follows the phone).
 *
 * 1. **Check**: GitHub's newest release (`UpdateRules.LATEST`), at most once a day when the app opens and
 *    "Look for a new version when Encore starts" is on, or by hand with Check for updates. The ETag is kept, so an
 *    unchanged answer comes back as 304 and does not count against GitHub's limit of 60 checks an hour. A newer
 *    version is announced once (a toast) and shown in Settings with its notes.
 * 2. **Install** (only when asked): the APK is downloaded into `cacheDir/updates` with progress, its SHA-256 checked
 *    against the release's SHA256SUMS (and GitHub's own digest of the file) when the release has them, then the APK
 *    itself: the same package, a higher version code, and the same signing certificate as this app. Anything that
 *    does not match is deleted and never installed. With "Install unknown apps" not allowed for Encore yet, Android's
 *    page for it opens first and the install goes on when the person comes back.
 * 3. Android installs it through a `PackageInstaller` session ([ApkInstaller], [UpdateInstallReceiver]); it asks the
 *    person to confirm unless it lets an app update itself without asking (Android 12+). Encore closes while it
 *    installs.
 *
 * Debug builds (`app.encore.client.debug`) can't update themselves: the releases are the other package.
 */
object Updater {
    private const val TAG = "Encore"
    private const val PREFS = "encore_update"
    private const val K_AUTO = "check_updates"
    private const val K_CHECKED_AT = "checked_at"
    private const val K_ETAG = "etag"
    private const val K_RELEASE = "release"
    private const val K_ANNOUNCED = "announced"
    private const val DAY_MS = 24L * 3600 * 1000

    /** The package of the release builds; only it can be replaced by a release's APK. */
    const val RELEASE_PACKAGE = "app.encore.client"

    /** What Settings shows: where the updater is, the newer release the last check found, the start check switch. */
    data class State(
        val phase: Phase = Phase.Idle,
        val found: Release? = null,
        val auto: Boolean = true,
    )

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val working = AtomicBoolean(false)
    @Volatile private var loaded = false

    /** The checked APK, waiting for "Install unknown apps" or kept for another try (this process only; a new one checks the file again). */
    @Volatile private var ready: Pair<Release, File>? = null

    /** Android's confirm screen for the committed session, to open again if it did not come up by itself. */
    @Volatile private var confirm: Intent? = null

    private val client: OkHttpClient by lazy {
        // its own client: GitHub never sees the Encore session, and the APK never lands in the app's HTTP cache
        OkHttpClient.Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(60, TimeUnit.SECONDS)
            .followRedirects(true)
            // GitHub's downloads redirect to its file host over https; never down to http
            .followSslRedirects(false)
            .addInterceptor { chain -> chain.proceed(chain.request().newBuilder().header("User-Agent", USER_AGENT).build()) }
            .build()
    }

    private class Refused(message: String) : Exception(message)

    private const val TOO_BIG = "The download is bigger than the release says, so it was stopped."

    /** This build can replace itself with a release's APK (not a debug build). */
    fun canUpdateItself(context: Context): Boolean = context.packageName == RELEASE_PACKAGE

    /** A build without a release number: any release counts as newer. */
    val isDevBuild: Boolean get() = !UpdateRules.isReleaseVersion(BuildConfig.VERSION_NAME)

    private fun prefs(context: Context): SharedPreferences =
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun setPhase(p: Phase) = _state.update { it.copy(phase = p) }

    /** The switch and the last answer kept from GitHub (no network), once per process. */
    fun load(context: Context) {
        if (loaded) return
        loaded = true
        val prefs = prefs(context)
        val kept = parseJsonObject(prefs.getString(K_RELEASE, null))
        val found = (kept?.let { UpdateRules.newest(it, BuildConfig.VERSION_NAME) } as? Check.Newer)?.release
        _state.update { s ->
            s.copy(
                auto = prefs.getBoolean(K_AUTO, true),
                found = found,
                phase = if (s.phase == Phase.Idle && found != null) Phase.Available(found.version) else s.phase,
            )
        }
    }

    /** "Look for a new version when Encore starts". */
    fun setAuto(context: Context, on: Boolean) {
        prefs(context).edit().putBoolean(K_AUTO, on).apply()
        _state.update { it.copy(auto = on) }
    }

    /**
     * When the app opens (MainActivity): with the start check on, look at GitHub when the last look is a day old or
     * more, and announce a newer version once. Also clears update files left from earlier runs.
     */
    fun checkAtStart(context: Context) {
        val app = context.applicationContext
        if (!canUpdateItself(app)) return
        load(app)
        scope.launch {
            if (!working.get()) cleanUp(app, keep = _state.value.found?.apk?.name)
            if (!_state.value.auto) return@launch
            val last = prefs(app).getLong(K_CHECKED_AT, 0L)
            val age = System.currentTimeMillis() - last
            if (age in 0 until DAY_MS) {
                announce(app)
                return@launch
            }
            if (UpdateRules.busy(_state.value.phase) || working.get()) return@launch
            check(app)
            announce(app)
        }
    }

    /** Check for updates (the button). */
    fun checkNow(context: Context) {
        val app = context.applicationContext
        load(app)
        if (UpdateRules.busy(_state.value.phase) || working.get()) return
        scope.launch { check(app) }
    }

    /** A newer version the person has not been told about yet: once per version, a toast. */
    private fun announce(context: Context) {
        val v = _state.value.found?.version ?: return
        val prefs = prefs(context)
        if (prefs.getString(K_ANNOUNCED, "") == v) return
        prefs.edit().putString(K_ANNOUNCED, v).apply()
        Toasts.show("Encore $v is available – Settings → This device")
    }

    private suspend fun check(context: Context): Release? {
        setPhase(Phase.Checking)
        val result = try {
            UpdateRules.newest(fetchLatest(context), BuildConfig.VERSION_NAME)
        } catch (e: Refused) {
            Check.Error(e.message ?: "Couldn't check for updates")
        }
        return when (result) {
            is Check.Newer -> {
                CrashLog.log("update available: Encore ${result.release.version}")
                _state.update { it.copy(found = result.release, phase = Phase.Available(result.release.version)) }
                result.release
            }
            Check.None -> {
                _state.update { it.copy(found = null, phase = Phase.UpToDate) }
                null
            }
            is Check.Error -> {
                Log.i(TAG, result.message)
                setPhase(Phase.Failed(result.message))
                null
            }
        }
    }

    /** GitHub's answer for the newest release; the kept one when it answers 304 (Not Modified) to the ETag. */
    private fun fetchLatest(context: Context): kotlinx.serialization.json.JsonObject {
        val prefs = prefs(context)
        val kept = prefs.getString(K_RELEASE, null)
        val etag = prefs.getString(K_ETAG, null)
        val req = Request.Builder().url(UpdateRules.LATEST)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
        if (!etag.isNullOrEmpty() && parseJsonObject(kept) != null) req.header("If-None-Match", etag)
        try {
            client.newCall(req.build()).execute().use { res ->
                val code = res.code
                // GitHub answered: the start check waits a day (without a connection it tries at the next start)
                prefs.edit().putLong(K_CHECKED_AT, System.currentTimeMillis()).apply()
                if (code == 304) {
                    return parseJsonObject(kept) ?: throw Refused("Couldn't check for updates: GitHub answered 304")
                }
                val limited = res.header("x-ratelimit-remaining")?.trim() == "0"
                if ((code == 403 || code == 429) && limited) {
                    throw Refused("Couldn't check for updates: GitHub allows only so many checks an hour from this network. Try again later.")
                }
                if (code == 404) throw Refused("Couldn't check for updates: no release found")
                if (code !in 200..299) throw Refused("Couldn't check for updates: GitHub answered $code")
                val body = parseJsonObject(res.body.string())
                    ?: throw Refused("Couldn't check for updates: GitHub's answer could not be read")
                val trimmed = UpdateRules.trim(body)
                prefs.edit()
                    .putString(K_RELEASE, trimmed.toString())
                    .putString(K_ETAG, res.header("ETag").orEmpty())
                    .apply()
                return trimmed
            }
        } catch (e: IOException) {
            throw Refused("Couldn't check for updates: GitHub can't be reached")
        }
    }

    // ---- install ------------------------------------------------------------------------------------------------

    /** Install the newer version (the button): download, check, then hand it to Android. */
    fun install(context: Context) {
        val app = context.applicationContext
        if (!canUpdateItself(app)) return
        val waiting = confirm
        if (waiting != null && (_state.value.phase as? Phase.Installing)?.confirm == true) {
            openConfirm(context, waiting)
            return
        }
        if (UpdateRules.busy(_state.value.phase)) return
        if (!working.compareAndSet(false, true)) return
        scope.launch {
            try {
                val rel = _state.value.found ?: check(app) ?: return@launch
                val kept = ready
                val file = if (kept != null && kept.first == rel && kept.second.isFile) kept.second else {
                    ready = null
                    val f = download(app, rel)
                    setPhase(Phase.Verifying(rel.version))
                    verify(app, rel, f)
                    f
                }
                if (!app.packageManager.canRequestPackageInstalls()) {
                    // Android's page "Install unknown apps" for Encore; onResume goes on from here
                    ready = rel to file
                    setPhase(Phase.NeedsPermission(rel.version))
                    withContext(Dispatchers.Main) { openInstallPermission(app) }
                    return@launch
                }
                commit(app, rel, file)
            } catch (e: Refused) {
                CrashLog.log("update not installed: ${e.message}")
                setPhase(Phase.Failed(e.message ?: "The update didn't install."))
            } catch (e: Exception) {
                CrashLog.log("update not installed: $e")
                setPhase(Phase.Failed("The update didn't install: ${e.message ?: e.javaClass.simpleName}"))
            } finally {
                working.set(false)
            }
        }
    }

    /** Settings is in front again: back from "Install unknown apps", go on with the install when it is allowed now. */
    fun onResume(context: Context) {
        val app = context.applicationContext
        val kept = ready ?: return
        if (_state.value.phase !is Phase.NeedsPermission) return
        if (!app.packageManager.canRequestPackageInstalls()) return
        if (!working.compareAndSet(false, true)) return
        scope.launch {
            try {
                if (!kept.second.isFile) throw Refused("The download is gone. Try again.")
                commit(app, kept.first, kept.second)
            } catch (e: Refused) {
                setPhase(Phase.Failed(e.message ?: "The update didn't install."))
            } catch (e: Exception) {
                setPhase(Phase.Failed("The update didn't install: ${e.message ?: e.javaClass.simpleName}"))
            } finally {
                working.set(false)
            }
        }
    }

    private fun openInstallPermission(context: Context) {
        val i = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:" + context.packageName))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(i)
        } catch (_: ActivityNotFoundException) {
            setPhase(Phase.Failed("Android's page for installing apps could not be opened. Allow Encore to install apps in Android's settings, then try again."))
        }
    }

    private fun openConfirm(context: Context, i: Intent) {
        val launch = Intent(i)
        if (context !is android.app.Activity) launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(launch)
        } catch (e: Exception) {
            setPhase(Phase.Failed("The update didn't install: ${e.message ?: e.javaClass.simpleName}"))
        }
    }

    private fun updatesDir(context: Context): File = File(context.cacheDir, "updates")

    /** Files of earlier downloads, except the one of the version on offer. */
    private fun cleanUp(context: Context, keep: String?) {
        updatesDir(context).listFiles()?.forEach { f -> if (f.name != keep) f.delete() }
    }

    /** The APK into `cacheDir/updates` (a complete earlier download of the same file is used again). */
    private fun download(context: Context, rel: Release): File {
        val asset = rel.apk
        if (!UpdateRules.isReleaseFile(asset.url)) throw Refused("The update is not a file of Encore's releases.")
        val dir = updatesDir(context)
        dir.mkdirs()
        val file = File(dir, asset.name)
        if (file.isFile && asset.size > 0 && file.length() == asset.size) return file
        cleanUp(context, keep = null)
        val part = File(dir, asset.name + ".part")
        setPhase(Phase.Downloading(rel.version, 0L, asset.size))
        // never more than the release lists (or a fixed ceiling without a size): a wrong answer can't fill the phone
        val limit = if (asset.size > 0) asset.size else UpdateRules.MAX_APK_BYTES
        val req = Request.Builder().url(asset.url).header("Accept", "application/octet-stream").build()
        var got = 0L
        try {
            client.newCall(req).execute().use { res ->
                if (!res.isSuccessful) throw Refused("The download did not start: GitHub answered ${res.code}")
                if (res.body.contentLength() > limit) throw Refused(TOO_BIG)
                val total = res.body.contentLength().takeIf { it > 0 } ?: asset.size
                val out = try {
                    part.outputStream()
                } catch (e: IOException) {
                    throw Refused("The download cannot be saved: ${e.message}")
                }
                out.use { o ->
                    res.body.byteStream().use { input ->
                        val buf = ByteArray(1 shl 16)
                        var shown = 0L
                        while (true) {
                            val n = input.read(buf)
                            if (n < 0) break
                            if (got + n > limit) throw Refused(TOO_BIG)
                            try {
                                o.write(buf, 0, n)
                            } catch (e: IOException) {
                                throw Refused("The download cannot be saved: ${e.message}")
                            }
                            got += n
                            if (got - shown >= 256 * 1024) {
                                shown = got
                                setPhase(Phase.Downloading(rel.version, got, total))
                            }
                        }
                    }
                    o.fd.sync()
                }
            }
        } catch (e: Refused) {
            part.delete()
            throw e
        } catch (e: IOException) {
            part.delete()
            throw Refused(if (got == 0L) "The download did not start: GitHub can't be reached" else "The download stopped: GitHub can't be reached")
        }
        if (asset.size > 0 && got != asset.size) {
            part.delete()
            throw Refused("The download was cut off. Try again.")
        }
        if (!part.renameTo(file)) {
            part.delete()
            throw Refused("The download cannot be saved.")
        }
        return file
    }

    /**
     * The downloaded file against the release (SHA256SUMS, GitHub's digest) and against this app (package, version
     * code, signing certificate). A file that fails is deleted.
     */
    private fun verify(context: Context, rel: Release, file: File) {
        try {
            val hex = sha256(file)
            UpdateRules.checkDigest(rel.apk, hex)?.let { throw Refused(it) }
            val sums = rel.sums
            if (sums != null) {
                val text = try {
                    fetchText(sums.url)
                } catch (e: Exception) {
                    throw Refused("The release's SHA256SUMS could not be read (${e.message}), so nothing was installed.")
                }
                UpdateRules.checkSums(text, rel.apk.name, hex)?.let { throw Refused(it) }
            }
            ApkInstaller.check(context, file)?.let { throw Refused(it) }
            CrashLog.log("Encore ${rel.version} downloaded and checked ($hex)")
        } catch (e: Exception) {
            file.delete()
            throw e
        }
    }

    private fun sha256(file: File): String {
        val md = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buf = ByteArray(1 shl 16)
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                md.update(buf, 0, n)
            }
        }
        return UpdateRules.hex(md.digest())
    }

    /** A small text file of the release (SHA256SUMS), at most 1 MB. */
    private fun fetchText(url: String): String {
        if (!UpdateRules.isReleaseFile(url)) throw IOException("not a release file")
        client.newCall(Request.Builder().url(url).build()).execute().use { res ->
            if (!res.isSuccessful) throw IOException("GitHub answered ${res.code}")
            val out = java.io.ByteArrayOutputStream()
            res.body.byteStream().use { input ->
                val buf = ByteArray(8192)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    out.write(buf, 0, n)
                    if (out.size() > 1 shl 20) throw IOException("it is far too big")
                }
            }
            return String(out.toByteArray(), Charsets.UTF_8)
        }
    }

    private fun commit(context: Context, rel: Release, file: File) {
        setPhase(Phase.Installing(rel.version))
        confirm = null
        ready = null
        ApkInstaller.commit(context, file, rel.version)
        // kept (checked) for another try when the person cancels; the next start clears it once installed
        ready = rel to file
        CrashLog.log("update handed to Android: Encore ${rel.version}")
    }

    // ---- from UpdateInstallReceiver -----------------------------------------------------------------------------

    /** Android waits for the person to confirm; [install] opens its screen again if it did not come up. */
    internal fun awaitConfirm(version: String, intent: Intent) {
        confirm = intent
        setPhase(Phase.Installing(version, confirm = true))
    }

    internal fun installFailed(message: String) {
        confirm = null
        CrashLog.log("update not installed: $message")
        setPhase(Phase.Failed(message))
    }

    internal fun installed(context: Context, version: String) {
        confirm = null
        ready = null
        CrashLog.log("update installed: Encore $version")
        scope.launch { cleanUp(context.applicationContext, keep = null) }
        _state.update { it.copy(found = null, phase = Phase.Idle) }
    }
}
