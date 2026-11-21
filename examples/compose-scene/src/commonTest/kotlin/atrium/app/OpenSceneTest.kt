package atrium.app

import atrium.engine.SceneFile
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class OpenSceneTest {
    private fun scene(name: String) = SceneFile(name, "scenes/$name.yaml", name, savedByEditor = false, sounds = emptyList())

    /** Opening asks first only when it would throw away changes: another scene, with the open one unsaved. */
    @Test
    fun openingAsksOnlyWhenChangesWouldBeLost() {
        assertTrue(mustAskBeforeOpening(scene("storm"), scene("meadow"), unsaved = true))
        assertFalse(mustAskBeforeOpening(scene("storm"), scene("meadow"), unsaved = false))
        assertFalse(mustAskBeforeOpening(scene("meadow"), scene("meadow"), unsaved = true), "the same scene stays")
        assertFalse(mustAskBeforeOpening(scene("storm"), null, unsaved = false), "nothing open")
    }
}
