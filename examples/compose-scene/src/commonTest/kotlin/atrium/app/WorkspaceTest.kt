package atrium.app

import androidx.compose.ui.unit.dp
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class WorkspaceTest {
    private class Memory : SettingsStore {
        val values = mutableMapOf<String, String>()
        override fun read(key: String) = values[key]
        override fun write(key: String, value: String) {
            values[key] = value
        }
    }

    /** A border dragged too far stops at a sensible size, and the layout is there again after a restart. */
    @Test
    fun sizesStayInLimitsAndAreKept() {
        val store = Memory()
        val workspace = Workspace(store)
        workspace.leftWidth = 5.dp
        workspace.rightWidth = 2000.dp
        workspace.bottomHeight = 250.dp
        workspace.leftShown = false
        assertEquals(220.dp, workspace.leftWidth)
        assertEquals(520.dp, workspace.rightWidth)
        val restarted = Workspace(store)
        assertEquals(250.dp, restarted.bottomHeight)
        assertFalse(restarted.leftShown)
    }

    /** Tab hides every panel while any shows, and shows them all when none does. */
    @Test
    fun tabHidesAllOrShowsAll() {
        val workspace = Workspace(Memory())
        workspace.rightShown = false
        workspace.toggleAll()
        assertFalse(workspace.anyShown)
        workspace.toggleAll()
        assertTrue(workspace.leftShown && workspace.rightShown && workspace.bottomShown)
    }
}

class LabelsThatFitTest {
    /** Four labels crowded into a narrow ruler: the first by priority stays, those it would touch go. */
    @Test
    fun crowdedLabelsGiveWay() {
        val spans = listOf(0f..30f, 25f..55f, 60f..90f, 85f..115f)
        assertEquals(setOf(0, 2), labelsThatFit(spans, gap = 4f))
        assertEquals(setOf(3, 1), labelsThatFit(spans, gap = 4f, priority = listOf(3, 1, 0, 2)))
    }

    @Test
    fun roomyLabelsAllStay() {
        assertEquals(setOf(0, 1, 2), labelsThatFit(listOf(0f..20f, 40f..60f, 80f..100f), gap = 6f))
    }
}
