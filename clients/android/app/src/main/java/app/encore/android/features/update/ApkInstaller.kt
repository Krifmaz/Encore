package app.encore.android.features.update

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentSender
import android.content.pm.PackageInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.content.pm.Signature
import android.os.Build
import java.io.File
import java.security.MessageDigest

/**
 * The APK side of [Updater]: what a downloaded APK is (package, version code, signing certificates, read by
 * Android's own parser) and handing a checked one to Android through a `PackageInstaller` session.
 *
 * Needs in the manifest (the integrator adds them): `REQUEST_INSTALL_PACKAGES`, the receiver
 * [UpdateInstallReceiver] (not exported), and optionally `UPDATE_PACKAGES_WITHOUT_USER_ACTION` so Android 12+ may
 * update without the confirm screen.
 */
internal object ApkInstaller {
    /** What the updater compares: the package, its version code, and its signing certificates as SHA-256 hex. */
    class Info(val packageName: String, val versionCode: Long, val signers: Set<String>, val history: List<String>)

    /**
     * Why the APK at `file` may not replace this app; null when it may: Android must read it, it must be this
     * package, have a higher version code, and be signed with this app's certificate (or a rotation of it).
     */
    fun check(context: Context, file: File): String? {
        val apk = archiveInfo(context, file) ?: return "The download is not an app Android can read, so it was not installed."
        if (apk.packageName != context.packageName) {
            return "The download is a different app (${apk.packageName}), so it was not installed."
        }
        val self = installedInfo(context) ?: return "This app's own signature could not be read, so nothing was installed."
        if (apk.versionCode <= self.versionCode) {
            return "The download is not newer than this app (version code ${apk.versionCode}, this app ${self.versionCode}), so it was not installed."
        }
        if (apk.signers.isEmpty()) return "The download's signature could not be read, so it was not installed."
        if (!UpdateRules.signersMatch(self.signers, apk.signers, apk.history)) {
            return "The download is not signed with the same key as this app, so it was not installed."
        }
        return null
    }

    @Suppress("DEPRECATION")
    private fun archiveInfo(context: Context, file: File): Info? {
        val pm = context.packageManager
        val path = file.absolutePath
        fun read(flags: Int): PackageInfo? = try {
            if (Build.VERSION.SDK_INT >= 33) pm.getPackageArchiveInfo(path, PackageManager.PackageInfoFlags.of(flags.toLong()))
            else pm.getPackageArchiveInfo(path, flags)
        } catch (_: Exception) {
            null
        }
        val info = (if (Build.VERSION.SDK_INT >= 28) read(PackageManager.GET_SIGNING_CERTIFICATES) else null)
            ?.takeIf { signersOf(it).first.isNotEmpty() }
            ?: read(PackageManager.GET_SIGNATURES)
            ?: return null
        return toInfo(info)
    }

    @Suppress("DEPRECATION")
    private fun installedInfo(context: Context): Info? {
        val pm = context.packageManager
        val name = context.packageName
        fun read(flags: Int): PackageInfo? = try {
            if (Build.VERSION.SDK_INT >= 33) pm.getPackageInfo(name, PackageManager.PackageInfoFlags.of(flags.toLong()))
            else pm.getPackageInfo(name, flags)
        } catch (_: Exception) {
            null
        }
        val info = (if (Build.VERSION.SDK_INT >= 28) read(PackageManager.GET_SIGNING_CERTIFICATES) else null)
            ?.takeIf { signersOf(it).first.isNotEmpty() }
            ?: read(PackageManager.GET_SIGNATURES)
            ?: return null
        return toInfo(info).takeIf { it.signers.isNotEmpty() }
    }

    @Suppress("DEPRECATION")
    private fun toInfo(p: PackageInfo): Info {
        val code = if (Build.VERSION.SDK_INT >= 28) p.longVersionCode else p.versionCode.toLong()
        val (signers, history) = signersOf(p)
        return Info(p.packageName.orEmpty(), code, signers, history)
    }

    /** The current signing certificates and, with one signer, its proven history (oldest first). */
    @Suppress("DEPRECATION")
    private fun signersOf(p: PackageInfo): Pair<Set<String>, List<String>> {
        if (Build.VERSION.SDK_INT >= 28) {
            val si = p.signingInfo
            if (si != null) {
                val current = si.apkContentsSigners.orEmpty().map(::digest).toSet()
                val history = if (si.hasMultipleSigners()) emptyList() else si.signingCertificateHistory.orEmpty().map(::digest)
                if (current.isNotEmpty()) return current to history
            }
        }
        return p.signatures.orEmpty().map(::digest).toSet() to emptyList()
    }

    private fun digest(s: Signature): String =
        UpdateRules.hex(MessageDigest.getInstance("SHA-256").digest(s.toByteArray()))

    /** A session with the APK as its base, committed; [UpdateInstallReceiver] gets the outcome. */
    fun commit(context: Context, file: File, version: String) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
            setSize(file.length())
            setInstallReason(PackageManager.INSTALL_REASON_USER)
            // Android 12+ may skip the confirm screen for an app updating itself; it asks when it doesn't
            if (Build.VERSION.SDK_INT >= 31) setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_NOT_REQUIRED)
        }
        val id = installer.createSession(params)
        try {
            installer.openSession(id).use { session ->
                file.inputStream().use { input ->
                    session.openWrite("base.apk", 0, file.length()).use { out ->
                        input.copyTo(out, 1 shl 16)
                        session.fsync(out)
                    }
                }
                session.commit(UpdateInstallReceiver.statusSender(context, id, version))
            }
        } catch (e: Exception) {
            try {
                installer.abandonSession(id)
            } catch (_: Exception) {
            }
            throw e
        }
    }
}

/**
 * The outcome of the install session ([ApkInstaller.commit]). Not exported: only the session's PendingIntent, which
 * is this app's, reaches it. Android's confirm screen opens from here (the person just pressed Install, so Encore
 * is in front); when it can't, Settings → This device keeps an Install button that opens it.
 *
 * Manifest: `<receiver android:name=".features.update.UpdateInstallReceiver" android:exported="false" />`
 */
class UpdateInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION) return
        val version = intent.getStringExtra(EXTRA_VERSION).orEmpty()
        when (val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirm = confirmIntent(intent)
                if (confirm == null) {
                    Updater.installFailed("The update didn't install: Android did not ask to confirm it.")
                    return
                }
                Updater.awaitConfirm(version, confirm)
                try {
                    context.startActivity(Intent(confirm).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                } catch (_: Exception) {
                    // Settings → This device → Install opens it
                }
            }
            PackageInstaller.STATUS_SUCCESS -> Updater.installed(context, version)
            else -> Updater.installFailed(words(status, intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)))
        }
    }

    private fun confirmIntent(intent: Intent): Intent? =
        if (Build.VERSION.SDK_INT >= 34) intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
        else @Suppress("DEPRECATION") intent.getParcelableExtra(Intent.EXTRA_INTENT)

    private fun words(status: Int, message: String?): String {
        val why = when (status) {
            PackageInstaller.STATUS_FAILURE_ABORTED -> "it was cancelled"
            PackageInstaller.STATUS_FAILURE_STORAGE -> "there is not enough free space"
            PackageInstaller.STATUS_FAILURE_BLOCKED -> "Android blocked it"
            PackageInstaller.STATUS_FAILURE_CONFLICT, PackageInstaller.STATUS_FAILURE_INCOMPATIBLE ->
                "it doesn't fit the installed app" + (message?.takeIf { it.isNotBlank() }?.let { " ($it)" } ?: "")
            else -> message?.takeIf { it.isNotBlank() } ?: "Android gave no reason"
        }
        return "The update didn't install: $why."
    }

    companion object {
        private const val ACTION = "app.encore.android.action.UPDATE_INSTALL_STATUS"
        private const val EXTRA_VERSION = "app.encore.android.extra.UPDATE_VERSION"

        /** Where the session reports to: this receiver, by class, in a mutable PendingIntent (Android fills in the status). */
        fun statusSender(context: Context, sessionId: Int, version: String): IntentSender {
            val i = Intent(context, UpdateInstallReceiver::class.java)
                .setAction(ACTION)
                .setPackage(context.packageName)
                .putExtra(EXTRA_VERSION, version)
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
            return PendingIntent.getBroadcast(context, sessionId, i, flags).intentSender
        }
    }
}
