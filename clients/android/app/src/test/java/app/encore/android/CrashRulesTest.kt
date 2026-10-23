package app.encore.android

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Exit records that are an update install, not a crash ([CrashRules]). The owner's report (Oct 23): "Crash log: Closed
 * while in use: package updated: stop app.encore.client due to killDueToPackageUpdate".
 */
class CrashRulesTest {
    /** The record from the report (reason 16, Android 14 and later) and the GrapheneOS state change before it. */
    @Test
    fun anUpdateInstallIsNotACrash() {
        assertTrue(CrashRules.isUpdateKill(16, "stop app.encore.client due to killDueToPackageUpdate"))
        assertTrue(CrashRules.isUpdateKill(16, null))
        assertTrue(CrashRules.isUpdateKill(15, "[KILL UID] GosPackageStateChange"))
    }

    /** Before Android 14 an update kill has a less specific reason (13, "other"); its description still says so. */
    @Test
    fun anOlderAndroidsUpdateKillIsToldByItsDescription() {
        assertTrue(CrashRules.isUpdateKill(13, "stop app.encore.client due to killDueToPackageUpdate"))
    }

    /** Real crashes, kills for memory and the rest still become reports. */
    @Test
    fun crashesAndOtherClosesStillCount() {
        assertFalse(CrashRules.isUpdateKill(4, "java.lang.IllegalStateException"))
        assertFalse(CrashRules.isUpdateKill(5, null))
        assertFalse(CrashRules.isUpdateKill(3, "lmk"))
        assertFalse(CrashRules.isUpdateKill(13, "[KILL UID] other"))
    }
}
