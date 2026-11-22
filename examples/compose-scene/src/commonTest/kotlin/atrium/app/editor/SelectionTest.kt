package atrium.app.editor

import kotlin.test.Test
import kotlin.test.assertEquals

class SelectionTest {
    private fun of(vararg ids: String) = Selection(ids.toSet())

    @Test
    fun aPlainClickSelectsOneThingAndEmptyGroundNothing() {
        assertEquals(of("frog"), of("robin", "wind").clicked("frog", adding = false))
        assertEquals(Selection(), of("robin", "wind").clicked(null, adding = false))
    }

    @Test
    fun aClickWithShiftAddsASoundAndTakesItOutAgain() {
        val two = of("robin").clicked("frog", adding = true)
        assertEquals(of("robin", "frog"), two)
        assertEquals(of("robin"), two.clicked("frog", adding = true))
        assertEquals(two, two.clicked(null, adding = true), "a miss with Shift held keeps what was selected")
    }

    /** The listener is moved by itself, so it never joins a group of sounds, nor they it. */
    @Test
    fun theListenerIsOnlyEverSelectedAlone() {
        assertEquals(of(ListenerId), of("robin", "frog").clicked(ListenerId, adding = true))
        assertEquals(of("robin"), of(ListenerId).clicked("robin", adding = true))
    }

    @Test
    fun inAListAClickOnTheOnlySelectedRowLetsGo() {
        assertEquals(Selection(), of("robin").clickedInList("robin", adding = false))
        assertEquals(of("robin"), of("robin", "frog").clickedInList("robin", adding = false))
    }

    /** Dragging one of several selected sounds carries them all; dragging another carries only that one. */
    @Test
    fun aDragCarriesTheSelectionItStartsIn() {
        assertEquals(of("robin", "frog"), of("robin", "frog").grabbed("frog"))
        assertEquals(of("wind"), of("robin", "frog").grabbed("wind"))
    }

    @Test
    fun soundsTakenOutOfTheSceneAreForgotten() {
        assertEquals(of("robin"), of("robin", "frog").among(listOf("robin", "wind")))
        assertEquals(of(ListenerId), of(ListenerId).among(emptyList()))
    }

    /** Sounds keep the order they were picked in, through adding, taking out and forgetting. */
    @Test
    fun theOrderOfPickingIsKept() {
        val three = Selection().clicked("wind", adding = true).clicked("robin", adding = true).clicked("frog", adding = true)
        assertEquals(listOf("wind", "robin", "frog"), three.ids.toList())
        assertEquals(listOf("wind", "frog"), three.clicked("robin", adding = true).ids.toList())
        assertEquals(listOf("robin", "frog"), three.among(listOf("frog", "robin")).ids.toList())
    }

    @Test
    fun onlyOneThingSelectedIsSingle() {
        assertEquals("robin", of("robin").single)
        assertEquals(null, of("robin", "frog").single)
        assertEquals(null, Selection().single)
    }
}
