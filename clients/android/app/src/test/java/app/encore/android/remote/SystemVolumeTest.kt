package app.encore.android.remote

import org.junit.Assert.assertEquals
import org.junit.Test

/** The step arithmetic behind the `system_volume` snapshot field and command ([SystemVolume]). */
class SystemVolumeTest {
    @Test
    fun stepsReadAsAFractionOfTheStream() {
        assertEquals(0.5, SystemVolume.fraction(15, 30), 1e-9)
        assertEquals(1.0, SystemVolume.fraction(15, 15), 1e-9)
        // a stream without steps reads as silent, never as a division by zero
        assertEquals(0.0, SystemVolume.fraction(3, 0), 1e-9)
    }

    @Test
    fun aLevelBecomesTheNearestStepWithinTheStream() {
        assertEquals(8, SystemVolume.toSteps(0.5, 15))
        assertEquals(0, SystemVolume.toSteps(-0.2, 15))
        assertEquals(15, SystemVolume.toSteps(1.4, 15))
        assertEquals(0, SystemVolume.toSteps(0.5, 0))
    }
}
