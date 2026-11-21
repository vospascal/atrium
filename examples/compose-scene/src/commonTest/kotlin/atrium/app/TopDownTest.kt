package atrium.app

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import atrium.engine.Bout
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

    /** A wind's wash can be grabbed anywhere inside it, but a sound standing on it is picked first. */
    @Test
    fun aWashIsPickedAnywhereInsideItUnlessAPointIsUnderThePointer() {
        val frog = listOf(0 to Offset(300f, 300f))
        val wind = listOf(1 to Offset(250f, 300f))
        assertEquals(1, pickAt(frog, wind, washRadius = 80f, at = Offset(200f, 320f), radius = 20f), "inside the wash")
        assertEquals(0, pickAt(frog, wind, washRadius = 80f, at = Offset(305f, 300f), radius = 20f), "the frog on top wins")
        assertNull(pickAt(frog, wind, washRadius = 80f, at = Offset(600f, 300f), radius = 20f))
    }

    /** North is up: a sound facing north has its handle above it, one facing east to its right. */
    @Test
    fun theFacingHandleSitsTheWayTheSoundFaces() {
        val at = Offset(100f, 100f)
        val north = facingHandle(at, (kotlin.math.PI / 2).toFloat(), 40f)
        close(100f, north.x)
        close(60f, north.y)
        val east = facingHandle(at, 0f, 40f)
        close(140f, east.x)
        close(100f, east.y)
        // Dragging the handle to a point up and to the right turns the sound north-east.
        val toward = groundDirection(at, Offset(130f, 70f))
        assertEquals("45° NE", bearingLabel(Position(toward.x, toward.y, 0f)))
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

    /**
     * 20 dB more is ten times as far, 6 dB more about twice: an 80 dB sound has
     * fallen to the faint level of 60 dB at 10 m, a 100 dB one at 100 m.
     */
    @Test
    fun aLouderSoundReachesFurther() {
        close(10f, reachOf(80f, 1f))
        close(100f, reachOf(100f, 1f))
        assertTrue(kotlin.math.abs(reachOf(76f, 1f) / reachOf(70f, 1f) - 2f) < .01f, "6 dB is about twice as far")
        close(1f, reachOf(40f, 1f))
        assertTrue(reachOf(76f, 1f) > reachOf(55f, 1f))
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
        fun sound(kind: EmitterKind, width: Float, behind: Float) = SourceDescription(
            "s", 0u, width, behind, "S", null, "#000000", Position(0f, 0f, 0f), 65f, 1f, kind, null, 0f, Position(1f, 0f, 0f), emptyList(), null, Bout(1u, 1u), emptyList(),
        )
        assertEquals("Ambient: heard everywhere", shapeLine(sound(EmitterKind.FIELD, 360f, -60f)))
        assertEquals("All round: equal in every direction", shapeLine(sound(EmitterKind.OBJECT, 360f, -60f)))
        assertEquals("A beam 60° wide, nothing behind", shapeLine(sound(EmitterKind.OBJECT, 60f, -60f)))
        assertEquals("A beam 120° wide, -12 dB behind", shapeLine(sound(EmitterKind.OBJECT, 120f, -12f)))
    }

    /** The drawing follows the engine's beam: full ahead, halfway at the edge of the width, the level behind at the back. */
    @Test
    fun theBeamIsDrawnAsTheEngineShapesIt() {
        val pi = kotlin.math.PI.toFloat()
        assertEquals(1f, beamGain(120f, -12f, 0f), 1e-5f)
        assertEquals(.251f, beamGain(120f, -12f, pi), 1e-3f)
        assertEquals((1f + .251f) / 2f, beamGain(120f, -12f, pi / 3f), 1e-3f)
        assertEquals(.5f, beamGain(180f, SilentBehind, pi / 2f), 1e-5f, "a half circle is a cardioid")
        assertEquals(1f, beamGain(AllRound, SilentBehind, pi), 1e-5f, "all round")
        assertEquals(StartingShape.Focused, StartingShape.entries.single { it.matches(180f, -60f) })
        assertTrue(StartingShape.Omni.matches(360f, -12f), "behind does not matter all round")
    }

    @Test
    fun aBoutIsSaidInALine() {
        assertEquals("A new take every time", boutLine(1, 1))
        assertEquals("Each take 3 times running", boutLine(3, 3))
        assertEquals("Each take 2 to 5 times running", boutLine(2, 5))
    }
}

class MapFrameTest {
    private val size = Size(800f, 600f)
    private val reach = 30f

    private fun close(expected: Float, actual: Float) =
        assertTrue(kotlin.math.abs(expected - actual) < 1e-3f, "expected $expected, got $actual")

    /** Zooming about the pointer keeps the ground under it where it is, as in a map or Photoshop. */
    @Test
    fun zoomingKeepsThePointUnderThePointer() {
        val pointer = Offset(650f, 120f)
        val before = MapFrame().view(size, reach).toWorld(pointer)
        val zoomed = MapFrame().zoomedAbout(2f, pointer, size, reach)
        close(2f, zoomed.zoom)
        val after = zoomed.view(size, reach).toWorld(pointer)
        close(before.x, after.x)
        close(before.y, after.y)
        close(20f, zoomed.view(size, reach).pixelsPerMetre)
    }

    @Test
    fun zoomStopsAtItsLimits() {
        close(NearestZoom, MapFrame().zoomedAbout(1000f, Offset.Zero, size, reach).zoom)
        close(FarthestZoom, MapFrame().zoomedAbout(.001f, Offset.Zero, size, reach).zoom)
    }

    /** Dragged 100 px right and down, the ground follows the pointer: the middle now shows 10 m west and north. */
    @Test
    fun panningMovesTheGroundWithThePointer() {
        val panned = MapFrame().pannedBy(Offset(100f, 100f), size, reach)
        close(-10f, panned.centre.x)
        close(10f, panned.centre.y)
        val view = panned.view(size, reach)
        assertEquals(Offset(500f, 400f), view.toScreen(Point(0f, 0f)))
    }

    @Test
    fun arrowsNudgeByADecimetreOrAMetre() {
        assertEquals(Point(.1f, 0f), nudgeFor(androidx.compose.ui.input.key.Key.DirectionRight, coarse = false))
        assertEquals(Point(0f, -1f), nudgeFor(androidx.compose.ui.input.key.Key.DirectionDown, coarse = true))
        assertNull(nudgeFor(androidx.compose.ui.input.key.Key.A, coarse = false))
    }
}

class SpeakerMeterTest {
    @Test
    fun aMeterRunsFromSixtyDecibelsDownToFull() {
        assertEquals(0f, meterOf(-90f))
        assertEquals(.5f, meterOf(-30f))
        assertEquals(1f, meterOf(3f))
    }
}
