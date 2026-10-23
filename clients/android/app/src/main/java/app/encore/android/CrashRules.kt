package app.encore.android

/**
 * Which of Android's exit records are not crashes and never become a crash report ([CrashLog]), kept apart from
 * [CrashLog] (which needs Android) so the unit tests can check them.
 *
 * Bug report "Crash log: Closed while in use: package updated: stop app.encore.client due to killDueToPackageUpdate"
 * (Oct 23): every install of an update stops the running app, by Android's design, and each one was filed as a crash.
 * The next start logs "update installed" instead, and This device → App crashes still lists the record.
 */
object CrashRules {
    /** ApplicationExitInfo.REASON_PACKAGE_STATE_CHANGE (Android 14), by its value: the app was disabled, enabled or
     *  had its state changed (GrapheneOS: "GosPackageStateChange"), which an update install does too. */
    const val REASON_PACKAGE_STATE_CHANGE = 15

    /** ApplicationExitInfo.REASON_PACKAGE_UPDATED (Android 14), by its value: the app was stopped to install an update. */
    const val REASON_PACKAGE_UPDATED = 16

    /** The words Android's activity manager writes in an exit record's description when an update install stops the
     *  app; before Android 14 such a record has a less specific reason, so the description tells it apart. */
    private const val UPDATE_KILL = "killDueToPackageUpdate"

    /**
     * Whether an exit record with this [reason] and [description] is Android stopping the app to install an update of
     * it (or changing its package state): not a crash, so no report is filed for it, in use or not.
     */
    fun isUpdateKill(reason: Int, description: String?): Boolean =
        reason == REASON_PACKAGE_UPDATED ||
            reason == REASON_PACKAGE_STATE_CHANGE ||
            description?.contains(UPDATE_KILL, ignoreCase = true) == true
}
