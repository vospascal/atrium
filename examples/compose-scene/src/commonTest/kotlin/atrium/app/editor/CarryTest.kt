package atrium.app.editor

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class CarryTest {
    private val carry = Carry<String>()
    private val dropped = mutableListOf<Pair<String, Offset>>()

    init {
        carry.offer("map", Rect(100f, 50f, 500f, 450f)) { item, at -> dropped += item to at }
    }

    /** Let go over the map, the map gets it, at the place inside the map. */
    @Test
    fun letGoOverATargetDropsItThere() {
        carry.pickUp("frog", Offset(20f, 60f))
        assertFalse(carry.overTarget)
        carry.moveBy(Offset(200f, 100f))
        assertTrue(carry.overTarget)
        carry.letGo()
        assertEquals(listOf("frog" to Offset(120f, 110f)), dropped)
        assertNull(carry.item)
    }

    @Test
    fun letGoElsewherePutsItBack() {
        carry.pickUp("frog", Offset(20f, 60f))
        carry.letGo()
        assertTrue(dropped.isEmpty())
        assertNull(carry.item)
    }

    @Test
    fun aWithdrawnTargetTakesNothing() {
        carry.withdraw("map")
        carry.pickUp("frog", Offset(200f, 200f))
        carry.letGo()
        assertTrue(dropped.isEmpty())
    }
}
