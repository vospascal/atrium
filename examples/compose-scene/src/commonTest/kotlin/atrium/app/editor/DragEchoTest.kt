package atrium.app.editor

import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.TestTimeSource
import atrium.app.Point

class DragEchoTest {
    private val clock = TestTimeSource()
    private val echo = DragEcho(clock)
    private val released = Point(10f, 20f)
    private val stale = Point(4f, 20f)

    @Test
    fun telemetryIsAppliedWhenNothingIsDragged() {
        assertTrue(echo.accepts(stale))
    }

    @Test
    fun telemetryIsIgnoredDuringADrag() {
        echo.move()
        assertFalse(echo.accepts(stale))
    }

    @Test
    fun staleTelemetryAfterReleaseIsIgnoredUntilTheEngineCatchesUp() {
        echo.move()
        echo.release(released)
        clock += 100.milliseconds
        assertFalse(echo.accepts(stale))
        assertTrue(echo.accepts(released))
        assertTrue(echo.accepts(stale), "once caught up, the engine owns the position again")
    }

    @Test
    fun anEngineThatNeverReportsTheReleaseIsTrustedAfterHalfASecond() {
        echo.release(released)
        clock += 500.milliseconds
        assertTrue(echo.accepts(stale))
    }
}
