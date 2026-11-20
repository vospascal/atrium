package atrium.app

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class LayerMixTest {
    private val meadow = listOf("wind", "frogs", "birds")

    @Test
    fun everyLayerSoundsUntilSomethingIsAsked() {
        val mix = LayerMix()
        meadow.forEach { assertFalse(mix.silences(it), "$it should sound") }
    }

    @Test
    fun aMutedLayerIsSilentAndTheRestSound() {
        val mix = LayerMix().toggleMute("frogs")
        assertTrue(mix.silences("frogs"))
        assertFalse(mix.silences("wind"))
        assertFalse(mix.toggleMute("frogs").silences("frogs"), "muting again brings it back")
    }

    /** Solo is what the engine has no word for: it becomes a mute of everything else. */
    @Test
    fun aLayerHeardAloneSilencesEveryOther() {
        val mix = LayerMix().toggleSolo("birds")
        assertEquals(listOf("wind", "frogs"), meadow.filter(mix::silences))
        val two = mix.toggleSolo("frogs")
        assertEquals(listOf("wind"), meadow.filter(two::silences), "two can be heard alone together")
        assertEquals(emptyList(), meadow.filter(two.toggleSolo("birds").toggleSolo("frogs")::silences))
    }

    /** A mute is not forgotten while another layer is heard alone, and it wins over its own solo. */
    @Test
    fun aMuteOutlastsASolo() {
        val mix = LayerMix().toggleMute("wind").toggleSolo("birds")
        assertTrue(mix.silences("wind"))
        assertTrue(mix.toggleSolo("birds").silences("wind"), "still muted after the solo ends")
        assertTrue(LayerMix().toggleMute("birds").toggleSolo("birds").silences("birds"))
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
