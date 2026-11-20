package atrium.app

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import atrium.engine.EmitterKind
import atrium.engine.Position
import atrium.engine.SourceDescription
import kotlin.math.abs
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class TopDownTest {
    private val view = TopDown(Size(800f, 600f), reach = 30f)

    private fun close(expected: Float, actual: Float) =
        assertTrue(abs(expected - actual) < 1e-3f, "expected $expected, got $actual")

    /** 30 m fit in the nearer half (300 px), so a metre is 10 px in every direction. */
    @Test
    fun theListenerIsInTheMiddleAndNorthIsUp() {
        assertEquals(Offset(400f, 300f), view.toScreen(Point(0f, 0f)))
        assertEquals(Offset(400f, 200f), view.toScreen(Point(0f, 10f)), "10 m north is 100 px up")
        assertEquals(Offset(500f, 300f), view.toScreen(Point(10f, 0f)), "10 m east is 100 px right")
        close(10f, view.pixelsPerMetre)
    }

    /** A drag ends where the pointer is: screen to metres and back lands on the same pixel. */
    @Test
    fun aPointOnScreenIsThePlaceItShows() {
        val place = view.toWorld(Offset(523f, 111f))
        close(12.3f, place.x)
        close(18.9f, place.y)
        val back = view.toScreen(place)
        close(523f, back.x)
        close(111f, back.y)
    }

    @Test
    fun aClickPicksTheNearestSoundWithinReachOfThePointer() {
        val sounds = listOf(Offset(100f, 100f), Offset(130f, 100f), Offset(400f, 400f))
        assertEquals(0, nearestWithin(sounds, Offset(104f, 103f), radius = 20f))
        assertEquals(1, nearestWithin(sounds, Offset(120f, 100f), radius = 20f), "the nearer of two under the pointer")
        assertNull(nearestWithin(sounds, Offset(250f, 250f), radius = 20f), "the ground selects nothing")
        assertNull(nearestWithin(emptyList(), Offset(0f, 0f), radius = 20f))
    }

    /** The farthest sound decides the view, with a fifth to spare; a lone nearby sound does not fill it. */
    @Test
    fun theViewReachesPastTheFarthestSound() {
        close(24f, reachFor(listOf(Point(-12f, 16f), Point(3f, 0f))))
        close(12f, reachFor(listOf(Point(2f, 0f))))
        close(12f, reachFor(emptyList()))
    }

    @Test
    fun ringsDoubleFromFiveMetresUpToTheEdge() {
        assertEquals(listOf(5f, 10f, 20f), ringsWithin(22.3f))
        assertEquals(listOf(5f, 10f), ringsWithin(12f))
        assertEquals(listOf(5f, 10f, 20f, 40f), ringsWithin(40f))
    }

    /** The frog scene's own comment: 10.1 east and 10.1 north is 14.3 m at 45°. */
    @Test
    fun aSoundIsDescribedByDistanceAndBearing() {
        assertEquals("14.3 m · 45° NE", awayLabel(Point(10.1f, 10.1f)))
        assertEquals("9.8 m · 180° S", awayLabel(Point(0f, -9.8f)))
        assertEquals("0.0 m · 0° N", awayLabel(Point(0f, 0f)))
        assertEquals("-1.5", oneDecimal(-1.46f))
    }

    @Test
    fun theShapeIsSaidInALine() {
        fun sound(kind: EmitterKind, beam: Float?) = SourceDescription(
            "s", "S", null, "#000000", Position(0f, 0f, 0f), 65f, 1f, kind, beam, Position(1f, 0f, 0f),
        )
        assertEquals("Ambient: heard everywhere", shapeLine(sound(EmitterKind.FIELD, null)))
        assertEquals("Omni: equal all round", shapeLine(sound(EmitterKind.OBJECT, null)))
        assertEquals("Focused: a beam 60° wide", shapeLine(sound(EmitterKind.OBJECT, (30.0 * kotlin.math.PI / 180.0).toFloat())))
    }
}
