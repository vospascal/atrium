package atrium.app.editor

import kotlin.test.Test
import kotlin.test.assertEquals

class SoundLayersTest {
    /** A layer's fader runs evenly in dB from −60 to +12, 0 dB five sixths along, and sets half dBs. */
    @Test
    fun aLayersFaderRunsInDecibels() {
        assertEquals(0f, layerFader(-60f))
        assertEquals(1f, layerFader(12f))
        assertEquals(60f / 72f, layerFader(0f), 1e-6f)
        assertEquals(0f, layerLevelAt(layerFader(0f)))
        assertEquals(-6f, layerLevelAt(layerFader(-6.1f)))
        assertEquals(12f, layerLevelAt(2f), "kept on the track")
    }

    @Test
    fun aLayersLevelReadsAsDecibels() {
        assertEquals("0 dB", layerLevelLabel(0f))
        assertEquals("−6 dB", layerLevelLabel(-6f))
        assertEquals("+3.5 dB", layerLevelLabel(3.5f))
    }
}
