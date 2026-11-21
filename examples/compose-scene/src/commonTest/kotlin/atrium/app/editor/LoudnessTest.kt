package atrium.app.editor

import androidx.compose.ui.geometry.Size
import atrium.engine.Position
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertNull

class LoudnessTest {
    /**
     * Six dB a band down from the top: with the loudest sound 66 dB at 1 m, 66 down to 60 is the
     * loudest band, each 6 dB below the next, and under 24 dB nothing is drawn. Louder than the
     * top, closer than 1 m, is still the loudest band.
     */
    @Test
    fun levelsFallIntoSixDecibelBandsBelowTheLoudest() {
        val top = loudestBandTop(65f)
        assertEquals(66f, top)
        assertEquals(LoudnessBands - 1, loudnessBand(66f, top))
        assertEquals(LoudnessBands - 1, loudnessBand(60.1f, top))
        assertEquals(LoudnessBands - 2, loudnessBand(59.9f, top))
        assertEquals(0, loudnessBand(24.1f, top))
        assertNull(loudnessBand(23.9f, top))
        assertNull(loudnessBand(0f, top), "silence is never drawn")
        assertEquals(LoudnessBands - 1, loudnessBand(80f, top))
    }

    /** One sound in the open loses 6.02 dB per doubling of distance, so 1, 2, 4 and 8 m are four bands. */
    @Test
    fun aBandIsADoublingOfDistance() {
        val top = loudestBandTop(66f)
        val bands = listOf(1f, 2f, 4f, 8f).map { metres -> loudnessBand(66f - 20f * kotlin.math.log10(metres) - .01f, top) }
        assertEquals(listOf(6, 5, 4, 3), bands)
    }

    /** The quiet end is the one nearest the paper: the lightest step on light paper, the darkest on dark. */
    @Test
    fun theRampRunsFromThePaperOutwardInBothAppearances() {
        assertEquals(loudnessColor(0, dark = false), loudnessColor(LoudnessBands - 1, dark = true))
        assertEquals(loudnessColor(LoudnessBands - 1, dark = false), loudnessColor(0, dark = true))
        assertNotEquals(loudnessColor(0, dark = false), loudnessColor(1, dark = false))
    }

    /**
     * A map 800 × 400 px showing 20 m to its nearer edge is 80 × 40 m of ground around the origin,
     * asked for as 144 points along the wider side and half as many along the other.
     */
    @Test
    fun thePatchIsTheGroundTheMapShows() {
        val size = Size(800f, 400f)
        val patch = loudnessPatch(TopDown(size, 20f), size, Position(100f, 50f, 0f), height = 1.2f)!!
        assertEquals(Position(60f, 30f, 1.2f), patch.from)
        assertEquals(Position(140f, 70f, 1.2f), patch.to)
        assertEquals(144 to 72, patch.columns to patch.rows)
        assertNull(loudnessPatch(TopDown(Size.Zero, 20f), Size.Zero, Position(0f, 0f, 0f), 1.2f))
    }
}
