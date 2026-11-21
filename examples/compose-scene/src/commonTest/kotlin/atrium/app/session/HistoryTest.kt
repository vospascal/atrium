package atrium.app.session

import atrium.engine.EditHistory
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import atrium.app.editor.historyLine

class HistoryTest {
    private var now = 0L
    private val steps = Steps { now }

    /** A drag is many edits close together: one step. */
    @Test
    fun editsCloseTogetherOfOneThingAreOneStep() {
        assertTrue(steps.starts("level:1"))
        repeat(20) {
            now += 50
            assertFalse(steps.starts("level:1"))
        }
    }

    @Test
    fun anotherThingOrAPauseStartsANewStep() {
        assertTrue(steps.starts("level:1"))
        assertTrue(steps.starts("level:2"))
        now += SameStepWithinMillis + 1
        assertTrue(steps.starts("level:2"))
    }

    /** After an undo, the same slider moved again is a new step, not part of the one undone. */
    @Test
    fun endingAStepSeparatesTheNext() {
        assertTrue(steps.starts("level:1"))
        steps.end()
        assertTrue(steps.starts("level:1"))
    }

    @Test
    fun theLineHasTheOpenedSceneTheStepsAndWhereItStands() {
        val line = historyLine(EditHistory(listOf("Move robin", "Louder"), listOf("Add frog")))
        assertEquals(listOf("Opened", "Move robin", "Louder", "Add frog"), line.labels)
        assertEquals(2, line.current)
    }
}
