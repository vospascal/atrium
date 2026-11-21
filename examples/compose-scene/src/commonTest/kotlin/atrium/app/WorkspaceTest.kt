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
