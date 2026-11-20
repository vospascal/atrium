package atrium.app

import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.TimeMark
import kotlin.time.TimeSource

/**
 * Keeps engine telemetry from pulling a dragged marker backwards.
 *
 * While a sound is dragged, the UI owns its position. Telemetry trails the
 * drag by up to one frame interval (~66 ms), so applying it would make the marker
 * jump back. Telemetry is ignored during a drag, and after release until the
 * engine reports the released position or [Settle] has passed.
 */
internal class DragEcho(private val clock: TimeSource = TimeSource.Monotonic) {
    private var dragging = false
    private var released: Pair<Point, TimeMark>? = null

    fun move() {
        dragging = true
        released = null
    }

    fun release(at: Point) {
        dragging = false
        released = at to clock.markNow()
    }

    fun accepts(reported: Point): Boolean {
        if (dragging) return false
        val (target, mark) = released ?: return true
        if (reported.groundDistanceTo(target) > Tolerance && mark.elapsedNow() < Settle) return false
        released = null
        return true
    }

    fun reset() {
        dragging = false
        released = null
    }

    private companion object {
        const val Tolerance = .01f
        val Settle = 500.milliseconds
    }
}
