package app.encore.android.playback

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.content.ContextCompat
import app.encore.android.CrashLog

/**
 * The music plays on after an update of Encore is installed (Android's `MY_PACKAGE_REPLACED`, sent to the new
 * version right after the install).
 *
 * Android stops the running app to install its update, by design, also mid-song in the background (an app store or
 * updater installing a release by itself). Nothing started the music again until the app was opened, so it sounded
 * like the song stopped at random. Now, when the old version was playing ([ResumeRules.resumeAfterUpdate]: its
 * "playing" flag left behind, under 2 minutes ago, a queue saved), the playback service is started in the
 * foreground (this broadcast is one of the moments Android allows that from the background) and crash recovery
 * ([Resume.restoreAtStart]) puts the queue back at its spot and plays on. The start is marked as an update's
 * ([ResumeStore.updateKillAt]) so a release and its hotfix installed a minute apart don't count as a crash loop.
 */
class ResumeAfterUpdateReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        val store = ResumeStore(context)
        val (playing, sinceSavedMs, hasQueue) = store.lastState()
        if (!ResumeRules.resumeAfterUpdate(playing, sinceSavedMs, hasQueue)) return
        store.updateKillAt = System.currentTimeMillis()
        CrashLog.log("update installed while playing: starting the music again")
        val start = Intent(context, EncorePlaybackService::class.java).setAction(EncorePlaybackService.ACTION_RESUME_AFTER_UPDATE)
        try {
            ContextCompat.startForegroundService(context, start)
        } catch (e: Exception) {
            // Android refused a foreground start here (ForegroundServiceStartNotAllowedException): the queue still
            // comes back at its spot when the app opens
            store.updateKillAt = 0L
            CrashLog.log("update installed while playing: the system refused to start playback ($e)")
        }
    }
}
