package app.encore.android.features.files

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.content.ContextCompat
import app.encore.android.R
import app.encore.android.ui.MainActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * Keeps the app running while files upload to the server ([FileUploads]): a foreground service (type dataSync) with
 * the progress as its notification. Without it Android freezes or closes the app soon after the page is left when no
 * music plays, and a large folder upload stops halfway. Started with a batch, it stops itself when the batch ends
 * (the progress goes back to null). The upload itself runs in the app's scope, as before; this only keeps the
 * process in the foreground.
 */
class UploadService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var job: Job? = null
    private var shown: String = ""
    private var shownAt = 0L

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val p = FileUploads.progress.value
        // startForeground first, even when the batch has already ended (a small file can finish before this runs):
        // a service started with startForegroundService that stops without it makes Android close the app
        try {
            val n = notification(p)
            if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIFY_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            else startForeground(NOTIFY_ID, n)
        } catch (_: Exception) {
            // not allowed now (the app went to the background first): the upload goes on as before, without it
            stopSelf()
            return START_NOT_STICKY
        }
        if (p == null) {
            finish()
            return START_NOT_STICKY
        }
        if (job == null) {
            job = scope.launch {
                FileUploads.progress.collect { u ->
                    if (u == null) {
                        finish()
                    } else {
                        update(u)
                    }
                }
            }
        }
        // a process restart has no upload to go on with
        return START_NOT_STICKY
    }

    /** Android 15+: a dataSync service may run 6 hours a day. The upload goes on without it. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        finish()
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private fun finish() {
        job?.cancel()
        job = null
        try { stopForeground(STOP_FOREGROUND_REMOVE) } catch (_: Exception) {}
        stopSelf()
    }

    /** The notification again when the file changes, at most every second for the bytes. */
    private fun update(u: UploadProgress) {
        val key = "${u.done}/${u.count}"
        val now = System.currentTimeMillis()
        if (key == shown && now - shownAt < 1000L) return
        shown = key
        shownAt = now
        val nm = getSystemService(NotificationManager::class.java) ?: return
        // without the notification permission (Android 13+) nothing shows; the service still runs
        try { nm.notify(NOTIFY_ID, notification(u)) } catch (_: SecurityException) {}
    }

    /** The progress of [u]; with no batch (it ended before the service started) just "Uploads", removed at once. */
    private fun notification(u: UploadProgress?): Notification {
        val nm = getSystemService(NotificationManager::class.java)
        if (nm != null && nm.getNotificationChannel(CHANNEL) == null) {
            nm.createNotificationChannel(NotificationChannel(CHANNEL, "Uploads", NotificationManager.IMPORTANCE_LOW))
        }
        val open = Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pi = PendingIntent.getActivity(this, 4, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val b = Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_upload)
            .setContentIntent(pi)
            .setShowWhen(false)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
        if (u == null) return b.setContentTitle("Uploads").build()
        b.setContentTitle("Uploading ${u.done + 1} of ${u.count}")
            .setContentText(u.name.substringAfterLast('/'))
        if (u.total > 0) {
            // in KiB, so the numbers fit an Int
            b.setProgress((u.total / 1024).coerceAtMost(Int.MAX_VALUE.toLong()).toInt(), (u.sent / 1024).coerceAtMost(u.total / 1024).toInt(), false)
        } else {
            b.setProgress(u.count, u.done, false)
        }
        return b.build()
    }

    companion object {
        private const val CHANNEL = "uploads"
        private const val NOTIFY_ID = 7311

        /** With a batch that has just started (its progress set): the app stays up until it ends. */
        fun start(context: Context) {
            try {
                ContextCompat.startForegroundService(context, Intent(context, UploadService::class.java))
            } catch (_: Exception) {
                // not allowed in the background (Android 12+): the upload goes on without it
            }
        }
    }
}
