package atrium.app.editor

import atrium.engine.Bout
import atrium.engine.EmitterKind
import atrium.engine.Gap
import atrium.engine.Position
import atrium.engine.SourceDescription
import atrium.engine.Take
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class PlayInStepTest {
    private fun sound(id: String, slot: Int, loops: Boolean) = SourceDescription(
        id, slot.toUShort(), 360f, -60f, id.replaceFirstChar { it.uppercase() }, null, "#000000", Position(0f, 0f, 0f), 65f, 1f,
        EmitterKind.OBJECT, null, 0f, 360f, Position(1f, 0f, 0f),
        if (loops) listOf(Take("Take", 1f)) else emptyList(), if (loops) null else Gap(3f, 8f), loops, Bout(1u, 1u), emptyList(), emptyList(),
    )

    private val bass = sound("bass", 0, loops = true)
    private val drums = sound("drums", 1, loops = true)
    private val vocals = sound("vocals", 2, loops = true)
    private val robin = sound("robin", 3, loops = false)
    private val scene = listOf(bass, drums, vocals, robin)

    private fun picked(vararg ids: String) = ids.fold(Selection()) { selection, id -> selection.clicked(id, adding = true) }

    /** The first loop picked leads, whatever its place in the layer list; the others join it in the order they were picked. */
    @Test
    fun theFirstLoopPickedLeads() {
        assertEquals(InStep(vocals, listOf(bass, drums)), inStep(picked("vocals", "bass", "drums"), scene))
        assertEquals(InStep(drums, listOf(bass)), inStep(picked("drums", "bass"), scene))
    }

    /** A robin rests between its songs, so it has no playhead: picked first, the first loop after it leads, and it does not join. */
    @Test
    fun aSoundThatDoesNotLoopNeitherLeadsNorJoins() {
        assertEquals(InStep(drums, listOf(vocals)), inStep(picked("robin", "drums", "vocals"), scene))
    }

    /** Two or more selected and two or more of them loops; anything less has nothing to bring into step. */
    @Test
    fun itNeedsTwoLoops() {
        assertNull(inStep(Selection(), scene))
        assertNull(inStep(picked("bass"), scene), "one loop alone")
        assertNull(inStep(picked("bass", "robin"), scene), "two selected, one loop")
        assertNull(inStep(picked(ListenerId), scene), "the listener")
        assertNull(inStep(picked("bass", "gone"), scene), "a sound no longer in the scene")
    }

    /** Taking the leader out of the selection hands the lead to the next loop picked. */
    @Test
    fun theNextLoopPickedLeadsWhenTheFirstIsLetGo() {
        val three = picked("bass", "drums", "vocals")
        assertEquals(InStep(drums, listOf(vocals)), inStep(three.clicked("bass", adding = true), scene))
    }
}
