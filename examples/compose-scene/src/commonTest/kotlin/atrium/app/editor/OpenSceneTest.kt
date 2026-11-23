package atrium.app.editor

import atrium.app.SettingsStore
import atrium.app.session.NewScene
import atrium.app.session.Session
import atrium.engine.SceneKind
import atrium.engine.SceneFile
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class OpenSceneTest {
    private fun scene(name: String) = SceneFile(name, "scenes/$name.yaml", name, SceneKind.NATURE, savedByEditor = false, sounds = emptyList())

    /** Opening asks first only when it would throw away changes: another scene, with the open one unsaved. */
    @Test
    fun openingAsksOnlyWhenChangesWouldBeLost() {
        assertTrue(mustAskBeforeOpening(scene("storm"), scene("meadow"), unsaved = true))
        assertFalse(mustAskBeforeOpening(scene("storm"), scene("meadow"), unsaved = false))
        assertFalse(mustAskBeforeOpening(scene("meadow"), scene("meadow"), unsaved = true), "the same scene stays")
        assertFalse(mustAskBeforeOpening(scene("storm"), null, unsaved = false), "nothing open")
    }

    /** A new scene is asked only its kind: a nature scene starts in the meadow, a listening session dry, and either chooses its place in the World panel. */
    @Test
    fun aNewSceneIsAskedOnlyItsKind() {
        val session = Session(Memory())
        session.playNew(SceneKind.LISTENING, "scenes")
        assertEquals(NewScene(SceneKind.LISTENING), session.request?.new)
        assertEquals("scenes/untitled.yaml", session.request?.scene?.path)
        assertTrue("meadow" in newSceneNote(SceneKind.NATURE))
        assertTrue("dry" in newSceneNote(SceneKind.LISTENING))
        assertTrue(SceneKind.entries.all { "World panel" in newSceneNote(it) })
    }

    private class Memory : SettingsStore {
        private val values = mutableMapOf<String, String>()
        override fun read(key: String): String? = values[key]
        override fun write(key: String, value: String) {
            values[key] = value
        }
    }
}
