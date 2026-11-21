package atrium.app.session

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import atrium.app.editor.UngroupedLayers
import atrium.app.editor.groupedInSceneOrder

class LayerMixTest {
    private val meadow = listOf("wind", "frogs", "birds")

    @Test
    fun everyLayerSoundsUntilItsEarIsPressed() {
        val mix = LayerMix()
        meadow.forEach { assertFalse(mix.silences(it), "$it should sound") }
    }

    /** An ear silences its own layer and nothing else, and pressing it again brings the layer back. */
    @Test
    fun anEarSilencesItsOwnLayerOnly() {
        val mix = LayerMix().toggleMute("frogs")
        assertEquals(listOf("frogs"), meadow.filter(mix::silences))
        assertEquals(emptyList(), meadow.filter(mix.toggleMute("frogs")::silences))
    }

    /** Hearing some alone silences the rest and lets even a muted one of them sound; after, the ears count again. */
    @Test
    fun hearingAloneSilencesTheRestUntilItEnds() {
        val muted = LayerMix().toggleMute("frogs")
        val alone = muted.hearingAlone(setOf("frogs", "birds"))
        assertEquals(listOf("wind"), meadow.filter(alone::silences))
        assertEquals(listOf("frogs"), meadow.filter(alone.hearingAlone(emptySet())::silences))
        assertEquals(emptySet(), alone.without("frogs").without("birds").alone, "a removed layer is not waited for")
    }

    @Test
    fun aRemovedLayerIsForgotten() {
        assertEquals(LayerMix(), LayerMix().toggleMute("wind").without("wind"))
        assertTrue(LayerMix().toggleMute("wind").without("birds").silences("wind"), "a mute on another layer stays")
    }

    @Test
    fun aGroupIsMutedTogetherAndReturnsTogether() {
        val creatures = listOf("frogs", "birds")
        val some = LayerMix().toggleMute("frogs").toggleMuteAll(creatures)
        assertEquals(creatures, meadow.filter(some::silences), "a half-muted group is muted whole first")
        assertEquals(emptyList(), meadow.filter(some.toggleMuteAll(creatures)::silences))
    }

    @Test
    fun groupsKeepTheOrderOfTheScene() {
        val sounds = listOf("wind" to "Weather", "frogs" to "Creatures", "rain" to "Weather", "drum" to null)
        val groups = groupedInSceneOrder(sounds) { it.second }
        assertEquals(listOf("Weather", "Creatures", UngroupedLayers), groups.map { it.first })
        assertEquals(listOf("wind", "rain"), groups.first().second.map { it.first })
    }
}
