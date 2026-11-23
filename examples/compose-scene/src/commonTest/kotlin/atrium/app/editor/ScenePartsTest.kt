package atrium.app.editor

import atrium.engine.PlaceChoice
import atrium.engine.SceneKind
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class ScenePartsTest {
    /** Everything that only means something with weather, daylight or the air of a place. */
    private val natureParts = setOf(
        ScenePart.Weather,
        ScenePart.Air,
        ScenePart.ConditionLanes,
        ScenePart.WeatherOnTimeline,
        ScenePart.WeatherSpeed,
        ScenePart.WindOnMap,
        ScenePart.Follows,
    )

    /** A listening session shows none of the nature parts, and a nature scene shows all of them. */
    @Test
    fun aListeningSessionShowsNoneOfTheNatureParts() {
        assertEquals(emptySet(), SceneKind.LISTENING.shows() intersect natureParts)
        assertTrue(SceneKind.NATURE.shows().containsAll(natureParts))
    }

    /** A listening session shows its voices and its place, and both kinds say what they are and choose their place. */
    @Test
    fun aListeningSessionShowsItsVoicesAndPlace() {
        assertEquals(setOf(ScenePart.Kind, ScenePart.Voices, ScenePart.Place, ScenePart.Listening), SceneKind.LISTENING.shows())
        assertTrue(SceneKind.NATURE.shows().containsAll(setOf(ScenePart.Kind, ScenePart.Place)))
    }

    /** The line under a place: Dry has no room; an environment its size and whether the sky is open. */
    @Test
    fun aPlaceSaysItsSizeAndWhetherTheSkyIsOpen() {
        assertEquals("No room: direct sound only", placeLine(PlaceChoice(null, 20f, 20f, 10f, openSky = true)))
        assertEquals("60 × 60 × 30 m, open sky", placeLine(PlaceChoice("environments/meadow.yaml", 60f, 60f, 30f, openSky = true)))
        assertEquals("14 × 10 × 9 m, enclosed", placeLine(PlaceChoice("environments/atrium.yaml", 14f, 10f, 9f, openSky = false)))
        assertEquals("6.5 × 4 × 2.8 m, enclosed", placeLine(PlaceChoice("rooms/odd.yaml", 6.5f, 4f, 2.75f, openSky = false)))
    }

    @Test
    fun aSpaceIsNamedAfterItsFile() {
        assertEquals("Dry", spaceLabel(null))
        assertEquals("Meadow", spaceLabel("environments/meadow.yaml"))
        assertEquals("Small Atrium", spaceLabel("environments/small_atrium.yaml"))
    }
}
