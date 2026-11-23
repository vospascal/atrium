package atrium.app.editor

import atrium.engine.SceneKind
import atrium.engine.SceneFile
import kotlin.test.Test
import kotlin.test.assertEquals

class SaveSceneTest {
    @Test
    fun aTitleBecomesAPlainFileName() {
        assertEquals("meadow-at-dusk", fileNameFor("Meadow at dusk"))
        assertEquals("frog-edit", fileNameFor("  Frog — edit! "))
        assertEquals("untitled", fileNameFor("…"))
    }

    @Test
    fun aHandWrittenSceneIsNeverTheTarget() {
        val scenes = listOf(
            SceneFile("frog", "scenes/frog.yaml", "Frog", SceneKind.NATURE, savedByEditor = false, sounds = emptyList()),
            SceneFile("frog-edit", "scenes/frog-edit.yaml", "Frog edit", SceneKind.NATURE, savedByEditor = true, sounds = emptyList()),
        )
        assertEquals(SaveTarget.HandWritten("scenes/frog.yaml"), saveTarget("Frog", "scenes", scenes))
        assertEquals(SaveTarget.Replaces("scenes/frog-edit.yaml"), saveTarget("Frog edit", "scenes", scenes))
        assertEquals(SaveTarget.New("scenes/pond.yaml"), saveTarget("Pond", "scenes", scenes))
    }
}
