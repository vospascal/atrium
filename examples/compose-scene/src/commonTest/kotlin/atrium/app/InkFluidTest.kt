package atrium.app

import androidx.compose.ui.graphics.Color
import kotlin.math.PI
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class InkFluidTest {
    private val east = Plume(0f, 1f, Color.Blue)

    private fun water() = InkFluid(64, 64, listenerX = 32f, listenerY = 32f, reach = 28f)

    private fun InkFluid.run(seconds: Float, plumes: (Float) -> List<Plume>) {
        var time = 0f
        while (time < seconds) {
            step(plumes(time), 1f / 60)
            time += 1f / 60
        }
    }

    @Test
    fun inkStartsAtTheSoundAndReachesTheListener() {
        val water = water()
        water.run(.5f) { listOf(east) }
        val atSound = water.inkNear(32f + 28f, 32f, 6f)
        val atListener = water.inkNear(32f, 32f, 5f)
        assertTrue(atSound > 0f && atListener < atSound * .01f, "released at the sound first: sound $atSound, listener $atListener")
        water.run(8f) { listOf(east) }
        assertTrue(water.inkNear(32f, 32f, 6f) > .05f, "after a few seconds it has flowed in to the listener")
        assertTrue(water.inkNear(32f - 18f, 32f, 8f) < water.inkNear(32f + 14f, 32f, 8f) * .1f, "and not past it to the far side")
    }

    @Test
    fun aSilentSoundReleasesNothingAndItsInkDrainsAway() {
        val water = water()
        water.run(6f) { listOf(east) }
        val peak = water.totalInk()
        water.run(25f) { listOf(east.copy(strength = 0f)) }
        assertTrue(water.totalInk() < peak * .05f, "left ${water.totalInk()} of $peak")
        assertTrue(water.fastest() < 1f, "and the water comes to rest, at ${water.fastest()} cells/s")
    }

    @Test
    fun aLouderSoundReleasesMoreInk() {
        val loud = water().apply { run(3f) { listOf(east) } }
        val quiet = water().apply { run(3f) { listOf(east.copy(strength = .3f)) } }
        assertTrue(loud.totalInk() > quiet.totalInk() * 2f)
    }

    @Test
    fun lowerFrequenciesWidenTheOriginalPlumeAtTheSameLevel() {
        val bass = east.copy(signal = PlumeSignal(1f, .94f, .01f)).forBand(InkLayer.Original.band)
        val treble = east.copy(signal = PlumeSignal(1f, .01f, .94f)).forBand(InkLayer.Original.band)
        val broad = water().apply { step(listOf(bass), 1f / 30f) }
        val narrow = water().apply { step(listOf(treble), 1f / 30f) }
        assertTrue(narrow.inkNear(60f, 32f, 1f) > 0f)
        assertTrue(broad.inkNear(60f, 37f, 1f) > narrow.inkNear(60f, 37f, 1f))
    }

    @Test
    fun aMovingSoundStirsTheWaterAndNothingBlowsUp() {
        val water = water()
        water.run(30f) { time ->
            List(6) { index -> Plume((index * PI / 3 + time * .8f).toFloat(), 1f, Color.Red) }
        }
        val fastest = water.fastest()
        assertTrue(fastest.isFinite() && fastest < 200f, "fastest water $fastest cells/s")
        assertTrue(water.totalInk().isFinite())
    }

    @Test
    fun zenTwoFansOutFieldsAndKeepsBothLayersQuietAfterTheyStop() {
        for (layer in listOf(InkLayer.Veil, InkLayer.Ribbon)) {
            val fluid = InkFluid(36, 36, reach = 13f, detail = 1, layer = layer)
            val field = east.copy(extent = PlumeExtent.Fan)
            fluid.run(.4f) { listOf(field) }
            assertTrue(fluid.inkNear(28f, 10f, 3f) > 0f, "$layer upper edge receives ink")
            assertTrue(fluid.inkNear(28f, 26f, 3f) > 0f, "$layer lower edge receives ink")
            assertTrue(fluid.inkNear(5f, 18f, 3f) == 0f, "$layer has no inlet on the far side")
            fluid.run(2f) { time -> listOf(field.copy(direction = time)) }
            val ink = fluid.totalInk()
            fluid.run(25f) { listOf(field.copy(strength = 0f)) }
            assertTrue(fluid.totalInk() < ink * .05f, "$layer stops emitting and fades")
            assertTrue(fluid.fastest() < 1f, "$layer comes to rest")
        }
    }

    @Test
    fun zenTwoCarriesInkPastTheListener() {
        for (layer in listOf(InkLayer.Veil, InkLayer.Ribbon)) {
            val fluid = InkFluid(48, 32, reach = 30f, detail = 1, layer = layer)
            fluid.run(8f) { listOf(east) }
            val beyond = fluid.inkNear(15f, 16f, 7f)
            assertTrue(beyond > .02f, "$layer should carry pigment past the listener, got $beyond")
        }
    }

    @Test
    fun zenTwoIncreasesPigmentResolutionOnLargeCanvasesWithinItsBudget() {
        assertEquals(2, InkLayer.Ribbon.dyeDetail(860f, 104))
        assertEquals(6, InkLayer.Ribbon.dyeDetail(1727f, 104))
        assertEquals(3, InkLayer.Veil.dyeDetail(1727f, 68))
        assertEquals(2, InkLayer.Original.dyeDetail(1727f, 112))
    }
}
