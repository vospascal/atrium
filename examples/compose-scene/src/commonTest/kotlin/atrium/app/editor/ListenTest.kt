package atrium.app.editor

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ListenTest {
    @Test
    fun theWholeSceneIsEveryPartEveryEffectAndEveryOutput() {
        assertTrue(WholeRender.isWhole)
        assertTrue(WholeRender.hearsEveryPart)
        assertNull(RenderPart.heardAlone(WholeRender), "no part is alone while all are heard")
        RenderEffect.entries.forEach { assertTrue(it.isOn(WholeRender), "${it.label} is on") }
    }

    /** Hearing one part hides the other three and nothing else; every part again brings them back. */
    @Test
    fun onePartIsHeardAloneAndTheRestComeBack() {
        val windless = RenderEffect.MovingAir.toggled(WholeRender).copy(onlyChannel = 1u)
        for (part in RenderPart.entries) {
            val alone = part.alone(windless)
            assertEquals(part, RenderPart.heardAlone(alone))
            assertEquals(listOf(part), RenderPart.entries.filter { it.isHeard(alone) })
            assertFalse(alone.movingAir, "its effects stay as they were")
            assertEquals(1u.toUByte(), alone.onlyChannel, "and its output")
            assertEquals(windless, alone.everyPart())
        }
    }

    /** An effect is switched off and on again by itself. */
    @Test
    fun anEffectIsSwitchedByItself() {
        for (effect in RenderEffect.entries) {
            val off = effect.toggled(WholeRender)
            assertFalse(effect.isOn(off))
            assertEquals(listOf(effect), RenderEffect.entries.filterNot { it.isOn(off) })
            assertTrue(off.hearsEveryPart)
            assertEquals(WholeRender, effect.toggled(off))
        }
    }

    /** The top bar says what is left of the scene, and nothing while it is whole. */
    @Test
    fun whatIsHeardIsSaidInALine() {
        val room = listOf("L", "R", "C", "LFE", "Ls", "Rs")
        assertNull(probeLine(WholeRender, room))
        assertEquals("Hearing reverb only", probeLine(RenderPart.Reverb.alone(WholeRender), room))
        assertEquals(
            "Hearing without moving air, ground effect",
            probeLine(RenderEffect.Ground.toggled(RenderEffect.MovingAir.toggled(WholeRender)), room),
        )
        assertEquals("Hearing Ls only", probeLine(WholeRender.copy(onlyChannel = 4u), room))
        assertEquals(
            "Hearing direct sound only · without air absorption · R only",
            probeLine(RenderEffect.AirAbsorption.toggled(RenderPart.Direct.alone(WholeRender)).copy(onlyChannel = 1u), room),
        )
        assertEquals(
            "Hearing direct sound and reflections",
            probeLine(WholeRender.copy(reverb = false, ambient = false), room),
        )
        assertEquals("Hearing channel 3 only", probeLine(WholeRender.copy(onlyChannel = 2u), emptyList()))
        assertEquals(
            "Hearing without near field · R only",
            probeLine(RenderEffect.NearField.toggled(WholeRender).copy(onlyChannel = 1u), room),
        )
        assertEquals(
            "Hearing without headphone equalisation",
            probeLine(RenderEffect.HeadphoneEqualisation.toggled(WholeRender), room),
        )
    }

    /** The near field is an effect like the others: on in the whole scene, switched by itself. */
    @Test
    fun theNearFieldIsSwitchedLikeTheOtherEffects() {
        assertTrue(WholeRender.nearField)
        val without = RenderEffect.NearField.toggled(WholeRender)
        assertFalse(without.nearField)
        assertFalse(without.isWhole)
        assertTrue(without.hearsEveryPart)
        assertEquals(listOf(RenderEffect.NearField), RenderEffect.entries.filterNot { it.isOn(without) })
    }

    /** The headphone equalisation is an effect like the others: on in the whole scene, switched by itself. */
    @Test
    fun theHeadphoneEqualisationIsSwitchedLikeTheOtherEffects() {
        assertTrue(WholeRender.headphoneEqualisation)
        val without = RenderEffect.HeadphoneEqualisation.toggled(WholeRender)
        assertFalse(without.headphoneEqualisation)
        assertFalse(without.isWhole)
        assertTrue(without.hearsEveryPart)
        assertEquals(listOf(RenderEffect.HeadphoneEqualisation), RenderEffect.entries.filterNot { it.isOn(without) })
        assertEquals(WholeRender, RenderEffect.HeadphoneEqualisation.toggled(without))
    }
}
