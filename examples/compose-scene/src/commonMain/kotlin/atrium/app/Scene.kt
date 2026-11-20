package atrium.app

import atrium.engine.Position
import kotlin.math.abs
import kotlin.math.hypot

/** A point in metres: x east, y north, z up. */
internal data class Point(val x: Float, val y: Float, val z: Float = 0f) {
    fun groundDistanceTo(other: Point) = hypot(x - other.x, y - other.y)
    fun hasGroundBearing(): Boolean = abs(x) >= 1e-4f || abs(y) >= 1e-4f
}

internal fun Position.toPoint() = Point(x, y, z)
