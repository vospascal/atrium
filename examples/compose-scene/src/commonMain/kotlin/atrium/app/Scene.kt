package atrium.app

import atrium.engine.Position
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.hypot
import kotlin.math.roundToInt

/** A point in metres: x east, y north, z up. */
internal data class Point(val x: Float, val y: Float, val z: Float = 0f) {
    fun groundDistanceTo(other: Point) = hypot(x - other.x, y - other.y)
    fun hasGroundBearing(): Boolean = abs(x) >= 1e-4f || abs(y) >= 1e-4f
}

internal fun Position.toPoint() = Point(x, y, z)

/**
 * A facing as a compass bearing, the way the scene files give directions:
 * degrees clockwise from north (+Y), with the nearest of the eight compass points.
 */
internal fun bearingLabel(facing: Position): String =
    compassLabel((atan2(facing.x, facing.y) * 180.0 / PI).toFloat())

/** A compass bearing in degrees as "225° SW": whole degrees from 0 up to 360, and the nearest of eight points. */
internal fun compassLabel(bearing: Float): String {
    val degrees = ((bearing.roundToInt() % 360) + 360) % 360
    val point = CompassPoints[((degrees + 22) / 45) % 8]
    return "$degrees° $point"
}

private val CompassPoints = listOf("N", "NE", "E", "SE", "S", "SW", "W", "NW")
