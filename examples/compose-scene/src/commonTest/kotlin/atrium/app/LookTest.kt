package atrium.app

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class LookTest {
    private class MemoryStore(val values: MutableMap<String, String> = mutableMapOf()) : SettingsStore {
        override fun read(key: String) = values[key]
        override fun write(key: String, value: String) {
            values[key] = value
        }
    }

    @Test
    fun aFirstLaunchFollowsTheSystemInColor() {
        val look = Look(MemoryStore())
        assertEquals(Appearance.Auto, look.appearance)
        assertFalse(look.monochrome)
    }

    @Test
    fun choicesSurviveARestart() {
        val store = MemoryStore()
        Look(store).apply {
            appearance = Appearance.Dark
            monochrome = true
        }
        val restarted = Look(store)
        assertEquals(Appearance.Dark, restarted.appearance)
        assertTrue(restarted.monochrome)
    }

    @Test
    fun anUnknownSavedAppearanceFallsBackToAuto() {
        val look = Look(MemoryStore(mutableMapOf("appearance" to "Sepia")))
        assertEquals(Appearance.Auto, look.appearance)
    }
}
