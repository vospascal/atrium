package atrium.app

import atrium.engine.Follow
import kotlin.test.Test
import kotlin.test.assertEquals

class WeatherTest {
    /** A sound's links to the weather read as a sentence, whatever the setting is. */
    @Test
    fun aLinkIsSaidInALine() {
        assertEquals("Activity 100 % → 0 % as rain rises", followLine(Follow("rain", "activity", 1f, 0f)))
        assertEquals("Level 0 dB → -6 dB as wind rises", followLine(Follow("wind", "level", 0f, -6f)))
        assertEquals("Max speed 3 → 14 as wind rises", followLine(Follow("wind", "max_speed", 3f, 14f)))
        assertEquals("Crash chance 0.1 → 0.8 as wind rises", followLine(Follow("wind", "crash_chance", .1f, .8f)))
    }
}
