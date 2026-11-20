package atrium.app

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

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
